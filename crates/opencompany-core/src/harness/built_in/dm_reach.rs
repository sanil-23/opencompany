//! The persona brief for the two tools a teammate answering an operator
//! directly uses to reach the rest of the company:
//! [`consult_desk`](crate::hive::consult) and
//! [`hand_off`](super::handoff_tool).
//!
//! # Why the tools alone were not enough
//!
//! Both were registered on the belt with careful descriptions and then, in the
//! first live run against a real model, used by nobody. The belt was not the
//! problem — a roster assertion proved both tools were on it. The prompt was.
//!
//! Every other capability this crate wires also states itself in the persona:
//! the workspace, the ledgers, the sandbox, the web, publishing, Composio, and
//! — the neighbour these two extend — hand-offs, in
//! [`member_delegation_brief`](super::orchestrator::member_delegation_brief).
//! That brief reads as a complete account of what a teammate may do with its
//! colleagues: do what is yours, hand a slice to a teammate, hand a slice to a
//! desk, fold the answer in. A model given a complete-sounding playbook does
//! not go shopping in the tool list for a move the playbook does not mention,
//! however well that move describes itself. So the two tools were invisible in
//! the only place the model was actually reading.
//!
//! The fix is not louder tool descriptions. It is saying, where the rest of the
//! story is told, that the story has two more moves.
//!
//! # Why it is conditional
//!
//! Appended from the same block that registers the tools, under the same
//! condition, so the persona can never name a tool the belt does not carry.
//! An episode seat and a host with no pool get neither the tools nor this.

use crate::hive::consult::CONSULT_TEAMMATES_TOOL;

use super::handoff_tool::HAND_OFF_TOOL;

/// The brief naming both tools, what each is for, and how to tell them apart
/// from asking one teammate.
///
/// The distinction it has to land is three-way, because all three end with
/// somebody else doing something and a model that blurs them picks the
/// cheapest by accident:
///
/// - [`CONSULT_TEAMMATES_TOOL`] — several named colleagues work it out
///   together, you keep the conversation and read back what they concluded.
/// - [`HAND_OFF_TOOL`] — somebody else takes the conversation; you are done.
///
/// Stated as *when to reach for it* rather than as a list of tools, because
/// the choice the model is making is about the situation it is in, not about
/// the API.
#[must_use]
pub fn dm_reach_brief() -> String {
    format!(
        "\n\n## When the answer is not yours alone\n\nYou are talking to the operator \
directly, and two more moves are open to you here besides answering it yourself.\n\nWhen a \
question needs several of your colleagues **in the same conversation** — a call that crosses \
their work, a trade-off with more than one right answer, a plan no one of them can size alone — \
bring them into a room with `{CONSULT_TEAMMATES_TOOL}`. Name them by roster id from Your team \
above. They talk it through among themselves, not each to you separately, and everything they \
said comes back to you before you reply — so answer the operator yourself, saying what they \
worked out. Ask the way you would ask people who have not been reading over your shoulder: they \
cannot see this conversation, so put the question, what it is for, and anything they would \
otherwise have to ask you for, all in the one message. Name only who the question needs. Every \
seat is another voice to reconcile, and a room is the most expensive thing you can do in a \
turn.\n\nWhen the conversation itself belongs to somebody else — the work turned out to be \
another's, or you have taken it as far as you own — give it to them with `{HAND_OFF_TOOL}`. They \
pick it up in their own channel with the operator and open it themselves, so you are finished: \
say in your reply who you handed it to and why, and do not promise to come back with anything. \
Use it when you are no longer the one answering — it gives the whole conversation away.\n\n\
Between them these cover the case you must never resolve by declining: there is no question here \
that is nobody\'s. If it is yours, answer it; if it needs several people together, convene them; \
if it is somebody else\'s, hand it to them by name.\n"
    )
}

#[cfg(test)]
#[path = "dm_reach_tests.rs"]
mod tests;
