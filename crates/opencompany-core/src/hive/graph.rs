//! One `OpenHumanHive` per desk, bound over the company's live agents.
//!
//! A hive is tinyhivemind's immutable view of one desk: the desk record, one
//! `RouteCandidate` per seat for the router, and one `AgentBinding` per seat
//! onto an already-built `openhuman_embed::Agent`. A shared agent — the CEO
//! on both the engineering and the content desk — is the **same** runtime
//! handle cloned into both hives; what keeps its turns from overlapping is
//! the per-agent `turn_lock` the harness pool holds, not the hive.
//!
//! Built from the same snapshot every other desk reader uses
//! (`delegation_tools::tinyhivemind_desks`), so a console-created desk, an
//! overlay member and an operator's reorder are all in the graph the router
//! sees. Rebuilt whole with the roster: a hive is cheap, and a stale one
//! would route to a seat that no longer sits there.

use std::collections::HashMap;
use std::sync::Arc;

use tinyhivemind_driver::{AgentBinding, BoundHive, HiveGraph};
use tinyhivemind_embed::RouteCandidate;
use tinyhivemind_openhuman::EmbedSeat;

use crate::ports::types::CompanyRecord;

/// One desk's hive, with the identity the driver keys on.
#[derive(Debug)]
pub struct DeskHive {
    /// The desk id.
    pub desk_id: String,
    /// The desk's display name, for the session log and the prompt.
    pub desk_name: String,
    /// The validated graph and bindings.
    pub hive: BoundHive<EmbedSeat>,
    /// The candidate snapshot version a Jev evaluation must echo.
    pub roster_version: u64,
}

impl DeskHive {
    /// The canonical members, in desk order.
    pub fn members(&self) -> Vec<String> {
        self.hive.members().map(str::to_string).collect()
    }

    /// The desk lead — the first member — the deterministic fallback every
    /// route on this desk has.
    #[must_use]
    pub fn lead(&self) -> Option<String> {
        self.hive.members().next().map(str::to_string)
    }
}

/// Why a desk got no hive.
#[derive(Debug, thiserror::Error)]
pub enum HiveBuildError {
    /// tinyhivemind refused the graph.
    #[error("desk `{desk_id}`: {source}")]
    Invalid {
        /// The desk.
        desk_id: String,
        /// The refusal.
        #[source]
        source: tinyhivemind_driver::Error,
    },
    /// An ad-hoc room named too few teammates that could actually be seated.
    ///
    /// Only [`room_hive`] raises this. `desk_hives` skips such a desk without
    /// comment, because a desk of one is an ordinary company shape; a *room*
    /// of one is a caller who named nobody reachable, and it has to be told.
    #[error("room `{room_id}` seated {} of the teammates named, and a room needs two", bound.len())]
    TooFewSeats {
        /// The synthetic room id.
        room_id: String,
        /// The members that did bind, so the caller can say who was missing.
        bound: Vec<String>,
    },
}

/// Builds one hive per desk of two or more bound seats.
///
/// `bind` resolves a manifest agent id to its runtime handle; a member with
/// no handle (an overlay teammate the harness did not build, a retired one)
/// is left out of the graph rather than failing the desk, and a desk left
/// with fewer than two bound seats gets no hive at all — it answers through
/// one seat, as a desk of one always has. A desk tinyhivemind refuses is
/// reported and skipped, so one malformed desk cannot take the company's
/// other rooms down with it.
pub fn desk_hives(
    record: &CompanyRecord,
    roster_version: u64,
    bind: &dyn Fn(&str) -> Option<openhuman_embed::Agent>,
) -> (HashMap<String, Arc<DeskHive>>, Vec<HiveBuildError>) {
    let snapshots = crate::runtime::delegation_tools::tinyhivemind_desks(record);
    let desks = snapshots.set();
    let mut hives = HashMap::new();
    let mut errors = Vec::new();
    for desk in desks.iter() {
        if crate::server::chat_history::is_general_chat(Some(&desk.id)) {
            continue;
        }
        let Ok(members) = desks.members(&desk.id) else {
            continue;
        };
        let Seats {
            bindings,
            candidates,
            bound_members,
        } = bind_seats(record, members.into_iter(), bind);
        if bound_members.len() < 2 {
            continue;
        }
        let graph = HiveGraph::new(
            tinyhivemind::desk::Desk {
                id: desk.id.clone(),
                name: desk.name.clone(),
                description: desk.description.clone(),
                members: bound_members,
                responder_mode: desk.responder_mode.clone(),
            },
            candidates,
        );
        match BoundHive::new(graph, bindings) {
            Ok(hive) => {
                hives.insert(
                    desk.id.clone(),
                    Arc::new(DeskHive {
                        desk_id: desk.id.clone(),
                        desk_name: desk.name.clone(),
                        hive,
                        roster_version,
                    }),
                );
            }
            Err(source) => errors.push(HiveBuildError::Invalid {
                desk_id: desk.id.clone(),
                source,
            }),
        }
    }
    (hives, errors)
}

/// The three parallel lists a hive is built from, for one seat set.
struct Seats {
    bindings: Vec<AgentBinding<EmbedSeat>>,
    candidates: Vec<RouteCandidate>,
    bound_members: Vec<String>,
}

/// Resolves `members` to seats, dropping every id that is not a live roster
/// agent the binder can hand back a handle for.
///
/// Shared by [`desk_hives`] and [`room_hive`] so a manifest desk and an
/// ad-hoc room are seated by exactly the same rule: same roster check, same
/// `RouteCandidate` the router reads, same silent drop of a member that is
/// retired or was never built. A room assembled by a second, similar-looking
/// loop would be a second definition of who counts as a seat.
fn bind_seats<'a>(
    record: &CompanyRecord,
    members: impl Iterator<Item = &'a str>,
    bind: &dyn Fn(&str) -> Option<openhuman_embed::Agent>,
) -> Seats {
    let agents = record.effective_agents();
    let mut seats = Seats {
        bindings: Vec::new(),
        candidates: Vec::new(),
        bound_members: Vec::new(),
    };
    for member in members {
        if !record.is_roster_agent(member) {
            continue;
        }
        let Some(agent) = bind(member) else {
            continue;
        };
        let profile = agents.iter().find(|agent| agent.id == member);
        seats.candidates.push(RouteCandidate {
            id: member.to_string(),
            label: profile
                .and_then(|agent| agent.name.clone())
                .unwrap_or_else(|| member.to_string()),
            role: profile.map(|agent| agent.role.clone()),
            description: profile.and_then(|agent| agent.description.clone()),
            capabilities: Vec::new(),
            learned_topics: Vec::new(),
            available: true,
        });
        seats
            .bindings
            .push(AgentBinding::new(member, EmbedSeat(agent)));
        seats.bound_members.push(member.to_string());
    }
    seats
}

/// A hive over an arbitrary set of teammates, belonging to no desk.
///
/// A desk is a standing room: it is declared, it has a lead, an operator can
/// open it, and it persists. This is the other kind — a room convened for one
/// question and gone when the question is answered, whose members are whoever
/// the asking teammate named. `tinyhivemind::desk::Desk` never required a
/// manifest entry; it is a record of who is in a room, so a room assembled at
/// call time is as valid a one as a declared desk.
///
/// `room_id` is the synthetic desk id the episode is journaled under, and
/// `members` the seats in the order the caller named them — order is not
/// incidental, since the first bound member is the lead every route falls
/// back to when the router declines.
///
/// [`ResponderMode::Auto`] rather than `Lead`: an ad-hoc room has no standing
/// lead to prefer, and every seat in it was named because the asker wanted it
/// specifically. `Lead` would let the first name answer for all of them.
///
/// # Errors
///
/// [`HiveBuildError::Invalid`] when tinyhivemind refuses the graph, and when
/// fewer than two of `members` bind to a live seat — a room of one is not a
/// room, and the caller should be told rather than handed a hive that cannot
/// deliberate.
pub fn room_hive(
    record: &CompanyRecord,
    room_id: &str,
    room_name: &str,
    members: &[String],
    roster_version: u64,
    bind: &dyn Fn(&str) -> Option<openhuman_embed::Agent>,
) -> Result<Arc<DeskHive>, HiveBuildError> {
    let Seats {
        bindings,
        candidates,
        bound_members,
    } = bind_seats(record, members.iter().map(String::as_str), bind);
    if bound_members.len() < 2 {
        return Err(HiveBuildError::TooFewSeats {
            room_id: room_id.to_string(),
            bound: bound_members,
        });
    }
    let graph = HiveGraph::new(
        tinyhivemind::desk::Desk {
            id: room_id.to_string(),
            name: room_name.to_string(),
            description: None,
            members: bound_members,
            responder_mode: tinyhivemind::desk::ResponderMode::Auto,
        },
        candidates,
    );
    let hive = BoundHive::new(graph, bindings).map_err(|source| HiveBuildError::Invalid {
        desk_id: room_id.to_string(),
        source,
    })?;
    Ok(Arc::new(DeskHive {
        desk_id: room_id.to_string(),
        desk_name: room_name.to_string(),
        hive,
        roster_version,
    }))
}

#[cfg(test)]
#[path = "graph_tests.rs"]
mod tests;
