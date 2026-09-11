//! Talking as a tool call.
//!
//! # Why speaking was the one thing that was not a tool
//!
//! Every other thing an agent can do in this company is a tool: it writes a
//! ledger row with `record_entry`, opens a card with `spawn_task`, reads a
//! sibling thread with `read_thread`. Speaking was not. A turn's **return text**
//! was the message, journaled on the agent's behalf by
//! [`DeskChannel::send`](crate::runtime::channel) or by the chat route. So an
//! agent could not choose a recipient, could not say something to one teammate
//! rather than to the room, and could not decline to speak — the only way to
//! stay quiet was to return an empty string, which reads as a failed turn.
//!
//! These are the tools that close that. The names, the argument shapes and the
//! description text all come from
//! [`tinyhivemind::speech`](tinyhivemind_hive::speech), which states them once,
//! as data, and asks a host to render them verbatim. Nothing here invents a
//! contract: `interpret` reads the call, and this module does what the crate's
//! own rule says a host does — **"a tool call is a request to speak; the host
//! appends, the host decides."**
//!
//! # The names are prefixed
//!
//! The crate's names are bare (`post`, `dm`, `close`, `read`) and it explicitly
//! anticipates a namespacing host: *"an MCP server called `desk` serving `post`
//! presents it as `desk_post`, and the descriptions are written to read
//! correctly either way."* This belt is namespaced, because `read` and `post`
//! are far too generic to sit unqualified beside `read_thread`,
//! `read_ledger` and `pages_read` — a model reaching for "read" would have four
//! plausible answers and no way to pick.
//!
//! # Nothing here starts a turn
//!
//! `desk_dm` is one agent addressing another, so this is the point at which the
//! agent-to-agent edge stops being hypothetical. It stays an edge that
//! **journals a row and runs nothing**.
//!
//! That is not caution for its own sake; it is the rule
//! [`CompanyEvent::AgentReply`](crate::ports::types::CompanyEvent::AgentReply)
//! already states about its own `mentions` field — *"never consulted by
//! dispatch … an agent naming another agent draws a chip and files nothing to
//! run. The edge does not exist, which is a stronger guarantee than an edge
//! that is disabled"* — and the `mention_depth` gate beside it is the bound
//! that would apply if it ever were. A recipient hears about this row the next
//! time it takes a turn, through its own session delta
//! ([`agent_session`](crate::harness::built_in::agent_session)), which is the
//! stigmergic model the whole crate is built on and needs no dispatch edge at
//! all.
//!
//! # Off by default
//!
//! Registered only when the manifest says `[speech] enabled = true`. A company
//! that does not opt in behaves byte-for-byte as it did, and an agent that has
//! the tools but answers without calling one still has its return text
//! journaled — see [`crate::harness::built_in::speech_fallback`]. Going silent
//! because a model forgot to call a tool is not an acceptable failure mode.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Value, json};

use oh::tools::traits::{PermissionLevel, Tool, ToolResult};
use openhuman_core::openhuman as oh;
use tinyhivemind_hive::speech::{self, CallArguments, ToolCall, Utterance, UtteranceRejection};

use crate::ports::events::EventLog;
use crate::ports::types::{CompanyEvent, CompanyId, EventSeq};

/// Say one thing to the whole channel.
pub const POST_TOOL: &str = "desk_post";
/// Say one thing to named teammates only.
pub const DM_TOOL: &str = "desk_dm";
/// Say one last thing and report the work finished.
pub const CLOSE_TOOL: &str = "desk_close";
/// Read further back than the window this turn was handed.
pub const READ_TOOL: &str = "desk_read";

/// Every tool name this belt registers, for the registrar and its tests.
pub const SPEECH_TOOLS: [&str; 4] = [POST_TOOL, DM_TOOL, CLOSE_TOOL, READ_TOOL];

/// The bare crate-side name behind one of ours.
///
/// The prefix is this host's, so it is stripped before the crate is asked —
/// `interpret` is documented to take the bare name.
fn bare(name: &str) -> &str {
    name.strip_prefix("desk_").unwrap_or(name)
}

/// The crate's own description for a tool, rendered verbatim.
///
/// Verbatim is the contract: the descriptions *are* the contract text, and they
/// are the only place a seat is told that text outside a tool call reaches
/// nobody. Falls back to a plain sentence only if the crate ever stops naming a
/// tool this belt registers, which its own tests make unlikely.
fn crate_description(name: &str) -> &'static str {
    speech::tool_specs()
        .iter()
        .find(|spec| spec.name == bare(name))
        .map(|spec| spec.description)
        .unwrap_or("Say one thing to this channel.")
}

/// What every speech tool needs: who is speaking, where, and the journal.
#[derive(Clone)]
pub struct SpeechContext {
    company: CompanyId,
    agent_id: String,
    events: Arc<dyn EventLog>,
    store: Arc<dyn crate::ports::store::CompanyStore>,
}

impl SpeechContext {
    pub fn new(
        company: CompanyId,
        agent_id: String,
        events: Arc<dyn EventLog>,
        store: Arc<dyn crate::ports::store::CompanyStore>,
    ) -> Self {
        Self {
            company,
            agent_id,
            events,
            store,
        }
    }

    /// The channel this turn is answering in.
    ///
    /// `None` is a refusal, not a wildcard — the same rule `read_thread`
    /// applies. A turn with no conversation (a dispatched card, a workflow
    /// node) has no channel to speak into, and posting into a guessed one would
    /// put a line in front of people who were not in the exchange.
    fn channel(&self) -> Option<String> {
        crate::runtime::delegation::turn_conversation()
    }

    /// Says one thing to the whole channel.
    ///
    /// **Does not append.** The crate's rule is that a tool call is a *request*
    /// to speak and the host appends — and the host that appends here is the
    /// reply path that has always appended, because it is the one carrying the
    /// folded steps, the live SSE frame, the resolved mentions and the
    /// board-card correlation. A tool holds none of those, so a tool that
    /// appended directly would produce a bubble poorer than the one the same
    /// sentence gets today.
    ///
    /// Falls back to appending only when there is no sink — a turn run outside
    /// the harness's tracking scope, where being heard beats being well
    /// formatted.
    async fn post_to_channel(&self, chat_id: String, text: String) -> ToolResult {
        if text.trim().is_empty() {
            return ToolResult::error(
                "A message with no text reaches nobody. Say what you mean, or call no tool at all."
                    .to_string(),
            );
        }
        if crate::runtime::delegation::collect_utterance(text.clone()) {
            crate::runtime::delegation::mark_turn_spoke();
            return ToolResult::success(
                "Said to the channel. It is journaled when this turn ends.".to_string(),
            );
        }
        self.say(chat_id, text, Vec::new()).await
    }

    /// Appends one line to the journal, with the audience the caller resolved.
    ///
    /// `audience` empty is the ordinary channel-visible case. A non-empty one is
    /// a private aside **within one desk's deliberation**: every party is on
    /// that desk, and the list narrows which of them may read the row.
    ///
    /// That is why a DM does NOT use it — see [`Self::dm`]. An `audience` is a
    /// narrowing *inside* a channel, and it cannot carry a row *across* one.
    ///
    /// This is the fallback for a post with no sink, and the append every
    /// direct row goes through.
    /// Leaves one line for each named teammate, **in that teammate's own DM
    /// channel**.
    ///
    /// # Why not the ambient channel with a narrowed audience
    ///
    /// Because that delivers nowhere, and says it delivered. It was the first
    /// shape of this method and it was wrong in a way only a live run showed:
    /// a row journaled under the *current* conversation with
    /// `audience: ["motion_designer"]` sits in the operator's DM with the
    /// speaker. Which channels reach an agent is decided by
    /// [`agent_channels`](crate::server::chat_history::agent_channels), and the
    /// speaker's own DM is not one of the recipient's — so the recipient could
    /// never read it, while the operator could. The message went to exactly the
    /// wrong person, and the tool reported success.
    ///
    /// `audience` was reached for because it needed no journal migration. It is
    /// the asides field, and an aside is a narrowing *within a desk everybody
    /// named is already on*. A DM has no such guarantee, so the narrowing has to
    /// be the channel itself.
    ///
    /// So: one row per recipient, `chat_id` set to the recipient's own DM key —
    /// the bare teammate id, which is the spelling `agent_channels` registers
    /// and the console already posts under — and an empty `audience`, because
    /// the channel has done the narrowing and a non-empty one would additionally
    /// make [`fold_asides`](crate::server::chat_history) lift the row out of the
    /// transcript as a deliberation aside, which it is not.
    ///
    /// # What it does not do
    ///
    /// Start a turn. The recipient reads this on its next turn, through its own
    /// session delta — the stigmergic model the vendored crate is built on, and
    /// the reason `AgentReply::mentions` is never consulted by dispatch. The
    /// result sentence says so rather than claiming delivery, because an agent
    /// that is told "delivered" will tell the person who asked that it was.
    async fn dm(&self, peers: Vec<String>, text: String) -> ToolResult {
        if text.trim().is_empty() {
            return ToolResult::error(
                "A message with no text reaches nobody. Say what you mean, or call no tool at all."
                    .to_string(),
            );
        }
        let mut left_for: Vec<String> = Vec::new();
        for peer in &peers {
            match self.say(peer.clone(), text.clone(), Vec::new()).await {
                result if result.is_error => return result,
                _ => left_for.push(format!("@{peer}")),
            }
        }
        ToolResult::success(format!(
            "Left for {}. Not delivered now — each of them reads it on their next turn, and \
             nothing here wakes them. If it needs doing rather than knowing, raise a card.",
            left_for.join(", "),
        ))
    }

    async fn say(&self, chat_id: String, text: String, audience: Vec<String>) -> ToolResult {
        if text.trim().is_empty() {
            return ToolResult::error(
                "A message with no text reaches nobody. Say what you mean, or call no tool at all."
                    .to_string(),
            );
        }
        let event = CompanyEvent::AgentReply {
            chat_id,
            agent_id: self.agent_id.clone(),
            text,
            steps: Vec::new(),
            task_id: None,
            parent: None,
            // Drawn as chips and read by nobody's dispatcher — see the module
            // docs. Left empty here rather than resolved: this belt does not
            // hold a company record at call time, and a half-resolved mention
            // is worse than none.
            mentions: Vec::new(),
            mention_depth: 0,
            audience,
        };
        match self.events.append(&self.company, event).await {
            Ok(seq) => {
                // The turn has now been heard. What it returns from here is
                // private thinking, and the return-text fallback must not
                // journal it a second time — see `delegation::TURN_SPOKE`.
                crate::runtime::delegation::mark_turn_spoke();
                ToolResult::success(format!("Said. Journaled at [{seq}]."))
            }
            Err(error) => ToolResult::error(format!("The message could not be journaled: {error}")),
        }
    }
}

/// Turns a crate-side refusal into the sentence handed back to the seat.
fn refusal(rejection: UtteranceRejection) -> ToolResult {
    ToolResult::error(rejection.to_string())
}

/// `desk_post` — say one thing to the whole channel.
pub struct PostTool(pub SpeechContext);

#[async_trait]
impl Tool for PostTool {
    fn name(&self) -> &str {
        POST_TOOL
    }
    fn description(&self) -> &str {
        crate_description(POST_TOOL)
    }
    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "message": {
                    "type": "string",
                    "description": "What you established, what you did not finish, and the one \
                                    teammate you need next — that teammate named first."
                }
            },
            "required": ["message"],
            "additionalProperties": false
        })
    }
    fn permission_level(&self) -> PermissionLevel {
        // Speaking is not an effect on the world outside this company, and the
        // approval bridge classifies by tool NAME rather than by this level
        // anyway (see `built_in::policy`). Declared `None` for the same reason
        // `request_approval` is: a turn that has to ask permission to answer
        // cannot answer.
        PermissionLevel::None
    }
    async fn execute(&self, args: Value) -> anyhow::Result<ToolResult> {
        let Some(channel) = self.0.channel() else {
            return Ok(ToolResult::error(
                "`desk_post` is only available while answering in a channel; this turn is not in \
                 one."
                    .to_string(),
            ));
        };
        let message = args.get("message").and_then(Value::as_str);
        let call = speech::interpret(
            bare(POST_TOOL),
            &CallArguments {
                message,
                to: &[],
                limit: None,
            },
        );
        match call {
            Ok(ToolCall::Speak(Utterance::Post { message })) => {
                Ok(self.0.post_to_channel(channel, message).await)
            }
            Ok(_) => Ok(ToolResult::error(
                "`desk_post` says one thing to the channel; it takes no other form.".to_string(),
            )),
            Err(rejection) => Ok(refusal(rejection)),
        }
    }
}

/// `desk_dm` — say one thing to named teammates instead of the whole channel.
pub struct DmTool(pub SpeechContext);

#[async_trait]
impl Tool for DmTool {
    fn name(&self) -> &str {
        DM_TOOL
    }
    fn description(&self) -> &str {
        crate_description(DM_TOOL)
    }
    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "to": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Teammate ids, without the @."
                },
                "message": { "type": "string" }
            },
            "required": ["to", "message"],
            "additionalProperties": false
        })
    }
    fn permission_level(&self) -> PermissionLevel {
        // Speaking is not an effect on the world outside this company, and the
        // approval bridge classifies by tool NAME rather than by this level
        // anyway (see `built_in::policy`). Declared `None` for the same reason
        // `request_approval` is: a turn that has to ask permission to answer
        // cannot answer.
        PermissionLevel::None
    }
    async fn execute(&self, args: Value) -> anyhow::Result<ToolResult> {
        let Some(channel) = self.0.channel() else {
            return Ok(ToolResult::error(
                "`desk_dm` is only available while answering in a channel; this turn is not in one."
                    .to_string(),
            ));
        };
        let to: Vec<String> = args
            .get("to")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(|id| id.trim().trim_start_matches('@').to_string())
                    .filter(|id| !id.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        let message = args.get("message").and_then(Value::as_str);
        let call = speech::interpret(
            bare(DM_TOOL),
            &CallArguments {
                message,
                to: &to,
                limit: None,
            },
        );
        match call {
            Ok(ToolCall::Speak(Utterance::Dm { to, message })) => {
                // Self-addressing is refused by the crate in three places and is
                // refused here too, at the one point that holds the speaker's
                // own id: a message to yourself reaches nobody else, and a row
                // whose audience is only its author is a covert channel with a
                // journal entry.
                let peers: Vec<String> = to
                    .iter()
                    .filter(|id| *id != &self.0.agent_id)
                    .cloned()
                    .collect();
                if peers.is_empty() {
                    return Ok(ToolResult::error(
                        "`to` names only you; a message to yourself reaches nobody else. Use \
                         `desk_post` to say it to the channel."
                            .to_string(),
                    ));
                }
                // Every named teammate must be on the roster. An id that
                // resolves to nobody would journal a row nobody can ever read,
                // which is worse than a refusal that says so.
                if let Ok(Some(record)) = self.0.store.load(&self.0.company).await {
                    let unknown: Vec<&String> = peers
                        .iter()
                        .filter(|id| {
                            matches!(
                                record.resolve_teammate_key(id),
                                crate::ports::types::TeammateResolution::Unknown
                            )
                        })
                        .collect();
                    if !unknown.is_empty() {
                        let names = unknown
                            .iter()
                            .map(|id| format!("@{id}"))
                            .collect::<Vec<_>>()
                            .join(", ");
                        return Ok(ToolResult::error(format!(
                            "Nobody on this company is called {names}. Check the roster and try \
                             again."
                        )));
                    }
                }
                Ok(self.0.dm(peers, message).await)
            }
            Ok(_) => Ok(ToolResult::error(
                "`desk_dm` says one thing to named teammates; it takes no other form.".to_string(),
            )),
            Err(rejection) => Ok(refusal(rejection)),
        }
    }
}

/// `desk_close` — say one last thing and report the work finished.
pub struct CloseTool(pub SpeechContext);

#[async_trait]
impl Tool for CloseTool {
    fn name(&self) -> &str {
        CLOSE_TOOL
    }
    fn description(&self) -> &str {
        crate_description(CLOSE_TOOL)
    }
    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "message": {
                    "type": "string",
                    "description": "The result, and why nothing is left open."
                }
            },
            "required": ["message"],
            "additionalProperties": false
        })
    }
    fn permission_level(&self) -> PermissionLevel {
        // Speaking is not an effect on the world outside this company, and the
        // approval bridge classifies by tool NAME rather than by this level
        // anyway (see `built_in::policy`). Declared `None` for the same reason
        // `request_approval` is: a turn that has to ask permission to answer
        // cannot answer.
        PermissionLevel::None
    }
    async fn execute(&self, args: Value) -> anyhow::Result<ToolResult> {
        let Some(channel) = self.0.channel() else {
            return Ok(ToolResult::error(
                "`desk_close` is only available while answering in a channel; this turn is not in \
                 one."
                    .to_string(),
            ));
        };
        let message = args.get("message").and_then(Value::as_str);
        let call = speech::interpret(
            bare(CLOSE_TOOL),
            &CallArguments {
                message,
                to: &[],
                limit: None,
            },
        );
        match call {
            Ok(ToolCall::Speak(Utterance::Close { message })) => {
                Ok(self.0.post_to_channel(channel, message).await)
            }
            Ok(_) => Ok(ToolResult::error(
                "`desk_close` says one last thing and reports the work finished; it takes no other \
                 form."
                    .to_string(),
            )),
            Err(rejection) => Ok(refusal(rejection)),
        }
    }
}

/// `desk_read` — read further back in this channel than the turn was handed.
pub struct ReadTool(pub SpeechContext);

#[async_trait]
impl Tool for ReadTool {
    fn name(&self) -> &str {
        READ_TOOL
    }
    fn description(&self) -> &str {
        crate_description(READ_TOOL)
    }
    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "limit": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": speech::READ_MAX,
                    "description": format!(
                        "How many recent messages to return. Default {}, max {}.",
                        speech::READ_DEFAULT, speech::READ_MAX
                    )
                }
            },
            "required": [],
            "additionalProperties": false
        })
    }
    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::ReadOnly
    }
    async fn execute(&self, args: Value) -> anyhow::Result<ToolResult> {
        let Some(channel) = self.0.channel() else {
            return Ok(ToolResult::error(
                "`desk_read` is only available while answering in a channel; this turn is not in \
                 one."
                    .to_string(),
            ));
        };
        let call = speech::interpret(
            bare(READ_TOOL),
            &CallArguments {
                message: None,
                to: &[],
                limit: args.get("limit").and_then(Value::as_u64),
            },
        );
        // The clamp is the crate's, so two hosts cannot disagree about it and a
        // seat asking for the whole transcript gets a bounded answer rather
        // than its own context window back.
        let limit = match call {
            Ok(ToolCall::Read { limit }) => limit,
            Ok(_) => speech::READ_DEFAULT,
            Err(rejection) => return Ok(refusal(rejection)),
        };

        let (desk_id, desk_name) = crate::server::chat_history::resolve_seed_desk(
            &self.0.store,
            &self.0.company,
            Some(channel.as_str()),
        )
        .await;

        let mut lines: Vec<String> = Vec::new();
        let mut cursor: Option<EventSeq> = None;
        let mut scanned = 0usize;
        // Bounded for the reason every read in this area is: a read is a recent
        // window, and hunting the whole company journal for one is a defect
        // rather than thoroughness.
        const SEARCH_PAGE: usize = 256;
        const SEARCH_BUDGET: usize = 2048;
        while lines.len() < limit && scanned < SEARCH_BUDGET {
            let page = match self
                .0
                .events
                .read_before(&self.0.company, cursor, SEARCH_PAGE)
                .await
            {
                Ok(page) => page,
                Err(error) => {
                    return Ok(ToolResult::error(format!(
                        "This channel could not be read: {error}"
                    )));
                }
            };
            if page.is_empty() {
                break;
            }
            scanned += page.len();
            cursor = page.last().map(|event| event.seq);
            for stored in page {
                if lines.len() >= limit {
                    break;
                }
                if !crate::server::chat_history::owns(&desk_id, &desk_name, &stored.event) {
                    continue;
                }
                // The same audience narrowing the session applies: a private
                // exchange this agent is not party to is not readable by asking
                // for more of the channel.
                let line = match &stored.event {
                    CompanyEvent::AgentReply {
                        agent_id,
                        text,
                        audience,
                        ..
                    } => {
                        if !audience.is_empty()
                            && agent_id != &self.0.agent_id
                            && !audience.iter().any(|member| member == &self.0.agent_id)
                        {
                            continue;
                        }
                        format!("[{}] {agent_id}: {text}", stored.seq)
                    }
                    CompanyEvent::OperatorMessage { text, .. } => {
                        format!("[{}] operator: {text}", stored.seq)
                    }
                    _ => continue,
                };
                lines.push(line);
            }
        }
        lines.reverse();
        if lines.is_empty() {
            return Ok(ToolResult::success(
                "Nothing has been said in this channel yet.".to_string(),
            ));
        }
        // A read that was cut says so — `query_company` is the cautionary case
        // this repo already names: a partial list that reads as complete
        // becomes "we have no record of that".
        let truncated = lines.len() >= limit;
        let mut body = lines.join("\n");
        if truncated {
            body.push_str(&format!(
                "\n\n(Showing the most recent {limit}. Older messages are not in this reply.)"
            ));
        }
        Ok(ToolResult::success(body))
    }
}

/// The prompt brief for a company that speaks by calling a tool.
///
/// # A tool granted, unmentioned and never called
///
/// That is the failure this repo has already named twice — `shell` wired since
/// Cell A and described in no brief, `ledger` granted and never used — and
/// speech is the worst case of it, because the fallback is *silent*. An agent
/// that never learns about `desk_post` simply answers in text, the reply path
/// journals it, and nothing anywhere reports that the feature did nothing. It
/// was observed doing exactly that on the first live run of this change.
///
/// The tool descriptions cannot carry this on their own. They say "this is the
/// only way to speak", but they say it from inside a list of forty tools, and a
/// system prompt that never mentions speaking at all outranks them.
pub fn speech_brief() -> String {
    format!(
        "\n\n## Saying things\n\
         This company talks by calling a tool. Text you write outside a tool call is your own \
         thinking and reaches nobody — it is not sent, and nobody sees it.\n\
         - `{POST_TOOL}` — say one thing to the whole channel. Call it exactly once, at the end \
         of your turn. This is how you answer.\n\
         - `{DM_TOOL}` — say one thing to named teammates instead of the whole channel, when you \
         need to settle something without spending the room's attention.\n\
         - `{CLOSE_TOOL}` — say one last thing AND report the work finished. Only when it \
         genuinely is: a result somebody still has to check is not finished.\n\
         - `{READ_TOOL}` — read further back in this channel than you were handed.\n"
    )
}

/// Every speech tool, built for one agent.
pub fn speech_belt(context: SpeechContext) -> Vec<Box<dyn Tool>> {
    vec![
        Box::new(PostTool(context.clone())),
        Box::new(DmTool(context.clone())),
        Box::new(CloseTool(context.clone())),
        Box::new(ReadTool(context)),
    ]
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::ports::types::StoredEvent;
    use futures::stream::{self, BoxStream};
    use std::sync::Mutex;

    /// A log that records what was appended, so a test can ask what actually
    /// reached the journal rather than what the tool said it did.
    struct RecordingLog(Mutex<Vec<CompanyEvent>>);

    #[async_trait]
    impl EventLog for RecordingLog {
        async fn append(&self, _id: &CompanyId, event: CompanyEvent) -> crate::Result<EventSeq> {
            let mut appended = self.0.lock().expect("test log lock");
            appended.push(event);
            Ok(EventSeq::new(appended.len() as u64))
        }
        async fn read_from(
            &self,
            _id: &CompanyId,
            _seq: EventSeq,
            _limit: usize,
        ) -> crate::Result<Vec<StoredEvent>> {
            Ok(Vec::new())
        }
        fn subscribe(
            &self,
            _id: &CompanyId,
        ) -> BoxStream<'static, crate::ports::events::EventStreamItem> {
            Box::pin(stream::empty())
        }
    }

    fn context() -> (SpeechContext, Arc<RecordingLog>, tempfile::TempDir) {
        let dir = tempfile::Builder::new()
            .prefix("speech-tools-")
            .tempdir()
            .expect("tempdir");
        let store: Arc<dyn crate::ports::store::CompanyStore> =
            Arc::new(crate::store::FsCompanyStore::new(dir.path()));
        let events = Arc::new(RecordingLog(Mutex::new(Vec::new())));
        let context = SpeechContext::new(
            CompanyId::new("acme"),
            "designer".to_string(),
            events.clone() as Arc<dyn EventLog>,
            store,
        );
        (context, events, dir)
    }

    /// The contract text is the crate's, not this host's. It is the only place a
    /// seat is told that text outside a tool call reaches nobody, so a host that
    /// paraphrased it would be quietly rewriting the rule.
    #[test]
    fn the_descriptions_are_the_crates_own() {
        for name in SPEECH_TOOLS {
            let ours = crate_description(name);
            let theirs = speech::tool_specs()
                .iter()
                .find(|spec| spec.name == bare(name))
                .expect("every registered tool is one the crate names")
                .description;
            assert_eq!(ours, theirs, "{name} paraphrased the crate");
        }
    }

    /// A post is a *request* to speak: it does not append, it is collected and
    /// the reply path appends it — with the steps, the live frame and the
    /// mentions a tool does not hold.
    #[tokio::test]
    async fn a_post_is_collected_rather_than_journaled() {
        let (context, events, _dir) = context();
        let spoken = crate::runtime::delegation::new_turn_speech();
        let result = crate::runtime::delegation::with_turn_speech(spoken.clone(), async {
            crate::runtime::delegation::with_turn_conversation(
                Some("brand".to_string()),
                PostTool(context).execute(serde_json::json!({ "message": "warmer" })),
            )
            .await
        })
        .await
        .expect("the tool runs");
        assert!(!result.is_error, "{result:?}");
        assert_eq!(spoken.utterances(), vec!["warmer".to_string()]);
        assert!(spoken.spoke());
        assert!(
            events.0.lock().expect("lock").is_empty(),
            "a post must not append; the reply path does"
        );
    }

    /// A DM is the exception and journals itself, because a narrowed audience is
    /// not something a turn's single reply can express. The audience carries the
    /// addressees and never the author — the field's documented shape.
    #[tokio::test]
    async fn a_dm_journals_itself_with_its_audience() {
        let (context, events, _dir) = context();
        let spoken = crate::runtime::delegation::new_turn_speech();
        let result = crate::runtime::delegation::with_turn_speech(spoken.clone(), async {
            crate::runtime::delegation::with_turn_conversation(
                Some("brand".to_string()),
                DmTool(context).execute(serde_json::json!({
                    "to": ["copy"],
                    "message": "between us: ship it"
                })),
            )
            .await
        })
        .await
        .expect("the tool runs");
        assert!(!result.is_error, "{result:?}");
        let appended = events.0.lock().expect("lock");
        assert_eq!(appended.len(), 1, "{appended:?}");
        let CompanyEvent::AgentReply {
            audience,
            agent_id,
            chat_id,
            ..
        } = &appended[0]
        else {
            panic!("expected an AgentReply, got {:?}", appended[0]);
        };
        assert_eq!(audience, &vec!["copy".to_string()]);
        assert_eq!(agent_id, "designer");
        assert_eq!(chat_id, "brand");
    }

    /// Self-addressing is refused in three places in the crate and is refused
    /// here too, at the one point that holds the speaker's own id. A row whose
    /// audience is only its author is a covert channel with a journal entry.
    #[tokio::test]
    async fn a_dm_to_yourself_is_refused() {
        let (context, events, _dir) = context();
        let spoken = crate::runtime::delegation::new_turn_speech();
        let result = crate::runtime::delegation::with_turn_speech(spoken, async {
            crate::runtime::delegation::with_turn_conversation(
                Some("brand".to_string()),
                DmTool(context).execute(serde_json::json!({
                    "to": ["designer"],
                    "message": "note to self"
                })),
            )
            .await
        })
        .await
        .expect("the tool runs");
        assert!(result.is_error, "{result:?}");
        assert!(events.0.lock().expect("lock").is_empty());
    }

    /// A turn with no conversation has no channel to speak into. Posting into a
    /// guessed one would put a line in front of people who were not in the
    /// exchange, so this refuses rather than defaulting.
    #[tokio::test]
    async fn speaking_outside_a_channel_is_refused() {
        let (context, events, _dir) = context();
        let result = PostTool(context)
            .execute(serde_json::json!({ "message": "anyone there?" }))
            .await
            .expect("the tool runs");
        assert!(result.is_error, "{result:?}");
        assert!(events.0.lock().expect("lock").is_empty());
    }

    /// An empty message is not silence, it is a mistake — and saying so is
    /// better than journaling a blank row.
    #[tokio::test]
    async fn an_empty_message_is_refused() {
        let (context, _events, _dir) = context();
        let spoken = crate::runtime::delegation::new_turn_speech();
        let result = crate::runtime::delegation::with_turn_speech(spoken.clone(), async {
            crate::runtime::delegation::with_turn_conversation(
                Some("brand".to_string()),
                PostTool(context).execute(serde_json::json!({ "message": "   " })),
            )
            .await
        })
        .await
        .expect("the tool runs");
        assert!(result.is_error, "{result:?}");
        assert!(!spoken.spoke());
    }
}
