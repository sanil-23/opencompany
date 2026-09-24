pub(super) use super::*;

/// The company shape issue #272 was observed on: real desks, plus a
/// teammate (`writer`) the orchestrator mistook for one.
pub(super) fn record() -> CompanyRecord {
    let manifest = toml::from_str(
        r#"
[company]
name = "Acme"

[[agent]]
id = "ceo"
role = "Chief Executive"

[[agent]]
id = "writer"
role = "Writer"

[[group_chat]]
id = "engineering"
name = "Engineering desk"
members = ["ceo"]

[[group_chat]]
id = "content"
name = "Content desk"
members = ["writer"]

[[group_chat]]
id = "legal"
name = "Legal desk"
members = ["counsel"]
"#,
    )
    .expect("valid manifest");
    CompanyRecord {
        overlay_desk_hive: Vec::new(),
        overlay_retired_agents: Vec::new(),
        overlay_agent_edits: Vec::new(),
        id: crate::ports::types::CompanyId::new("acme"),
        manifest,
        ledger: Vec::new(),
        lifecycle: "running".to_string(),
        overlay_agents: Vec::new(),
        overlay_desk_members: Vec::new(),
        overlay_desk_order: Vec::new(),
        overlay_desks: Vec::new(),
        overlay_workflows: Vec::new(),
        overlay_budgets: Vec::new(),
        overlay_policy: None,
        overlay_desk_tools: Default::default(),
        disabled_workflows: Vec::new(),
        template_provenance: None,
        setup: None,
        activation_completed_at: None,
        created_at_millis: None,
        name_confirmed: false,
        overlay_tool_grants: Default::default(),
    }
}

// --- Teammate hand-off (issue #884) ------------------------------------

/// The one-member-per-desk shape, reachable from a test that has shadowed
/// [`record`] with a binding of its own.
pub(super) fn solo_record() -> CompanyRecord {
    record()
}

/// The company shape #884 D1 was observed on: one desk with THREE members,
/// so its lead has peers it could not previously reach, plus a second desk
/// nobody on the first sits on.
pub(super) fn desk_record() -> CompanyRecord {
    let manifest = toml::from_str(
        r#"
[company]
name = "Acme"

[[agent]]
id = "chief"
role = "Chief of Staff"
tier = "orchestrator"

[[agent]]
id = "brand_strategist"
role = "Brand Strategist"

[[agent]]
id = "seo_specialist"
role = "SEO Specialist"

[[agent]]
id = "copywriter"
role = "Copywriter"

[[agent]]
id = "analyst"
role = "Analyst"

[[group_chat]]
id = "strategy"
name = "Strategy desk"
members = ["brand_strategist", "seo_specialist", "copywriter"]

[[group_chat]]
id = "data"
name = "Data desk"
members = ["analyst"]
"#,
    )
    .expect("valid manifest");
    CompanyRecord {
        manifest,
        ..record()
    }
}

/// A desk is not a direct message however it is addressed, and a teammate is
/// one whether addressed bare or through the console's `dm:` key.
///
/// The distinction decides whether a mention may redirect the conversation:
/// on a desk it picks which member answers, in a DM it is a reference to
/// somebody who is not in the room.
#[test]
fn a_direct_message_is_a_teammate_and_a_desk_is_not() {
    let record = record();

    for chat in ["writer", "dm:writer", "ceo", "dm:ceo"] {
        assert!(
            is_direct_message(&record, chat),
            "`{chat}` addresses one teammate"
        );
    }
    for chat in ["engineering", "main", "general"] {
        assert!(
            !is_direct_message(&record, chat),
            "`{chat}` is a desk or the company's own line, not a direct message"
        );
    }
    // A key naming nobody is not a DM either: it resolves to no teammate, so
    // there is no single counterpart for a mention to be redundant with.
    assert!(!is_direct_message(&record, "marketing"));
}
