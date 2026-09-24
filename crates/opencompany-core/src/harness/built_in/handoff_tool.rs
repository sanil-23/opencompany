//! Handing a conversation to the teammate who should be having it.
//!
//! An operator talking to one teammate in a direct message often wants
//! somebody else: the work turned out to belong to another desk, or the first
//! teammate has taken it as far as it owns. Until now the only way through
//! was to tell the operator to go and message that person themselves.
//!
//! `hand_off` names the teammate and ends the handing agent's part. The named
//! teammate then opens **its own** direct channel with the operator and says
//! what it has picked up, so the operator has one conversation per teammate
//! and each one is with the teammate actually doing the work.
//!
//! # Why this one does not wait
//!
//! [`consult_desk`](crate::hive::consult) blocks, because the teammate calling
//! it needs the answer before it can finish its own sentence. A hand-off is
//! the opposite: the point is that this teammate is *done*. It says what it is
//! handing over and why, finishes its turn in the ordinary way, and the
//! operator reads that. Waiting would hold a turn open for a conversation it
//! is no longer part of.
//!
//! So the named teammate runs on a detached task, and its opening lands in its
//! own channel whenever it lands. The same shape `spawn_episode` uses for the
//! same reason.
//!
//! # What it deliberately does not write
//!
//! No desk row, no card, no journal entry of its own. The handing agent's
//! ordinary reply already says it handed off -- that is the sentence the
//! operator reads -- and the named teammate's opening is a row in its own
//! channel. A third row describing the hand-off would be the only one nobody
//! wrote on purpose.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Value, json};
use tinytools::{PermissionLevel, Tool, ToolResult};

use super::{HarnessDeps, PoolHandle};
use crate::error::OpenCompanyError;
use crate::ports::events::EventLog;
use crate::ports::store::CompanyStore;
use crate::ports::types::{CompanyEvent, CompanyId};

/// The tool's stable name, as the model calls it and the belt registers it.
pub const HAND_OFF_TOOL: &str = "hand_off";
/// How the driver's turn-wall error reads, as `conducted::run` stringifies it.
///
/// A string match, because the error arrives flattened into
/// [`OpenCompanyError::Harness`] and the typed variant does not survive. Same
/// classifier caveat as `steps::TRUNCATION_MARKERS`, and the same mitigation:
/// [`the_turn_wall_marker_still_matches_the_drivers_own_error`] builds the
/// real error and fails if the wording drifts, so this cannot rot silently
/// into "every hand-over failed".
const TURN_WALL_MARKER: &str = "turn wall";

/// Hand this conversation to a teammate, who picks it up in its own channel.
pub struct HandOffTool {
    company: CompanyId,
    /// The teammate handing over. It cannot hand to itself, and it is named
    /// in the brief the receiving teammate is opened with.
    agent: String,
    /// Read at call time so the roster is the current one: an operator can
    /// add a teammate mid-session, and a snapshot would refuse somebody who
    /// exists. Same reasoning as the delegation tools'.
    store: Arc<dyn CompanyStore>,
    events: Arc<dyn EventLog>,
    deps: HarnessDeps,
    pool: PoolHandle,
}

impl HandOffTool {
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

    /// Whether `to` is somebody this teammate may hand to, and why not.
    ///
    /// A pure rule over the live roster, so it can be stated and tested
    /// without a company: every refusal is something the model can correct in
    /// the same turn, which is why each one names what it saw.
    pub(crate) fn check_target(
        roster: &[String],
        retired: &[String],
        from: &str,
        to: &str,
    ) -> Result<String, String> {
        let to = to.trim();
        if to.is_empty() {
            return Err("name the teammate you are handing to".to_owned());
        }
        if to == from {
            return Err(
                "you cannot hand to yourself; answer the operator, or name somebody else"
                    .to_owned(),
            );
        }
        if retired.iter().any(|id| id == to) {
            return Err(format!(
                "`{to}` has been retired from this company and cannot pick anything up"
            ));
        }
        if !roster.iter().any(|id| id == to) {
            let mut names: Vec<&str> = roster
                .iter()
                .map(String::as_str)
                .filter(|id| *id != from)
                .collect();
            names.sort_unstable();
            return Err(format!(
                "no teammate here is called `{to}`; the ones you can hand to are: {}",
                if names.is_empty() {
                    "nobody — you are the only teammate".to_owned()
                } else {
                    names.join(", ")
                }
            ));
        }
        Ok(to.to_owned())
    }

    /// What the receiving teammate is opened with.
    ///
    /// Addressed to it as an instruction rather than journaled as a message
    /// from anybody: nobody said this, the runtime did, and a row claiming
    /// otherwise would put words in the handing teammate's mouth.
    fn opening_brief(from: &str, brief: &str) -> String {
        format!(
            "I am @{from}, and I am handing this to you. It is yours from here, and this is the \
             ask:\n\n{brief}\n\n\
             Act on it. If something in it is genuinely unclear, ask me — here, not the \
             operator, and only about what the ask does not already say.\n\n\
             **The operator hears from you, not from me, and only through `dm_operator`.** \
             Anything you say in this room reaches me alone. So tell them twice: once now, that \
             you have taken it on and what you understand it to be, and again when you have \
             something — what you produced and where it is. Between those two they are waiting, \
             so do not leave the second one unsaid. They have been talking to me about this, so \
             pick it up where it left off rather than opening as though it were new."
        )
    }
}

#[async_trait]
impl Tool for HandOffTool {
    fn name(&self) -> &str {
        HAND_OFF_TOOL
    }

    fn description(&self) -> &str {
        "Hand this conversation to the teammate who should be having it, and stop. Use it when \
         the work turns out to be somebody else's, or when you have taken it as far as you own. \
         They pick it up in their own channel with the operator and open the conversation \
         themselves, so do not promise to come back with anything. After calling this, say in \
         your reply who you handed to and why — that sentence is what the operator reads. If you \
         only need something from a teammate and are still the one answering, delegate to them \
         instead; this gives the conversation away."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "to": {
                    "type": "string",
                    "description": "The teammate taking it over, by roster id."
                },
                "brief": {
                    "type": "string",
                    "description": "The ask, written so they can act on it without coming back to you: what is being asked for, what you have already done or ruled out, what is still open, and anything the operator told you that bears on it. State the deliverable plainly — they should be able to start from this alone."
                }
            },
            "required": ["to", "brief"],
            "additionalProperties": false
        })
    }

    fn permission_level(&self) -> PermissionLevel {
        // It gives a conversation away and spends another teammate's turn. It
        // reaches nothing outside the company and decides nothing on its own.
        PermissionLevel::Write
    }

    async fn execute(&self, args: Value) -> anyhow::Result<ToolResult> {
        let brief = args
            .get("brief")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty());
        let Some(brief) = brief else {
            return Ok(ToolResult::error(
                "a hand-off needs a `brief`: they cannot see this conversation",
            ));
        };
        let Some(pool) = self.pool.get() else {
            return Ok(ToolResult::error("handing off is not wired on this host"));
        };
        let record = match self.store.load(&self.company).await {
            Ok(Some(record)) => record,
            Ok(None) => return Ok(ToolResult::error("this company is no longer loaded")),
            Err(error) => {
                return Ok(ToolResult::error(format!(
                    "this company could not be read: {error}"
                )));
            }
        };
        let roster: Vec<String> = record
            .effective_agents()
            .iter()
            .map(|agent| agent.id.clone())
            .collect();
        let to = match Self::check_target(
            &roster,
            &record.overlay_retired_agents,
            &self.agent,
            args.get("to").and_then(Value::as_str).unwrap_or_default(),
        ) {
            Ok(to) => to,
            Err(reason) => return Ok(ToolResult::error(reason)),
        };

        // **The hand-over is a real exchange, in the channel the two share.**
        //
        // It was a prompt injection with a log beside it: the ask went into
        // the receiving teammate's turn as text, and a row was *also* written
        // to the pair channel so the journal would show an agent-to-agent
        // hand-over. Only one was load-bearing and it was not the one the
        // journal showed -- the teammate never read the channel. So it runs as
        // an episode on the same machinery a consult uses, rooted at a message
        // the handing teammate actually sends.
        let channel = crate::hive::referral::pair_conversation(&self.agent, &to);
        let handed_at = match self
            .events
            .append(
                &self.company,
                CompanyEvent::AgentReply {
                    chat_id: channel.clone(),
                    agent_id: self.agent.clone(),
                    text: Self::opening_brief(&self.agent, brief),
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
            Err(error) => {
                return Ok(ToolResult::error(format!(
                    "the hand-over could not be journaled, so nobody was told: {error}"
                )));
            }
        };

        let members = vec![to.clone(), self.agent.clone()];
        let mut agents = std::collections::HashMap::new();
        for id in &members {
            if let Some(live) = pool.agent(&record.id, id).await {
                agents.insert(id.clone(), live.runtime_agent().clone());
            }
        }
        let room = match crate::hive::graph::room_hive(
            &record,
            &channel,
            &format!("{} handing to {to}", self.agent),
            &members,
            roster.len() as u64,
            &|id| agents.get(id).cloned(),
        ) {
            Ok(room) => room,
            Err(error) => return Ok(ToolResult::error(error.to_string())),
        };

        let company = self.company.clone();
        let events = Arc::clone(&self.events);
        let deps = self.deps.clone();
        let receiving = to.clone();
        let handing = self.agent.clone();
        let record = Arc::new(record);
        tokio::spawn(async move {
            // **Bounded, and by the room's own shape rather than a desk's.**
            // `desk_routing` on a channel no desk declares hands back the desk
            // defaults -- 12 rounds at width 5 -- which is sized for a room
            // deliberating a hard question. Two teammates transferring a task
            // have nothing to converge on, so with those defaults the room
            // simply ran: observed live, seven minutes, four turns, no end in
            // sight, every one of them paid for.
            //
            // Two seats, so a round is both of them; two rounds, so the
            // receiver can read the ask, put one question to the handing
            // teammate, hear the answer and get on with it. Reaching the cap
            // is not a failure -- the ask is journaled in the pair channel and
            // the receiver owns the work whether or not the room is open. The
            // room is a chance to clarify, not the hand-over itself.
            let routing = crate::hive::routing::EffectiveRouting {
                round_width: 2,
                max_rounds: 2,
                ..crate::hive::routing::desk_routing(&record, &channel)
            };
            // **A wall-clock bound as well as a turn bound**, because the
            // turn bound cannot stop a seat that never returns.
            //
            // `turn_wall` is checked in `Phase::Wall`, which the wave machine
            // only reaches once every turn in the wave has come back. A seat
            // that hangs therefore blocks the wave *before* the bound is
            // consulted, and the room waits on the runner's own per-turn
            // timeout instead -- a hardcoded 300s upstream, which
            // `HostedRunner::seat` takes no parameter for, so
            // `routing.turn_timeout_secs` (600s by default, resolved and
            // validated) reaches nothing. Observed live: a seat timed out, two
            // turns never settled, and the room sat open through six
            // checkpoints.
            //
            // So the room is bounded in time too. Reaching it is not a
            // failure for the same reason reaching the turn wall is not: the
            // ask is journaled in the pair channel and the receiver owns the
            // work whether or not the room is still open.
            let wall = routing
                .turn_timeout()
                .saturating_mul(u32::from(routing.round_width.min(u8::MAX as usize) as u8));
            let router = crate::hive::dispatch::host_router();
            let report = tokio::time::timeout(
                wall,
                crate::hive::conducted::run(crate::hive::conducted::Episode {
                    record: Arc::clone(&record),
                    deps: Arc::new(deps),
                    pool: Arc::clone(&pool),
                    events: Arc::clone(&events),
                    desk: &room,
                    routing: &routing,
                    router: router.as_deref(),
                    episode_id: uuid::Uuid::new_v4().simple().to_string(),
                    thread_root: Some(handed_at),
                    opened_at: handed_at,
                    // The receiver opens: the handing teammate has already said
                    // its piece, in the row this episode is rooted at.
                    starters: vec![receiving.clone()],
                    plan: crate::hive::routing::RoutingPlanDto::Fallback {
                        primary_id: receiving.clone(),
                        reason: "handed_off".to_owned(),
                    },
                    parking: None,
                    mentions: None,
                    // **Detached, so no seat sits inside somebody else's turn.**
                    // Awaiting it pinned the handing teammate open for the whole
                    // room -- minutes, at this model's turn time -- while the
                    // operator watched an empty channel. It has nothing to wait
                    // for: its reply is "I handed this to @to", true the moment
                    // the row above lands.
                    originator: None,
                }),
            )
            .await
            .unwrap_or_else(|_| {
                Err(OpenCompanyError::Harness(format!(
                    "the hand-over room ran past {}s and was abandoned",
                    wall.as_secs()
                )))
            });
            // **A wall is not a failure here.** The room is bounded precisely
            // because a hand-over has nothing to converge on, so running to
            // the bound is the expected ending, not a broken one: the ask is
            // in the pair channel and the receiver owns the work either way.
            // Only a room that could not run at all is worth telling the
            // operator about.
            let failed = match &report {
                Err(OpenCompanyError::Harness(text)) => {
                    !text.contains(TURN_WALL_MARKER) && !text.contains("ran past")
                }
                Err(_) => true,
                Ok(_) => false,
            };
            if let Err(error) = report
                && failed
            {
                // The operator was told somebody would pick this up. If the
                // room never ran, the channel has to say so, or the hand-off
                // is a conversation that silently went nowhere.
                tracing::warn!(
                    company = %company, agent = %receiving, %error,
                    "[hand_off] the hand-over room failed"
                );
                let row = CompanyEvent::AgentReply {
                    chat_id: receiving.clone(),
                    agent_id: receiving.clone(),
                    text: format!(
                        "@{handing} handed this to me but I could not pick it up: {error}. \
                         Please say again what you need and I will start from there."
                    ),
                    steps: Vec::new(),
                    outputs: Vec::new(),
                    task_id: None,
                    episode: None,
                    parent: None,
                    mentions: Vec::new(),
                    mention_depth: 0,
                    audience: Vec::new(),
                };
                if let Err(error) = events.append(&company, row).await {
                    tracing::warn!(
                        company = %company, agent = %receiving, %error,
                        "[hand_off] and the failure could not be journaled either"
                    );
                }
            }
        });

        Ok(ToolResult::success(format!(
            "Delivered. @{to} has the ask in the channel you share, and is taking it on there \
             now — this returns before their first turn, so say you have handed it over, not \
             that they have finished. They speak to the operator themselves from here: do not \
             relay for them and do not promise anything on their behalf. If they need something \
             from you they will ask you directly. You are no longer on this — say who you handed \
             to and why, and stop."
        )))
    }
}

#[cfg(test)]
#[path = "handoff_tool_tests.rs"]
mod tests;
