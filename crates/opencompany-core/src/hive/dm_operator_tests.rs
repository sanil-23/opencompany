//! What the tool writes, and the one distinction it has to keep sharp.

use super::*;
use crate::hive::test_support::MemoryLog;
use crate::ports::types::{CompanyEvent, EventSeq};

/// The row lands in the seat's **own** conversation, under the bare roster id
/// — the spelling the chat route normalizes to and the console reads back. A
/// `dm:<id>` row is filed where nothing displays it, which is the mistake
/// `hand_off` already made once.
#[tokio::test]
async fn it_writes_to_the_seats_own_channel_under_the_bare_id() {
    let events: Arc<dyn EventLog> = Arc::new(MemoryLog::default());
    let company = CompanyId::new("acme");
    let tool = DmOperatorTool::new(company.clone(), "qa_engineer", events.clone());

    let result = tool
        .execute(serde_json::json!({ "message": "Backend is still in progress." }))
        .await
        .expect("the tool ran");
    assert!(!result.is_error, "{result:?}");

    let rows = events
        .read_from(&company, EventSeq::new(0), 16)
        .await
        .unwrap();
    let said: Vec<_> = rows
        .iter()
        .filter_map(|row| match &row.event {
            CompanyEvent::AgentReply {
                chat_id,
                agent_id,
                text,
                ..
            } => Some((chat_id.as_str(), agent_id.as_str(), text.as_str())),
            _ => None,
        })
        .collect();
    assert_eq!(
        said,
        vec![(
            "qa_engineer",
            "qa_engineer",
            "Backend is still in progress."
        )],
        "one row, in and from the seat's own channel"
    );
}

/// An empty message is a refusal the model can correct in the same turn, not a
/// blank row in the operator's conversation.
#[tokio::test]
async fn an_empty_message_is_refused_and_writes_nothing() {
    let events: Arc<dyn EventLog> = Arc::new(MemoryLog::default());
    let company = CompanyId::new("acme");
    let tool = DmOperatorTool::new(company.clone(), "qa_engineer", events.clone());

    for args in [
        serde_json::json!({}),
        serde_json::json!({ "message": "   " }),
    ] {
        let result = tool.execute(args).await.expect("the tool ran");
        assert!(result.is_error, "{result:?}");
    }
    let rows = events
        .read_from(&company, EventSeq::new(0), 16)
        .await
        .unwrap();
    assert!(rows.is_empty(), "nothing was written: {rows:?}");
}

/// The brief's whole job is the choice between this and escalating, so it has
/// to say both halves: that nothing waits, and what to use when something must.
#[test]
fn the_brief_separates_telling_from_waiting() {
    let brief = dm_operator_brief();
    assert!(brief.contains(&format!("`{DM_OPERATOR_TOOL}`")), "{brief}");
    assert!(brief.contains("nothing waits"), "{brief}");
    assert!(
        brief.contains("escalate instead"),
        "the blocking alternative has to be named: {brief}"
    );
    assert!(
        brief.contains("ask them"),
        "and a question for a teammate must not go to the operator: {brief}"
    );
}
