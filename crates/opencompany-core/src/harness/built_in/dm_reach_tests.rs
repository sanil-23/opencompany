//! The brief has one job: make a model in a direct message aware of two moves
//! it otherwise never reaches for. These assert what that job needs — the real
//! tool names, the distinction between the two, and the refusal it forecloses.

use super::*;

/// A brief that names a tool the belt does not call by that name teaches the
/// model an invalid call, which costs a turn to discover. Same reason
/// `team_brief` asserts this of its own section.
#[test]
fn the_brief_names_the_tools_by_their_real_names() {
    let brief = dm_reach_brief();
    for tool in [CONSULT_TEAMMATES_TOOL, HAND_OFF_TOOL] {
        assert!(brief.contains(&format!("`{tool}`")), "{brief}");
    }
}

/// Both moves end with somebody else doing something, so the brief has to
/// say what is different: whether this teammate is still the one talking to
/// the operator afterwards. Without the contrast a model picks whichever it
/// read last.
#[test]
fn the_brief_says_a_room_is_several_people_at_once() {
    let brief = dm_reach_brief();
    assert!(
        brief.contains("in the same conversation"),
        "a room is several colleagues together, not a fan-out: {brief}"
    );
    assert!(
        brief.contains("not each to you separately"),
        "and the brief must rule out the fan-out reading: {brief}"
    );
}

/// The caller names who it wants, so the brief has to say where those names
/// come from — an invented id costs a turn to discover.
#[test]
fn the_brief_says_where_the_names_come_from() {
    let brief = dm_reach_brief();
    assert!(brief.contains("roster id"), "{brief}");
    assert!(brief.contains("Your team"), "{brief}");
}

/// A room is the most expensive thing a teammate can do in a turn, and a
/// model that is not told that seats everyone it can think of.
#[test]
fn the_brief_states_what_a_room_costs() {
    let brief = dm_reach_brief();
    assert!(
        brief.contains("Name only who the question needs"),
        "{brief}"
    );
    assert!(brief.contains("most expensive thing you can do"), "{brief}");
}

/// Handing off ends this teammate's part. A model that reads it as "ask
/// somebody and carry on" promises the operator a follow-up that will never
/// come, because the reply it is waiting for lands in a different channel.
#[test]
fn the_brief_says_a_hand_off_finishes_this_teammates_part() {
    let brief = dm_reach_brief();
    assert!(brief.contains("their own channel"), "{brief}");
    assert!(brief.contains("do not promise to come back"), "{brief}");
    assert!(
        brief.contains("gives the whole conversation away"),
        "the cost has to be stated, or it reads as free: {brief}"
    );
}

/// The failure the whole team-brief line exists to fix — an agent declining
/// because the work is "not mine" — has a new escape now that a teammate can
/// hand the conversation on. The brief closes it explicitly rather than
/// leaving the model to infer it from two tool descriptions.
#[test]
fn the_brief_forecloses_declining() {
    let brief = dm_reach_brief();
    assert!(
        brief.contains("nobody's"),
        "the brief must rule out declining outright: {brief}"
    );
}

/// It is appended to a persona that already ends in prose, so it has to open
/// its own section and close cleanly — the same shape every other brief in
/// this module follows.
#[test]
fn the_brief_is_a_section_of_its_own() {
    let brief = dm_reach_brief();
    assert!(brief.starts_with("\n\n## "), "{brief:?}");
    assert!(brief.ends_with('\n'), "{brief:?}");
}
