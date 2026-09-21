//! Completion-driven episodes: the room ends when its members say it has.
//!
//! [`super::episode`] ends a room by *inference* — a quorum it can name, a
//! deadlock, a spent budget, or nobody left with anything to say. This module is
//! the alternative, and the library is explicit that it is an alternative rather
//! than a fifth rung inside that fold: an episode here ends when **every
//! currently assigned member has explicitly reported its own assignment
//! finished**, and nothing else is interpreted.
//!
//! # Why this exists beside quorum rather than under it
//!
//! Quorum counts *distinct grounded supporters*, so it cannot express a room of
//! one: [`super::types::HiveConfig::deliberates`] floors a room at two members,
//! and `desk_episode` additionally refuses a desk that cannot reach its own
//! configured quorum with its live roster. A two-seat exchange on a desk whose
//! `hive.quorum` is three is therefore not a small room — it is no room, and it
//! silently falls back to a single responder's turn.
//!
//! Completion has no quorum, no standings and no blind round, so it is
//! well-defined at one assignee and at five alike. That is the whole reason to
//! have both: quorum is what you select when you want supporters counted;
//! completion is what you select when you want the work reported done.
//!
//! # Why the marker is not a permissioned move
//!
//! Reporting your own assignment finished is not a claim about the topic, so it
//! grants nothing and takes nothing from anybody: it cannot carry a proposal,
//! cannot supply a supporter, and cannot break a tie. Gating it through the
//! `moves` table would let a manifest configure a member that can be *given*
//! work and can never hand it back, which is a deadlock a desk could author by
//! accident. [`super::ASIDE_MARKER`] and [`super::SURFACE_MARKER`] sit outside
//! [`MOVE_KINDS`](super::moves::MOVE_KINDS) for the same reason.
//!
//! # Reopening is the point, not an edge case
//!
//! A member that reported finished is *not* done for the episode: a later
//! assignment reopens exactly its recipients and nobody else. That is what makes
//! a handoff chain terminate correctly — solver finishes, checker finds a fault
//! and assigns it back, and the episode is live again with one pending member
//! rather than having ended on the first report.

use tinyhivemind::Sequence;
use tinyhivemind_hive::{
    CompletionEpisodeState, CompletionStep, apply_assignment, apply_completion, completion_status,
};

use crate::Result;
use crate::error::OpenCompanyError;

/// The marker a member ends its turn with to report its assignment finished.
///
/// Spelled like every other marker this module reads so a seat holds one
/// grammar, not two. The upstream crate advertises the same act as a
/// `complete_episode` *tool*, which is the right shape on the chat path where a
/// belt is in scope — [`crate::harness::speech_tools`] registers it as
/// `desk_close`. A hive turn has no belt: [`super::HiveTurnRunner::speak`]
/// returns a `String`, so the room reads markers out of the reply exactly as it
/// does for `!support` and `!refute`.
pub const COMPLETE_MARKER: &str = "!complete";

/// The marker a member hands work on with, without naming who takes it.
///
/// The counterpart to [`COMPLETE_MARKER`], and the other half of what the
/// reference runner gives a seat: `broadcast` and `complete_episode` are the
/// only two actions its agents have. There it is an MCP tool because an
/// OpenHuman agent holds a belt; a hive turn here has none —
/// [`super::HiveTurnRunner::speak`] returns a `String` — so it is a marker,
/// read out of the reply exactly as `!support` and `!complete` are.
pub const BROADCAST_MARKER: &str = "!broadcast";

/// The work one line hands on, or `None` when it hands on nothing.
///
/// The text after the marker is both the desk row and what the router matches
/// against — there is no separate routing hint, so a vague broadcast is
/// simultaneously a poor transcript row and a poor routing signal.
/// Whether one authored line IS a handoff marker, bodied or bare.
///
/// Exact, not `starts_with`: `!broadcasting the results` opens with the
/// marker's letters and is prose, so it must be left alone. Mirrors
/// [`reports_completion`]'s rule — the marker, then end-of-line or a space.
#[must_use]
pub fn is_broadcast(line: &str) -> bool {
    line.trim_start()
        .strip_prefix(BROADCAST_MARKER)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with(char::is_whitespace))
}

#[must_use]
pub fn broadcast_body(line: &str) -> Option<&str> {
    let rest = line.trim_start().strip_prefix(BROADCAST_MARKER)?;
    // The marker must END here, or `!broadcasting the results` parses as a
    // handoff carrying "ing the results" — and this feeds `reply_broadcast`,
    // which feeds `route_handoff`, so a sentence about broadcasting would have
    // spent a real routing call on a fragment of its own verb. Same rule
    // `reports_completion` already applied to `!complete`.
    if !rest.is_empty() && !rest.starts_with(char::is_whitespace) {
        return None;
    }
    let body = rest.trim();
    (!body.is_empty()).then_some(body)
}

/// The work the first broadcast in one reply hands on.
///
/// First rather than every: a turn is one move, and a seat that wrote two
/// broadcasts has asked for two routing calls on one turn. Taking the first
/// keeps the cost of a turn bounded by the turn, not by what the model wrote.
#[must_use]
pub fn reply_broadcast(reply: &str) -> Option<&str> {
    reply.lines().find_map(broadcast_body)
}

/// Whether one authored line reports its author's assignment finished.
///
/// Leading-marker only, matching [`super::moves::line_kind`]: a line that
/// merely mentions the marker mid-sentence is prose about completing, not a
/// report of it.
#[must_use]
pub fn reports_completion(line: &str) -> bool {
    line.trim_start()
        .strip_prefix(COMPLETE_MARKER)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with(char::is_whitespace))
}

/// Whether any line of one reply reports completion.
///
/// A whole turn rather than a line, because a member may say what it found and
/// then report — the report is the last thing it does, not the only thing.
#[must_use]
pub fn reply_reports_completion(reply: &str) -> bool {
    reply.lines().any(reports_completion)
}

/// Open an episode over the members that have been assigned its opening work.
///
/// # Errors
///
/// Returns [`OpenCompanyError::Config`] when the assignment names nobody, names
/// a blank id, or names one member twice — each of which can only be a bug in
/// the caller that built the roster snapshot.
pub fn opened(
    conversation: tinyhivemind::Conversation,
    watermark: Sequence,
    roster: &[String],
    assigned: &[String],
) -> Result<CompletionEpisodeState> {
    // **The roster says who MAY be assigned; only `assigned` owes a report.**
    //
    // Seeded complete and then assigned, which is what the reference runner
    // does. Opening every seat pending instead looks equivalent and is not: a
    // six-member desk could then not end until all six spoke, and a member the
    // work never reached would be asked for an evidence-dense result it had no
    // way to produce. Observed live — a seat wrote a bare `!complete` with
    // nothing after it, because it had nothing to complete.
    //
    // It also keeps the opening route load-bearing: whoever routing selected is
    // who the room is waiting on, rather than routing picking a first speaker
    // and the termination ignoring it.
    // A room with nobody in it reports Complete on its first pass, which reads
    // as "the work is done" when what happened is that there was never any. A
    // desk that cannot seat anybody is a caller bug, and saying so beats
    // ending silently.
    if roster.is_empty() {
        return Err(OpenCompanyError::Config(
            "hive completion episode: a room with no members can never do work".to_owned(),
        ));
    }
    // Opening with nobody assigned is the same bug one step along: every seat
    // is seeded COMPLETE, so a room that reopens none of them reports
    // `Complete` on its first pass — "the work is done" when no work was ever
    // given out. `desk_episode` cannot reach this (it falls back to the
    // desk's first member), but this function is public and its contract is
    // "the members that have been assigned its opening work". Refusing beats
    // ending silently, for the same reason the empty roster does.
    // (CodeRabbit on #2412.)
    if assigned.is_empty() {
        return Err(OpenCompanyError::Config(
            "hive completion episode: opening work must be assigned to at least one member"
                .to_owned(),
        ));
    }
    let mut state = CompletionEpisodeState {
        conversation,
        watermark,
        participants: roster
            .iter()
            .map(|agent_id| tinyhivemind_hive::ParticipantCompletion {
                agent_id: agent_id.clone(),
                // Everyone opens COMPLETE at the watermark, and the opening
                // assignment below reopens only those who owe work. A seat
                // never assigned therefore owes no report and cannot hold the
                // room open — which is what makes a room of one terminate.
                //
                // Seeded AT the watermark, not below it. `Sequence(0)` was the
                // obvious floor and it is wrong: `apply_assignment` refuses an
                // assignment that does not strictly advance
                // (`at <= assigned_at`), so seeding at 0 works only while the
                // trigger is itself above 0. A desk answering its FIRST message
                // has a trigger of `Sequence(0)`, and the opening assignment
                // was then rejected as a stale event — the whole episode
                // failing with `stale completion event for <id> at sequence
                // Sequence(0)`. Caught by two brain tests that run a hive
                // episode through a fresh cycle.
                assigned_at: watermark,
                completed_at: Some(watermark),
            })
            .collect(),
    };
    // Reopened by hand rather than through [`assigned`], for the same reason:
    // the opening assignment lands ON the watermark, which that function reads
    // as failing to advance. Nothing is superseded here — these seats have done
    // no work yet — so there is no staleness to guard against.
    for id in assigned {
        let Some(participant) = state
            .participants
            .iter_mut()
            .find(|participant| &participant.agent_id == id)
        else {
            return Err(OpenCompanyError::Config(format!(
                "hive completion episode: `{id}` was assigned opening work but is not seated"
            )));
        };
        participant.completed_at = None;
    }
    Ok(state)
}

/// Record that `agent_id` reported its current assignment finished.
///
/// # Errors
///
/// [`OpenCompanyError::Config`] when the agent is not a participant, or when the
/// sequence is not above the one its assignment was made at — a report that
/// predates its own work is a replayed row, not a completion.
pub fn completed(
    state: &CompletionEpisodeState,
    agent_id: &str,
    at: Sequence,
) -> Result<CompletionEpisodeState> {
    apply_completion(state, agent_id, at)
        .map_err(|error| OpenCompanyError::Config(format!("hive completion: {error}")))
}

/// Record new work assigned to `recipients`, reopening exactly those members.
///
/// # Errors
///
/// [`OpenCompanyError::Config`] as [`completed`], for the same class of caller
/// bug.
pub fn assigned(
    state: &CompletionEpisodeState,
    recipients: &[String],
    at: Sequence,
) -> Result<CompletionEpisodeState> {
    apply_assignment(state, recipients, at)
        .map_err(|error| OpenCompanyError::Config(format!("hive assignment: {error}")))
}

/// Whether the episode may end, and who it is still waiting on.
#[must_use]
pub fn step(state: &CompletionEpisodeState) -> CompletionStep {
    completion_status(state)
}

/// The members the episode is still waiting on, in stable opening order.
///
/// Empty exactly when [`step`] says the episode is complete, which is the form
/// a driver loop wants: it schedules the next turn from this list and stops when
/// the list empties.
#[must_use]
pub fn pending(state: &CompletionEpisodeState) -> Vec<String> {
    match step(state) {
        CompletionStep::Active { pending_ids } => pending_ids,
        CompletionStep::Complete { .. } => Vec::new(),
    }
}

#[cfg(test)]
#[path = "completion_tests.rs"]
mod tests;

/// One completion-episode line as a person should read it, or `None` when the
/// line carries no marker of this module's.
///
/// Three renderings, and the stored row is untouched by all of them — the fold
/// reads markers off the journal, so a projection that rewrote them would leave
/// the episode unable to read its own transcript. This mirrors
/// [`moves::readable`](super::moves::readable), which does the same for the
/// deliberation grammar.
///
/// * `!broadcast <work>` renders as the work alone. The marker is addressed to
///   the host, not to the room: the room only ever needed the finding, and a
///   reader who sees `!broadcast` is being shown plumbing.
/// * `!complete <result>` renders as the result alone, for the same reason.
/// * a **bare** `!complete` renders as nothing at all. It is a seat reporting
///   to the host that its assignment is finished and carrying no result to
///   report — a row with no content for a person, which belongs in the fold
///   and not in a transcript somebody reads.
#[must_use]
pub fn readable(line: &str) -> Option<String> {
    if let Some(work) = broadcast_body(line) {
        return Some(work.to_owned());
    }
    if reports_completion(line) {
        let rest = line
            .trim_start()
            .strip_prefix(COMPLETE_MARKER)
            .unwrap_or_default()
            .trim();
        // A bare marker that survived the correction retry still SURFACES: it
        // is the member's reply, and a row that renders to nothing would leave
        // an operator looking at a turn that appears not to have happened.
        // Rendered as the plain sentence rather than the marker, because the
        // marker is addressed to the host and the fact is addressed to the room.
        if rest.is_empty() {
            return Some("Reported this assignment finished.".to_owned());
        }
        return Some(rest.to_owned());
    }
    // Same for a broadcast whose body did not survive: `broadcast_body` refuses
    // an empty one, so it falls through to here rather than above.
    if is_broadcast(line) {
        return Some("Handed this on, but carried no detail with it.".to_owned());
    }
    None
}

/// Whether one line carries a marker this module owns.
///
/// The cheap pre-check `readable_moves` makes before rewriting anything, so a
/// transcript with no completion grammar in it is returned untouched.
#[must_use]
pub fn is_completion_line(line: &str) -> bool {
    // `is_broadcast`, not `broadcast_body().is_some()`: a BARE `!broadcast`
    // has no body, so the old form answered `false` for it and
    // `readable_moves` skipped the rewrite entirely — leaking the raw marker
    // into the console, which is the one thing this pre-check exists to stop.
    // A retry can return a second bare marker, so the case is reachable.
    // (CodeRabbit on #2412.)
    is_broadcast(line) || reports_completion(line)
}

/// The correction a seat is handed when its marker carried nothing, or `None`
/// when the line is fine.
///
/// A bare `!broadcast` reaches nobody: [`broadcast_body`] finds no work, so the
/// router has nothing to match a candidate against and the hand-off silently
/// does not happen. A bare `!complete` reports a finished assignment and no
/// result, which is a completion the room cannot check.
///
/// Written to be read by the seat mid-turn, the way
/// [`moves::correction`](super::moves::correction) is — it is shown inside the
/// same turn, so the fix is one retry rather than a lost hand-off.
#[must_use]
pub fn bare_marker_correction(line: &str) -> Option<String> {
    let trimmed = line.trim_start();
    let bare = |marker: &str| {
        trimmed
            .strip_prefix(marker)
            .is_some_and(|rest| rest.trim().is_empty())
    };
    if bare(BROADCAST_MARKER) {
        return Some(format!(
            "That `{BROADCAST_MARKER}` carried nothing, so it reached nobody. The work and the \
             marker are ONE line: `{BROADCAST_MARKER}` then the finding, the command and its \
             output, the counterexample — everything the next teammate needs without re-reading \
             your turn. Write that line now. Do not name who should take it."
        ));
    }
    if bare(COMPLETE_MARKER) {
        return Some(format!(
            "That `{COMPLETE_MARKER}` carried no result, so nothing can be checked. The result \
             and the marker are ONE line: `{COMPLETE_MARKER}` then what you established and the \
             evidence it rests on. Write that line now, or hand the work on with \
             `{BROADCAST_MARKER}` if it is not finished."
        ));
    }
    None
}
