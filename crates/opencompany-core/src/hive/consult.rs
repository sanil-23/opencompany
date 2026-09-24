//! Putting a question to your desk, from inside your own turn.
//!
//! A teammate answering an operator in a direct message has no room to think
//! in: its belt carries no speech tools, because those are built per seat of
//! a running episode and bound to it. So it can hand work to one teammate and
//! wait ([`DelegateToTeammateTool`](crate::harness::orchestrator::DelegateToTeammateTool)),
//! or open a card for a desk and not wait, and nothing in between lets it
//! actually *use* its colleagues.
//!
//! This is the in-between. The teammate calls one tool, an episode opens on
//! its desk with the teammate seated, and inside that episode the library's
//! own vocabulary does everything: `ask` opens a private conversation with
//! each teammate it needs, `read` looks back, `complete_episode` records. The
//! tool writes none of that. It opens the room, waits, and hands back what
//! was said.
//!
//! # Why it blocks
//!
//! Every other way this crate involves a second agent enqueues an intent and
//! lets the brain act on it after the turn. That is the right shape when the
//! caller has nothing more to say. It is the wrong shape here: the whole
//! point is that the teammate reads what the room concluded and *then*
//! decides, in the same breath, whether to answer the operator itself or hand
//! the work on. A continuation would split that decision across two turns,
//! and a seeded turn is composed from the journal rather than from where the
//! model left off.
//!
//! [`RunWorkflowTool`](crate::harness::orchestrator::RunWorkflowTool) already
//! drives the runtime this way, through the same kind of deferred handle, and
//! for the same reason.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Value, json};
use tinytools::{PermissionLevel, Tool, ToolResult};

use crate::harness::built_in::{HarnessDeps, PoolHandle};
use crate::ports::events::EventLog;
use crate::ports::store::CompanyStore;
use crate::ports::types::{CompanyEvent, CompanyId, EventSeq};

/// The tool's stable name, as the model calls it and the belt registers it.
pub const CONSULT_TEAMMATES_TOOL: &str = "consult_teammates";

/// How many journal rows the transcript reads back before giving up.
///
/// An episode that said more than this has not failed; the answer is just
/// truncated, and the rows are all on the desk for anyone who wants them.
const TRANSCRIPT_LIMIT: usize = 512;

/// The channel every ad-hoc room is journaled under, plus its episode id.
///
/// Distinct from a desk id on purpose: `channel_for` and the console both key
/// off the chat id, and a room that borrowed a desk's would put a private
/// consult into a standing channel an operator never opened.
const ROOM_PREFIX: &str = "room:";

/// The most seats one room may hold, the caller included.
///
/// Every seat is a model call per wave, so a room is the most expensive thing
/// a teammate can do in a turn. Six is the largest room that still reads as a
/// conversation rather than a queue -- and a question that genuinely needs
/// more people than that is one to put on a desk, where an operator can watch
/// it, not to run inside somebody's reply.
const MAX_ROOM: usize = 6;

/// Put a question to your desk and wait for the room to answer it.
pub struct ConsultTeammatesTool {
    company: CompanyId,
    /// The teammate whose belt this copy sits on. It is the seat the episode
    /// opens for, and the one exempt from the turn lock while it runs.
    agent: String,
    store: Arc<dyn CompanyStore>,
    events: Arc<dyn EventLog>,
    deps: HarnessDeps,
    pool: PoolHandle,
}

impl ConsultTeammatesTool {
    /// Build this teammate's copy.
    #[must_use]
    pub fn new(
        company: CompanyId,
        agent: impl Into<String>,
        store: Arc<dyn CompanyStore>,
        events: Arc<dyn EventLog>,
        deps: HarnessDeps,
        pool: PoolHandle,
    ) -> Self {
        Self {
            company,
            agent: agent.into(),
            store,
            events,
            deps,
            pool,
        }
    }

    /// The teammates to seat, with the caller first.
    ///
    /// Takes the roster rather than the desks, because a room is no longer a
    /// desk: the caller names who it wants and they are seated for this one
    /// question. `desks` used to decide this, and the rule it encoded --
    /// refuse when the teammate sits on several rooms and named none -- was a
    /// rule about *standing* rooms. It has no meaning here: there is nothing
    /// to disambiguate when the room does not exist until it is asked for.
    ///
    /// The caller is always a member and always first: it holds the question,
    /// it is the starter, and the first member is the lead every route falls
    /// back to when the router declines. Naming itself in `with` is dropped
    /// rather than refused -- it is a redundant way of saying something true.
    ///
    /// Refuses rather than guesses on an id it does not recognise. A room
    /// convened without the person the caller meant answers in the wrong
    /// voice, and its answer reads exactly like a right one.
    ///
    /// `roster` is the caller's **reach**, not the company -- see the call
    /// site for why -- so an id that exists but is out of reach is refused by
    /// the same arm as one that does not exist at all. Both are "not somebody
    /// you can bring in", which is what the refusal says.
    fn choose_room(
        roster: &[String],
        retired: &[String],
        caller: &str,
        asked_for: &[String],
    ) -> Result<Vec<String>, String> {
        let mut room = vec![caller.to_owned()];
        for name in asked_for {
            let name = name.trim();
            if name.is_empty() || name == caller || room.iter().any(|seat| seat == name) {
                continue;
            }
            if retired.iter().any(|id| id == name) {
                return Err(format!(
                    "`{name}` has been retired from this company and cannot be brought into a room"
                ));
            }
            if !roster.iter().any(|id| id == name) {
                let mut names: Vec<&str> = roster
                    .iter()
                    .map(String::as_str)
                    .filter(|id| *id != caller)
                    .collect();
                names.sort_unstable();
                return Err(format!(
                    "no teammate here is called `{name}`; the ones you can bring in are: {}",
                    match names.is_empty() {
                        true => "nobody -- you are the only teammate here".to_owned(),
                        false => names.join(", "),
                    }
                ));
            }
            room.push(name.to_owned());
        }
        if room.len() < 2 {
            return Err("name at least one teammate to bring into the room, in `with`".to_owned());
        }
        if room.len() > MAX_ROOM {
            return Err(format!(
                "a room of {} is too many to deliberate; name at most {} teammates",
                room.len(),
                MAX_ROOM - 1
            ));
        }
        Ok(room)
    }

    /// Every row this episode committed, as the asking teammate should read
    /// it back.
    ///
    /// Read from the journal rather than kept in memory: the episode's rows
    /// *are* the journal's, the host wrote them as it went, and building a
    /// second copy alongside would be a second truth to keep in step.
    async fn transcript(&self, episode_id: &str, from: EventSeq) -> String {
        let rows = match self
            .events
            .read_from(&self.company, from, TRANSCRIPT_LIMIT)
            .await
        {
            Ok(rows) => rows,
            Err(error) => {
                return format!("the room answered, but its rows could not be read back: {error}");
            }
        };
        let mut said = Vec::new();
        for row in rows {
            let CompanyEvent::AgentReply {
                agent_id,
                text,
                episode: Some(episode),
                ..
            } = &row.event
            else {
                continue;
            };
            if episode.id != episode_id {
                continue;
            }
            said.push(format!("@{agent_id}: {text}"));
        }
        if said.is_empty() {
            return "the room concluded without recording anything.".to_owned();
        }
        said.join("\n\n")
    }
}

#[async_trait]
impl Tool for ConsultTeammatesTool {
    fn name(&self) -> &str {
        CONSULT_TEAMMATES_TOOL
    }

    fn description(&self) -> &str {
        "Bring named teammates into a room together and wait while they work a question out. \
         They talk it through among themselves — not each to you separately — and everything \
         they said comes back to you here, so you can answer with it. Use this when the answer \
         needs them in the same conversation: a call that crosses their work, a trade-off with \
         more than one right answer, a plan no one of them can size alone. Name only the people \
         the question actually needs; every extra seat is another voice to reconcile. Say what \
         you need and why it matters, in full — they cannot see the conversation you are having."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "question": {
                    "type": "string",
                    "description": "What you need the room to work out, self-contained: the question, what it is for, and anything they would otherwise have to ask you for."
                },
                "with": {
                    "type": "array",
                    "items": { "type": "string" },
                    "minItems": 1,
                    "description": "The teammates to bring in, by roster id, as listed under Your team. You are in the room already — name the others. One to five of them."
                }
            },
            "required": ["question", "with"],
            "additionalProperties": false
        })
    }

    fn permission_level(&self) -> PermissionLevel {
        // It spends model calls for several teammates and writes to the
        // desk, so it is not a read — but it decides nothing on its own and
        // reaches nothing outside the company.
        PermissionLevel::Write
    }

    async fn execute(&self, args: Value) -> anyhow::Result<ToolResult> {
        let question = args
            .get("question")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty());
        let Some(question) = question else {
            return refuse("a consult needs a `question` for the room to work out");
        };
        let Some(pool) = self.pool.get() else {
            return refuse("consulting is not wired on this host");
        };
        let record = match self.store.load(&self.company).await {
            Ok(Some(record)) => Arc::new(record),
            Ok(None) => return refuse("this company is no longer loaded"),
            Err(error) => return refuse(&format!("this company could not be read: {error}")),
        };

        // The whole roster, on exactly the terms `hand_off` uses -- and
        // deliberately NOT the `delegates_to` reach this once read.
        //
        // `delegates_to` bounds delegation: who this teammate may hand a slice
        // of its own work to, capped at depth 2 (issue #884). Bounding a
        // consult by it confused two different questions. Delegation transfers
        // work and costs a depth level, so keeping it on your own desk is
        // sound; a consult transfers nothing and costs no depth. It is a
        // conversation, and `dm_reach_brief` describes it as being for exactly
        // the case that crosses desks -- "a call that crosses their work, a
        // trade-off with more than one right answer".
        //
        // Run live, the old bound made the tool useless for its own stated
        // purpose. A product manager whose `delegates_to` names its own desk
        // was asked to get the backend and security engineers into a room, and
        // was refused with "the ones you can bring in are: `designer`" --
        // while `hand_off`, the far more drastic move, would have handed those
        // same two engineers the entire conversation without complaint. The
        // cheaper move must not have the narrower reach.
        //
        // Read at call time, so a teammate added this morning can be brought
        // into a room this afternoon. `choose_room` filters the caller and
        // refuses a retired id; `MAX_ROOM` is what bounds the cost.
        let roster: Vec<String> = record
            .effective_agents()
            .iter()
            .map(|agent| agent.id.clone())
            .collect();
        let asked_for: Vec<String> = args
            .get("with")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        let members = match Self::choose_room(
            &roster,
            &record.overlay_retired_agents,
            &self.agent,
            &asked_for,
        ) {
            Ok(members) => members,
            Err(reason) => return refuse(&reason),
        };

        // Only the named seats are bound. `desk_hives` binds the whole
        // company because it builds every standing room at once; a room built
        // for one question binds the people in it and nobody else.
        let mut agents = std::collections::HashMap::new();
        for id in &members {
            if let Some(live) = pool.agent(&record.id, id).await {
                agents.insert(id.clone(), live.runtime_agent().clone());
            }
        }

        // The room's own id, unique per consult: it is a room, not a desk, so
        // it borrows no desk's name and outlives nothing. `conducted::run`
        // journals the episode under it, which is what lets an operator read
        // the exchange back without it landing in a standing channel nobody
        // opened.
        let episode_id = uuid::Uuid::new_v4().simple().to_string();
        let room_id = format!("{ROOM_PREFIX}{episode_id}");
        let room_name = format!("{} with {}", self.agent, members[1..].join(", "));
        let desk = match crate::hive::graph::room_hive(
            &record,
            &room_id,
            &room_name,
            &members,
            roster.len() as u64,
            &|id| agents.get(id).cloned(),
        ) {
            Ok(hive) => hive,
            Err(error) => return refuse(&error.to_string()),
        };

        // The question goes on the desk before the room opens, in the asking
        // teammate's own voice. It is what the episode is rooted at, it is
        // what the seats are briefed from, and it is what an operator reading
        // the desk later sees the room was convened for.
        let opened_at = match self
            .events
            .append(
                &self.company,
                CompanyEvent::AgentReply {
                    chat_id: room_id.clone(),
                    agent_id: self.agent.clone(),
                    text: question.to_owned(),
                    steps: Vec::new(),
                    outputs: Vec::new(),
                    task_id: None,
                    episode: None,
                    parent: None,
                    mentions: Vec::new(),
                    mention_depth: 0,
                    audience: Vec::new(),
                },
            )
            .await
        {
            Ok(seq) => seq,
            Err(error) => return refuse(&format!("the question could not be journaled: {error}")),
        };

        let routing = crate::hive::routing::desk_routing(&record, &room_id);
        let router = crate::hive::dispatch::host_router();
        let report = crate::hive::conducted::run(crate::hive::conducted::Episode {
            record: Arc::clone(&record),
            deps: Arc::new(self.deps.clone()),
            pool: Arc::clone(&pool),
            events: Arc::clone(&self.events),
            desk: &desk,
            routing: &routing,
            router: router.as_deref(),
            episode_id: episode_id.clone(),
            thread_root: Some(opened_at),
            opened_at,
            // **The teammates, not the caller.** The caller already holds the
            // question -- it is the row the episode is rooted at, in the
            // caller's own voice -- so seating it as a starter buys nothing
            // and costs the whole point: a seat that may speak may also call
            // `complete_episode`, and observed live, the caller took the
            // first turn, answered its own question from what it already
            // knew, and closed the room in round 1 before anybody else was
            // asked. The transcript came back, the operator got an answer,
            // and nothing about the machinery looked wrong -- it was one
            // model's opinion wearing a room's clothes.
            //
            // The caller stays a *member*: it is addressable, the seats can
            // put a question back to it, and it is first in the order so it
            // remains the deterministic fallback. It simply does not open.
            starters: members[1..].to_vec(),
            // No router decided this. The teammate did, by asking, so the
            // plan says so in the same words a router-less desk message
            // would: this seat, deterministically, and why.
            plan: crate::hive::routing::RoutingPlanDto::Fallback {
                primary_id: self.agent.clone(),
                reason: "consulted_by_seat".to_owned(),
            },
            parking: None,
            // No mention seam. Resolving a mention needs the user store, and
            // that reaches the brain from the runtime rather than a teammate's
            // deps, so a tool cannot build one. The consequence is real and
            // worth naming: a seat that `@`s a human inside a consult does not
            // notify them, where the same seat in an operator-opened episode
            // would. Carrying the seam to a teammate's own tools is the fix,
            // and it is a change of its own.
            mentions: None,
            // And it is inside its own turn while this runs, which is what
            // exempts it from taking its own lock a second time.
            originator: Some(self.agent.clone()),
        })
        .await;

        let transcript = self.transcript(&episode_id, opened_at).await;
        match report {
            // The transcript rides in the payload, not only in the rendered
            // markdown. A pooled teammate runs on a text dialect that shows
            // the model the payload and drops the rendering, so a transcript
            // that lived only there arrived as an episode id and a turn
            // count -- everything except the thing the tool exists to return.
            Ok(report) => Ok(ToolResult::success_with_markdown(
                json!({
                    "room": members,
                    "episode": episode_id,
                    "turns": report.turns,
                    "conversations": report.conversations,
                    "transcript": transcript,
                }),
                format!("{room_name} worked it through:\n\n{transcript}"),
            )),
            // A room that could not finish is still a room that said things,
            // so the transcript comes back either way: the teammate can use
            // what it got and say what is missing, which is better than being
            // told only that something went wrong.
            Err(error) => Ok(ToolResult::error(format!(
                "{room_name} did not finish: {error}\n\nWhat it said before it \
                 stopped:\n\n{transcript}"
            ))),
        }
    }
}

/// A refusal the model can act on, rather than a turn-ending failure.
///
/// Every one of these is something the model can correct in the same turn --
/// name a desk, ask a teammate instead -- so it comes back as a tool result
/// it can read, not as an error that stops the turn.
fn refuse(reason: &str) -> anyhow::Result<ToolResult> {
    Ok(ToolResult::error(reason))
}

#[cfg(test)]
#[path = "consult_tests.rs"]
mod tests;
