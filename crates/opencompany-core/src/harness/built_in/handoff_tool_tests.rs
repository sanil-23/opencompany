//! Who a teammate may hand to, and what it is told when it may not.

use super::{HandOffTool, TURN_WALL_MARKER};

fn check(roster: &[&str], retired: &[&str], from: &str, to: &str) -> Result<String, String> {
    let roster: Vec<String> = roster.iter().map(|id| (*id).to_owned()).collect();
    let retired: Vec<String> = retired.iter().map(|id| (*id).to_owned()).collect();
    HandOffTool::check_target(&roster, &retired, from, to)
}

#[test]
fn a_teammate_on_the_roster_is_accepted() {
    assert_eq!(
        check(&["ceo", "engineer"], &[], "ceo", "engineer").as_deref(),
        Ok("engineer")
    );
}

/// A model that pads an id would otherwise be refused for a space.
#[test]
fn the_name_is_taken_trimmed() {
    assert_eq!(
        check(&["ceo", "engineer"], &[], "ceo", "  engineer ").as_deref(),
        Ok("engineer")
    );
}

/// Handing to yourself is a loop: your turn ends, and the runtime opens you
/// again with your own brief, forever.
#[test]
fn handing_to_yourself_is_refused() {
    let refusal = check(&["ceo", "engineer"], &[], "ceo", "ceo").expect_err("self hand-off");

    assert!(refusal.contains("yourself"), "{refusal}");
}

/// The refusal lists who *is* available, so the model can correct itself in
/// the same turn rather than guessing again.
#[test]
fn an_unknown_teammate_is_refused_with_the_ones_that_exist() {
    let refusal =
        check(&["ceo", "engineer", "writer"], &[], "ceo", "designer").expect_err("no such id");

    assert!(refusal.contains("designer"), "{refusal}");
    assert!(refusal.contains("engineer"), "{refusal}");
    assert!(refusal.contains("writer"), "{refusal}");
}

/// And never offers the caller itself as a choice.
#[test]
fn the_choices_never_include_the_one_asking() {
    let refusal = check(&["ceo", "engineer"], &[], "ceo", "designer").expect_err("no such id");

    assert!(!refusal.contains("ceo"), "{refusal}");
}

/// A retired teammate is on no roster but may still be named from an older
/// part of the conversation. It gets its own reason rather than "no such
/// teammate", which would be a different and wrong fact.
#[test]
fn a_retired_teammate_is_refused_as_retired() {
    let refusal = check(&["ceo", "engineer"], &["writer"], "ceo", "writer").expect_err("retired");

    assert!(refusal.contains("retired"), "{refusal}");
}

#[test]
fn an_empty_name_asks_for_one() {
    let refusal = check(&["ceo", "engineer"], &[], "ceo", "   ").expect_err("no name");

    assert!(refusal.contains("name the teammate"), "{refusal}");
}

/// A company of one has nobody to hand to, and is told that rather than
/// handed an empty list.
#[test]
fn a_lone_teammate_is_told_there_is_nobody() {
    let refusal = check(&["ceo"], &[], "ceo", "engineer").expect_err("nobody else");

    assert!(refusal.contains("only teammate"), "{refusal}");
}

/// [`TURN_WALL_MARKER`] is matched against a stringified error, so it rots the
/// moment the driver rewords it — and it rots *silently*, into "every
/// hand-over failed and the operator was told so". Build the real error and
/// check, rather than trusting the two to stay in step.
#[test]
fn the_turn_wall_marker_still_matches_the_drivers_own_error() {
    let wall =
        tinyhivemind_openhuman::Error::Conduct(tinyhivemind_driver::Error::TurnWall { wall: 4 });
    let flattened = crate::error::OpenCompanyError::Harness(wall.to_string()).to_string();

    assert!(
        flattened.contains(TURN_WALL_MARKER),
        "the driver now says {flattened:?}, which `{TURN_WALL_MARKER}` no longer matches"
    );
}
