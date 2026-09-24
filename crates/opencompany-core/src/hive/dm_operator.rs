//! A seat's one way to say something to the operator without leaving the room.
//!
//! # Why a seat needs this
//!
//! Inside an episode a teammate can reach the other seats (`ask`, `post`,
//! `broadcast`) and it can stop and wait for a human
//! ([`escalate_to_human`](crate::harness::built_in::blockers)). It has had
//! nothing in between — no way to simply *tell* the operator something and
//! carry on.
//!
//! That gap showed up the first time a hand-off worked end to end. The teammate
//! picking the work up opened with three scoping questions, and the only
//! surface it had was the operator's. Two of the three were questions the
//! teammate that handed it over could have answered, and the operator was left
//! relaying between two agents — which is the exact job this whole feature
//! exists to remove.
//!
//! # Fire and forget, deliberately
//!
//! It does not park, does not block and returns nothing but a receipt. That is
//! the whole distinction from `escalate_to_human`, and it is worth keeping
//! sharp because the two read alike and cost very differently:
//!
//! * `escalate_to_human` — *I cannot continue until a human answers.* The work
//!   stops, the episode holds the seat
//!   ([`SeatParking`](crate::hive::host::SeatParking)), and nothing moves until
//!   somebody replies.
//! * `dm_operator` — *you should know this.* Said and done; the seat takes its
//!   next turn immediately and the room carries on.
//!
//! A seat that used the first where it meant the second would stall a room on a
//! remark. A seat that used the second where it meant the first would act on an
//! answer nobody gave.
//!
//! # Where it lands
//!
//! The operator's own channel with **this** teammate, under the bare roster id
//! — the spelling the chat route normalizes to and the console reads back. A
//! row addressed as `dm:<id>` instead would be filed where nothing displays it,
//! which is a mistake this crate has already made once
//! ([`hand_off`](crate::harness::built_in::handoff_tool)).

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Value, json};
use tinytools::{PermissionLevel, Tool, ToolResult};

use crate::ports::events::EventLog;
use crate::ports::types::{CompanyEvent, CompanyId};

/// The tool's stable name, as the model calls it and the belt registers it.
pub const DM_OPERATOR_TOOL: &str = "dm_operator";

/// The persona paragraph naming the tool, for a seat's prompt.
///
/// Short on purpose. A seat carries its tools natively — the whole belt is on
/// its tool list with each description attached — so this does not have to
/// teach the call, only the *choice*: which of the three ways out of a room
/// this is, and when it is the wrong one.
#[must_use]
pub fn dm_operator_brief() -> String {
    format!(
        "\n\n## Telling the operator something\n\nYou are in a room with your colleagues, and \
the operator cannot see it. When something here is worth their knowing — what you found, what \
you did, something that changes what they should expect — say it with `{DM_OPERATOR_TOOL}`. It \
lands in your own conversation with them and nothing waits: you keep your turn and the room \
carries on. Write it for somebody who was not here, so say the thing in full rather than \
pointing at what was just said.\n\nIt is not for a question you need answered. If you cannot \
continue without the operator, escalate instead — that stops and waits, which is the whole \
difference. And if what you need is from one of the people in this room, ask them; the operator \
is not a relay between you.\n"
    )
}

/// Say something to the operator from inside a room, without waiting.
pub struct DmOperatorTool {
    company: CompanyId,
    /// The seat this copy belongs to. It is both the author of the row and the
    /// channel it lands in: a teammate speaks to the operator in its own
    /// conversation, never in somebody else's.
    agent: String,
    events: Arc<dyn EventLog>,
}

impl DmOperatorTool {
    /// Build this seat's copy.
    #[must_use]
    pub fn new(company: CompanyId, agent: impl Into<String>, events: Arc<dyn EventLog>) -> Self {
        Self {
            company,
            agent: agent.into(),
            events,
        }
    }
}

#[async_trait]
impl Tool for DmOperatorTool {
    fn name(&self) -> &str {
        DM_OPERATOR_TOOL
    }

    fn description(&self) -> &str {
        "Tell the operator something, and carry on. The message lands in your own direct \
         conversation with them and you keep working — nothing waits for a reply. Use it to \
         report what you have found or done, or to flag something they need to know about while \
         the room continues. Write it for somebody who cannot see this room: say the thing in \
         full rather than referring to what was just said here. If you cannot go on until they \
         answer, this is the wrong tool — escalate instead, which stops and waits. And if what \
         you need is from a teammate rather than the operator, ask the teammate."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "message": {
                    "type": "string",
                    "description": "What you are telling the operator, self-contained: they cannot see the room you are in."
                }
            },
            "required": ["message"],
            "additionalProperties": false
        })
    }

    fn permission_level(&self) -> PermissionLevel {
        // It writes one row to a conversation the operator already has with
        // this teammate. It reaches nothing outside the company, spends no
        // other teammate's turn, and decides nothing.
        PermissionLevel::Write
    }

    async fn execute(&self, args: Value) -> anyhow::Result<ToolResult> {
        let message = args
            .get("message")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty());
        let Some(message) = message else {
            return Ok(ToolResult::error(
                "say what you want the operator to know, in `message`",
            ));
        };
        let row = CompanyEvent::AgentReply {
            chat_id: self.agent.clone(),
            agent_id: self.agent.clone(),
            text: message.to_owned(),
            steps: Vec::new(),
            outputs: Vec::new(),
            task_id: None,
            episode: None,
            parent: None,
            mentions: Vec::new(),
            mention_depth: 0,
            audience: Vec::new(),
        };
        match self.events.append(&self.company, row).await {
            Ok(_) => Ok(ToolResult::success(
                "Told the operator. They will read it in your own conversation; nothing is \
                 waiting on them, so carry on.",
            )),
            Err(error) => Ok(ToolResult::error(format!(
                "that could not be delivered to the operator: {error}"
            ))),
        }
    }
}

#[cfg(test)]
#[path = "dm_operator_tests.rs"]
mod tests;
