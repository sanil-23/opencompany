//! The **team** section of every agent's system prompt: who else is at the
//! company, which desks they sit on, and who this agent may hand work to.
//!
//! # Why every agent gets one
//!
//! Before this section existed, a non-orchestrator agent was told who *it* was
//! and nothing about anybody else. The orchestrator could learn the roster with
//! `query_company`; a desk lead or a specialist had no such tool, no roster in
//! its prompt, and — unless its manifest entry opted in with `delegates_to` —
//! no hand-off tool either. Asked for something a teammate owned, it did the
//! only thing its briefing allowed: declined, guessed, or said it "could not
//! contact" a colleague sitting on the same desk. The model was not wrong about
//! its situation; the situation was wrong.
//!
//! So the roster is rendered into every agent's prompt, statically, beside the
//! tools that act on it. It is a listing of ids, roles and mandates — what a
//! new hire is told on day one — not a live status board; what a teammate is
//! doing right now is a question for the board tools, not for this section.
//!
//! # Where it sits
//!
//! Appended after the persona and the bundle documents and before the tool
//! briefs, on the same cache-stability argument the rest of the prompt follows
//! (`docs/spec/runtime/agents.md`): the roster changes when an operator adds or
//! removes a teammate, which is exactly when the belt is rebuilt anyway, so it
//! is as static as the briefs behind it.
//!
//! Always compiled, like [`prompt`](crate::company::prompt): `opencompany prompt`
//! renders this section from a manifest alone, so what an operator reads in the
//! dump is what the agent is briefed with.

use crate::ports::types::CompanyRecord;
use crate::runtime::delegation_tools::{desk_lead, desks_of_member};

/// The heading the section opens with. Named so the tool descriptions and the
/// orchestrator brief can point at it ("as listed under Your team").
pub const TEAM_HEADING: &str = "## Your team";

/// The team section for `agent_id`, or `""` when it is the only agent at the
/// company — a roster of one has nobody to hand work to, and a heading over an
/// empty list would read as a team that exists and says nothing.
///
/// Lists every *other* roster teammate (manifest agents in declaration order,
/// then operator-added ones; removed ones are not on the effective roster and
/// so not here), then every desk with its members and lead, then which of
/// those this agent sits on and which it may hand work to.
///
/// The reach line is rendered from the same rule the tools enforce at call
/// time ([`teammate_targets`]), so the prompt never names a teammate the tool
/// would then refuse. With an unrestricted reach — the ordinary case, a
/// manifest entry that says nothing — the line says so in one clause rather
/// than repeating the roster.
pub fn team_section(record: &CompanyRecord, agent_id: &str) -> String {
    let manifest_roster = record.effective_agents();
    // The manifest roster, then the operator-added teammates — the same order
    // `roster_agent_ids` (and every refusal message) uses, so the listing and
    // the tools agree about who comes first. An overlay teammate has no
    // manifest row; it carries its display name, its role and its mandate,
    // which is all this section needs of anybody.
    let roster: Vec<Teammate<'_>> = manifest_roster
        .iter()
        .map(|agent| Teammate {
            id: &agent.id,
            name: agent.name.as_deref(),
            role: &agent.role,
            description: agent.description.as_deref(),
        })
        .chain(
            record
                .overlay_agents
                .iter()
                .filter(|overlay| !manifest_roster.iter().any(|agent| agent.id == overlay.id))
                .map(|overlay| Teammate {
                    id: &overlay.id,
                    name: Some(&overlay.name),
                    role: &overlay.role,
                    description: overlay.description.as_deref(),
                }),
        )
        .collect();
    let others: Vec<&Teammate<'_>> = roster.iter().filter(|agent| agent.id != agent_id).collect();
    if others.is_empty() {
        return String::new();
    }
    let orchestrator = crate::company::orchestrator_id(&manifest_roster).map(str::to_string);
    let company = record.manifest.company.name.trim();
    // **No reach narrowing.** This section used to compute a `delegates_to`
    // reach and, when it was narrower than the roster, end by naming the
    // subset a teammate "may bring in".
    //
    // `delegates_to` bounds DELEGATION — who may be handed a slice of this
    // teammate's own work, capped at depth 2 (issue #884). The two tools this
    // section is actually about do not delegate. `consult_teammates` convenes
    // a room that talks, and `hand_off` gives the conversation away; both now
    // reach the whole roster, so a reach line drawn from `delegates_to` states
    // a bound neither tool enforces. Run live, that mismatch cost a product
    // manager its turn: told it could bring in only its desk-mate, it went
    // looking for another way round.
    //
    // The cost bound is not here and never was. `MAX_ROOM` caps a room's
    // seats, and `dm_reach_brief` tells the model a room is the most expensive
    // thing it can do.

    let mut out = String::new();
    out.push_str("\n\n");
    out.push_str(TEAM_HEADING);
    out.push_str(&format!(
        "\n\nYou are one of {} teammates at {company}, and you are not working alone. ",
        roster.len(),
    ));
    // **No tool named here any more.** This section used to end by pointing at
    // `delegate_to_teammate`, which a teammate no longer carries: bringing
    // colleagues in is `consult_teammates`, giving the conversation away is
    // `hand_off`, and both are described where the choice between them is —
    // `dm_reach`. Naming a third, removed tool here would teach a call that
    // refuses, and naming one of those two would split their explanation
    // across two places that then drift.
    //
    // What this section is for stays exactly what it was: knowing who is here
    // and what they do, so a teammate never says somebody is out of reach.
    out.push_str(
        "Every teammate below is a real agent you can bring in, and they answer in this same \
         turn. Never tell anyone a teammate is out of reach or that you cannot contact \
         them — you can.",
    );
    out.push_str("\n\nTeammates (roster id — role: mandate), named exactly as written:\n");
    for agent in &others {
        out.push_str("- `");
        out.push_str(agent.id);
        out.push_str("` — ");
        match agent.name.map(str::trim).filter(|n| !n.is_empty()) {
            Some(name) if !name.eq_ignore_ascii_case(agent.role.trim()) => {
                out.push_str(name);
                out.push_str(", ");
                out.push_str(agent.role.trim());
            }
            _ => out.push_str(agent.role.trim()),
        }
        if orchestrator.as_deref() == Some(agent.id) {
            out.push_str(
                " (the orchestrator: the operator's point of contact, who can bring anyone in \
                 and owns the board)",
            );
        }
        if let Some(description) = agent.description.map(str::trim)
            && !description.is_empty()
        {
            out.push_str(": ");
            out.push_str(description);
        }
        out.push('\n');
    }

    // **The desks this agent sits on, not every desk the company has.**
    //
    // A seat acts where it sits. The full org chart was the whole of this
    // section on a large roster, listing membership and a lead for rooms this
    // agent will never take a turn in — and in a hive turn it arrives beside
    // `EpisodePrompt::peers`, which lists the desks it may actually ask,
    // filtered by the referral policy. The same desk was therefore described
    // twice in one turn, once as somewhere to hand work whose lead answers and
    // once as somewhere to put a question that the room answers, which are
    // different mechanisms with different costs (#2368).
    //
    // What is deliberately NOT filtered is the roster above: knowing who does
    // what is how a seat knows who is worth asking, and hiding that is the
    // failure this whole section exists to fix — "declined, guessed, or said it
    // could not contact a colleague sitting on the same desk".
    let desks = desks_of_member(record, agent_id);
    if !desks.is_empty() {
        out.push_str("\nYou sit on (desk id — name: members):\n");
        for desk in &desks {
            let lead = desk_lead(record, desk);
            let members: Vec<String> = record
                .effective_desk_members(desk)
                .into_iter()
                .filter(|member| record.is_roster_agent(member))
                .map(|member| match lead.as_deref() == Some(member.as_str()) {
                    true => format!("{member} (lead)"),
                    false => member,
                })
                .collect();
            out.push_str("- `");
            out.push_str(desk);
            out.push_str("` — ");
            out.push_str(&desk_label(record, desk));
            out.push_str(": ");
            out.push_str(match members.is_empty() {
                true => "nobody on the roster yet",
                false => "",
            });
            out.push_str(&members.join(", "));
            out.push('\n');
        }
    }
    out
}

/// One roster entry as this section renders it — the four things a new hire
/// is told about a colleague, whichever of the two roster halves they are on.
struct Teammate<'a> {
    id: &'a str,
    name: Option<&'a str>,
    role: &'a str,
    description: Option<&'a str>,
}

/// A desk's operator-facing name, or its id when it has none to show.
fn desk_label(record: &CompanyRecord, desk_id: &str) -> String {
    record
        .manifest
        .group_chats
        .iter()
        .find(|chat| chat.id == desk_id)
        .map(|chat| chat.name.clone())
        .or_else(|| {
            record
                .overlay_desks
                .iter()
                .find(|desk| desk.id == desk_id)
                .map(|desk| desk.name.clone())
        })
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| desk_id.to_string())
}

#[cfg(test)]
#[path = "team_brief_tests.rs"]
mod tests;
