//! Who a consult seats, and what it refuses rather than guess.

use super::ConsultTeammatesTool;
use crate::harness::built_in::PoolHandle;

/// The rule is about roster membership and nothing else, so it is driven
/// against a roster rather than a company: a hive is a validated graph that
/// needs a runtime to bind, and none of that participates in this decision.
fn room(
    roster: &[&str],
    retired: &[&str],
    caller: &str,
    with: &[&str],
) -> Result<Vec<String>, String> {
    let roster: Vec<String> = roster.iter().map(|id| (*id).to_owned()).collect();
    let retired: Vec<String> = retired.iter().map(|id| (*id).to_owned()).collect();
    let with: Vec<String> = with.iter().map(|id| (*id).to_owned()).collect();
    ConsultTeammatesTool::choose_room(&roster, &retired, caller, &with)
}

/// The caller holds the question and is the episode's starter, so it is in
/// the room whether or not it said so -- and first, because the first member
/// is the lead every route falls back to when the router declines.
#[test]
fn the_caller_is_seated_first_without_naming_itself() {
    let seated = room(
        &["engineer", "designer", "qa"],
        &[],
        "engineer",
        &["designer", "qa"],
    );

    assert_eq!(
        seated.as_deref(),
        Ok(["engineer", "designer", "qa"].map(str::to_owned).as_slice())
    );
}

/// Naming yourself is a redundant way of saying something already true, not
/// a mistake worth spending a turn on.
#[test]
fn naming_yourself_is_dropped_rather_than_refused() {
    let seated = room(&["engineer", "qa"], &[], "engineer", &["engineer", "qa"]);

    assert_eq!(
        seated.as_deref(),
        Ok(["engineer", "qa"].map(str::to_owned).as_slice())
    );
}

#[test]
fn a_teammate_named_twice_is_seated_once() {
    let seated = room(&["engineer", "qa"], &[], "engineer", &["qa", "qa"]);

    assert_eq!(
        seated.as_deref(),
        Ok(["engineer", "qa"].map(str::to_owned).as_slice())
    );
}

/// A room convened without the person the caller meant answers in the wrong
/// voice, and its answer reads exactly like a right one -- so an id the
/// roster does not know is refused, and told what it could have said.
#[test]
fn an_unknown_teammate_is_refused_with_the_ones_that_exist() {
    let refusal =
        room(&["engineer", "qa"], &[], "engineer", &["marketing"]).expect_err("no such teammate");

    assert!(refusal.contains("marketing"), "{refusal}");
    assert!(refusal.contains("qa"), "{refusal}");
    // Never offers the caller back to itself as a choice.
    assert!(!refusal.contains("engineer,"), "{refusal}");
}

#[test]
fn a_retired_teammate_is_refused_by_name() {
    let refusal = room(&["engineer", "qa"], &["qa"], "engineer", &["qa"])
        .expect_err("retired teammates answer nothing");

    assert!(
        refusal.contains("qa") && refusal.contains("retired"),
        "{refusal}"
    );
}

/// A room of one is the caller talking to itself. The refusal names the
/// argument to fix, because the model can correct it in the same turn.
#[test]
fn a_room_of_one_is_refused() {
    let refusal = room(&["engineer", "qa"], &[], "engineer", &[]).expect_err("nobody named");

    assert!(refusal.contains("`with`"), "{refusal}");
}

/// Every seat is a model call per wave, so the cap is a cost bound, and the
/// refusal says what the limit is rather than just that one was hit.
#[test]
fn a_room_past_the_cap_is_refused_with_the_limit() {
    let roster = ["a", "b", "c", "d", "e", "f", "g", "h"];
    let refusal = room(&roster, &[], "a", &roster[1..]).expect_err("too many seats");

    assert!(
        refusal.contains(&(super::MAX_ROOM - 1).to_string()),
        "{refusal}"
    );
}

#[test]
fn a_room_exactly_at_the_cap_is_allowed() {
    let roster = ["a", "b", "c", "d", "e", "f"];
    let seated = room(&roster, &[], "a", &roster[1..]).expect("six seats is the cap");

    assert_eq!(seated.len(), super::MAX_ROOM);
}

/// An unwired handle is empty, which is what keeps the tool off a belt that
/// could never open an episode.
#[test]
fn an_unfilled_handle_reports_no_pool() {
    assert!(PoolHandle::default().get().is_none());
}
