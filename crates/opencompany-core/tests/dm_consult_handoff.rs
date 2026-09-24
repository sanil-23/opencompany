#![cfg(feature = "openhuman")]
//! **End-to-end: a teammate using its colleagues from a direct message.**
//!
//! An operator talking to one teammate has always been one ordinary turn on
//! one responder. These two targets are what that teammate can now do with
//! the rest of the company from inside it.
//!
//! `consult_teammates` convenes the teammates it names in a room and waits:
//! they work the question through among themselves, and everything they said
//! comes back as the tool's result, in the same turn, before the teammate
//! answers the operator. The room is built for the question and belongs to no
//! desk, so who is in it is the caller's choice rather than a membership.
//!
//! `hand_off` gives the conversation away: the named teammate opens its own
//! direct channel with the operator, and the handing one stops.
//!
//! Both run against the scripted model in `support::script_model`, so what is
//! under test is the wiring and the journal rather than any model's judgement:
//! the script decides what each turn calls, and the assertions are about where
//! the rows landed and what the teammate was handed back.
//!
//! # What each test proves
//!
//! | Test | Claim |
//! | --- | --- |
//! | `a_consult_opens_a_room_and_hands_its_answer_back` | the question lands in a room of its own (never a desk), an episode opens and completes there, and the room's words reach the asking teammate in the same turn |
//! | `a_consult_naming_a_stranger_is_refused_and_convenes_nobody` | an id off the roster is a tool refusal that names who could have been brought in, and no room opens |
//! | `a_hand_off_opens_the_named_teammates_own_channel` | the hand-over note appears in the named teammate's channel at once, and its own reply follows there, without the operator sending anything |
//! | `a_hand_over_room_ends_even_if_nobody_closes_it` | the room closes on its own bound when no seat folds it, and the hand-off still stands |
//! | `a_hand_off_to_a_stranger_is_refused_and_opens_nothing` | an invented id is a tool refusal, and no channel is opened |
//!
//! The room itself is scripted to record and fold at once, so these prove the
//! seam rather than the deliberation. How a room deliberates is `hive_e2e`'s
//! subject.

mod support;

use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use support::script_model::{Ask, Reply, Responder, spawn_script};

use opencompany::CompanyRuntime;
use opencompany::company::CompanyManifest;
use opencompany::hive::tools::via_opencompany_mcp;
use opencompany::ports::types::{CompanyEvent, CompanyId, EventSeq, StoredEvent};
use opencompany::runtime::RuntimeBuilder;
use opencompany::{AppConfig, AppState};

// ---------------------------------------------------------------------------
// The company
// ---------------------------------------------------------------------------

const ADMIN: &str = "operator@opencompany.local";
const CEO: &str = "ceo";
const ENGINEER: &str = "engineer";
const WRITER: &str = "writer";
const ENGINEERING: &str = "engineering";
const CONTENT: &str = "content";

/// The operator's question, and the room's answer, kept distinct so an
/// assertion cannot pass on the wrong one.
const ASKED: &str = "Can we ship the checkout rollout on Friday?";
/// What a handed-to teammate says to the operator, in its own name.
const PICKED_UP: &str = "I have picked up the launch note and own it from here.";
const ROOM_SAID: &str = "Friday works if the freeze lifts on Thursday.";

/// Two desks sharing the CEO, so one test can convene a room and another can
/// find a teammate that sits on two and must be asked which.
///
/// `[policy] mode = "full"` so no tool call parks: these tests are about
/// where rows land, and a parked turn would hang rather than fail.
fn manifest(name: &str, base_url: &str) -> String {
    format!(
        r#"
[company]
name = "{name}"
summary = "Proves a teammate can use its colleagues from a direct message."

[inference]
provider = "ollama"
base_url = "{base_url}"

[inference.models]
chat-v1 = "llama3"
reasoning-v1 = "llama3"
agentic-v1 = "llama3"

[policy]
mode = "full"

[tools]
allow = []

[users]
admins = ["{ADMIN}"]

[[agent]]
id = "{CEO}"
role = "Chief Executive"
tier = "orchestrator"

[[agent]]
id = "{ENGINEER}"
role = "Engineer"

[[agent]]
id = "{WRITER}"
role = "Writer"

[[group_chat]]
id = "{ENGINEERING}"
name = "Engineering"
description = "How things are built."
members = ["{ENGINEER}", "{CEO}"]

[group_chat.routing]
round_width = 2

[[group_chat]]
id = "{CONTENT}"
name = "Content"
description = "Written drafts and copy."
members = ["{WRITER}", "{CEO}"]

[group_chat.routing]
round_width = 2
"#
    )
}

/// A company id no other test in this process uses: the runtime keeps one
/// agent per `(company, agent)` for the life of the process.
fn unique(prefix: &str) -> String {
    format!(
        "{prefix}-{}",
        uuid::Uuid::new_v4().simple().to_string()[..12].to_owned()
    )
}

// ---------------------------------------------------------------------------
// Booting, and talking to it
// ---------------------------------------------------------------------------

async fn boot(home: &Path, base_url: &str) -> (SocketAddr, Arc<CompanyRuntime>) {
    let id = unique("dm-lab");
    let mut manifest =
        CompanyManifest::from_stored_toml(&manifest(&id, base_url)).expect("the manifest parses");
    manifest.apply_globals();
    let problems = manifest.validate();
    assert!(problems.is_empty(), "the manifest is valid: {problems:?}");

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let company_id = CompanyId::new(&id);
    let state = AppState::new(AppConfig {
        bind: address.to_string(),
        ..AppConfig::default()
    })
    .with_home(home.to_path_buf());
    let runtime = Arc::new(
        RuntimeBuilder::new(state.home().to_path_buf(), manifest)
            .with_id(company_id.clone())
            // The pool is what fills the handle both tools reach back
            // through; without it neither is on anybody's belt.
            .with_harness(Arc::new(opencompany::harness::HarnessPool::new()))
            .build()
            .await
            .expect("the company builds"),
    );
    state
        .registry()
        .insert(company_id.clone(), Arc::clone(&runtime));
    tokio::spawn(async move {
        let _ = opencompany::server::serve_on(listener, state).await;
    });
    (address, runtime)
}

/// A cookie-carrying HTTP client — `reqwest`'s own cookie store is behind a
/// feature this crate does not enable.
struct Client {
    inner: reqwest::Client,
    base: String,
    cookie: std::sync::Mutex<Option<String>>,
}

impl Client {
    fn new(address: SocketAddr) -> Self {
        Self {
            inner: reqwest::Client::new(),
            base: format!("http://{address}"),
            cookie: std::sync::Mutex::new(None),
        }
    }

    async fn post(&self, path: &str, body: Value) -> (u16, Value) {
        let mut request = self.inner.post(format!("{}{path}", self.base)).json(&body);
        if let Some(cookie) = self.cookie.lock().unwrap().clone() {
            request = request.header(reqwest::header::COOKIE, cookie);
        }
        let response = request.send().await.expect("the loopback host answers");
        let status = response.status().as_u16();
        if let Some(set) = response.headers().get(reqwest::header::SET_COOKIE)
            && let Ok(value) = set.to_str()
            && let Some(pair) = value.split(';').next()
        {
            *self.cookie.lock().unwrap() = Some(pair.to_owned());
        }
        let text = response.text().await.unwrap_or_default();
        let json = serde_json::from_str(&text).unwrap_or(Value::String(text));
        (status, json)
    }

    /// Signs in over the loopback magic-link flow, which echoes the code.
    async fn sign_in(&self, email: &str) {
        let (status, body) = self
            .post("/api/v1/company/auth/request", json!({ "email": email }))
            .await;
        assert_eq!(status, 200, "sign-in refused: {body}");
        let code = body["dev_code"]
            .as_str()
            .unwrap_or_else(|| panic!("no dev_code, so no session: {body}"))
            .to_string();
        let (status, body) = self
            .post("/api/v1/company/auth/verify", json!({ "code": code }))
            .await;
        assert_eq!(status, 200, "the login code was refused: {body}");
    }

    /// Says something to one teammate in its own direct channel.
    async fn dm(&self, agent: &str, text: &str) -> Value {
        let (status, body) = self
            .post(
                "/api/v1/company/chat",
                json!({ "text": text, "chat": format!("dm:{agent}") }),
            )
            .await;
        assert_eq!(status, 200, "chat refused: {body}");
        body
    }
}

// ---------------------------------------------------------------------------
// Reading the journal
// ---------------------------------------------------------------------------

/// A generous bound: every turn here is a couple of loopback calls.
const SETTLE: Duration = Duration::from_secs(45);

async fn journal(runtime: &Arc<CompanyRuntime>) -> Vec<StoredEvent> {
    runtime
        .events()
        .read_from(runtime.id(), EventSeq::new(0), 4096)
        .await
        .expect("the journal reads")
}

/// Polls the journal until `done` holds, or fails after [`SETTLE`] saying
/// what it did see.
async fn wait_for(
    runtime: &Arc<CompanyRuntime>,
    what: &str,
    done: impl Fn(&[StoredEvent]) -> bool,
) -> Vec<StoredEvent> {
    let started = Instant::now();
    loop {
        let rows = journal(runtime).await;
        if done(&rows) {
            return rows;
        }
        assert!(
            started.elapsed() < SETTLE,
            "timed out waiting for {what}; journal kinds: {:?}",
            rows.iter().map(|row| row.event.kind()).collect::<Vec<_>>()
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Every agent reply on `chat`, as `(author, text)`, in order.
fn said(rows: &[StoredEvent], chat: &str) -> Vec<(String, String)> {
    rows.iter()
        .filter_map(|row| match &row.event {
            CompanyEvent::AgentReply {
                chat_id,
                agent_id,
                text,
                ..
            } if chat_id == chat => Some((agent_id.clone(), text.clone())),
            _ => None,
        })
        .collect()
}

/// Whether an episode both opened and completed on `desk`.
/// The ad-hoc room channel a consult opened, if it opened one.
///
/// Its id carries the episode's own uuid, so a test cannot predict it and has
/// to read it back — which is the point: the room is per-question, not a desk
/// anybody could have named in advance.
fn room_channel(rows: &[StoredEvent]) -> Option<String> {
    rows.iter().find_map(|row| match &row.event {
        CompanyEvent::EpisodeOpened { chat_id, .. } if chat_id.starts_with("room:") => {
            Some(chat_id.clone())
        }
        _ => None,
    })
}

fn episode_ran(rows: &[StoredEvent], desk: &str) -> bool {
    let opened = rows.iter().any(
        |row| matches!(&row.event, CompanyEvent::EpisodeOpened { chat_id, .. } if chat_id == desk),
    );
    let completed = rows.iter().any(|row| {
        matches!(&row.event, CompanyEvent::EpisodeCompleted { chat_id, .. } if chat_id == desk)
    });
    opened && completed
}

// ---------------------------------------------------------------------------
// The scripted teammate
// ---------------------------------------------------------------------------

/// The line the episode brief ends every seat turn with. Its presence is what
/// says a request is a seat of a running room rather than an ordinary turn.
const FENCE: &str = "Every tool call must carry \"chat\": \"";

fn role(message: &Value) -> &str {
    message.get("role").and_then(Value::as_str).unwrap_or("")
}

fn content(message: &Value) -> &str {
    message.get("content").and_then(Value::as_str).unwrap_or("")
}

/// The last thing the harness put in front of the model.
fn prompt(ask: &Ask) -> String {
    ask.messages
        .iter()
        .rev()
        .find(|message| role(message) == "user")
        .map(|message| content(message).to_string())
        .unwrap_or_default()
}

/// The desk a seat turn must name on every call, or `None` when this request
/// is not a seat turn at all.
fn seat_desk(ask: &Ask) -> Option<String> {
    let text = prompt(ask);
    let at = text.rfind(FENCE)?;
    text[at + FENCE.len()..]
        .split_once('"')
        .map(|(desk, _)| desk.to_owned())
}

/// Whether this turn has already made its one tool call.
///
/// Counted from the assistant's own messages rather than from tool results,
/// because a pooled teammate runs on a **text dialect**: its tool results
/// come back as `user` messages, so `tool_outputs` -- which reads `tool`
/// messages -- is empty on this path however the call went. An episode seat
/// is the opposite, native, which is why the room's own suite can read them.
///
/// Getting this wrong does not read as wrong: the script simply calls again,
/// and the harness correctly stops a turn that repeats one action without
/// making progress.
fn answered(ask: &Ask) -> bool {
    ask.messages
        .iter()
        .any(|message| role(message) == "assistant")
}

/// What the last tool call handed back, which on this path is the newest
/// `user` message.
fn tool_result(ask: &Ask) -> String {
    prompt(ask)
}

/// One of this crate's own tools, as a pooled teammate calls it.
fn oc_tool(tool: &str, arguments: Value) -> Reply {
    let (name, args) = via_opencompany_mcp(tool, arguments);
    assert_eq!(name, "mcp_call_tool", "this crate's tools are MCP-served");
    Reply::Call {
        tool: "mcp_call_tool",
        args,
    }
}

/// A room seat: record and be done, so the episode folds in one wave.
///
/// Deliberately the simplest seat there is. What a room does once it is
/// running -- who it turns next, how a private `ask` threads, when it folds
/// -- is the hive suite's subject and is covered there. What these tests are
/// for is the seam: that a teammate in a direct message can open one at all,
/// and that what was said comes back to it.
fn seat_records(desk: &str) -> Reply {
    Reply::Call {
        tool: "desk_complete_episode",
        args: json!({
            "message": ROOM_SAID,
            "chat": desk,
            "parent": Value::Null,
        }),
    }
}

/// A seat that has just been handed work: tell the operator whose
/// conversation it now is, then close the room.
///
/// `dm_operator` is the only route a seat has to the operator -- an episode
/// journals to the room, not to anybody's direct channel -- so a receiver that
/// skipped it would leave the operator hearing nothing at all.
fn handed_seat(ask: &Ask, desk: &str) -> Reply {
    if answered(ask) {
        return seat_records(desk);
    }
    Reply::Call {
        tool: "dm_operator",
        args: json!({ "message": PICKED_UP }),
    }
}

/// The teammate the operator is talking to: consult once, then answer with
/// what the room said.
///
/// Reads the transcript out of the tool result rather than repeating a
/// constant, so the assertion that the room's words reached it cannot pass on
/// a string the script already knew.
fn consulting_teammate(ask: &Ask) -> Reply {
    if answered(ask) {
        let heard = tool_result(ask);
        return Reply::Say(format!("I checked with the team. {heard}"));
    }
    oc_tool(
        "consult_teammates",
        json!({ "question": ASKED, "with": [ENGINEER, WRITER] }),
    )
}

// ---------------------------------------------------------------------------
// 1: a consult opens a room and hands its answer back
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_consult_opens_a_room_and_hands_its_answer_back() {
    let home = tempfile::tempdir().unwrap();
    let script: Responder = Arc::new(|ask: &Ask| {
        // A seat of the room the consult opened.
        if let Some(desk) = seat_desk(ask) {
            return seat_records(&desk);
        }
        // The engineer is the only teammate on exactly one room, so it needs
        // to name no desk.
        consulting_teammate(ask)
    });
    let (base_url, _script) = spawn_script(script).await;
    let (address, runtime) = boot(home.path(), &base_url).await;
    let client = Client::new(address);
    client.sign_in(ADMIN).await;

    client
        .dm(ENGINEER, "Ask the team whether Friday works.")
        .await;

    let rows = wait_for(&runtime, "the engineer to answer the operator", |rows| {
        said(rows, &format!("dm:{ENGINEER}"))
            .iter()
            .any(|(author, _)| author == ENGINEER)
    })
    .await;

    // The question is journaled in the room the consult convened, in the
    // asking teammate's own voice. **Not on a desk**: the room is built for
    // this question and belongs to nobody, so putting the row on `engineering`
    // would file a private consult in a standing channel an operator never
    // opened.
    let room = room_channel(&rows).expect("the consult opened a room");
    let said_in_room = said(&rows, &room);
    assert!(
        said_in_room
            .iter()
            .any(|(author, text)| author == ENGINEER && text == ASKED),
        "the consult's question is a row in its own room: {said_in_room:?}"
    );
    assert!(
        said(&rows, ENGINEERING).is_empty(),
        "and nothing was written to the desk: {:?}",
        said(&rows, ENGINEERING)
    );

    // And a real episode ran there: opened, and folded.
    assert!(
        episode_ran(&rows, &room),
        "an episode opened and completed on the desk: {:?}",
        rows.iter().map(|row| row.event.kind()).collect::<Vec<_>>()
    );

    // The room's words reached the asking teammate inside its own turn, which
    // is the whole point of the tool blocking.
    let answer = said(&rows, &format!("dm:{ENGINEER}"))
        .into_iter()
        .find(|(author, _)| author == ENGINEER)
        .map(|(_, text)| text)
        .expect("the engineer answered the operator");
    assert!(
        answer.contains(ROOM_SAID),
        "the operator's answer carries what the room said: {answer}"
    );
}

// ---------------------------------------------------------------------------
// 2: a teammate on two rooms is asked which
// ---------------------------------------------------------------------------

/// A consult naming somebody who is not on the roster is refused, and the
/// refusal names who *could* have been brought in — so the model can correct
/// it in the same turn rather than spending another one discovering the list.
///
/// Crucially no room opens. A room convened without the person the caller
/// meant answers in the wrong voice, and its answer reads exactly like a right
/// one, so guessing is worse than refusing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_consult_naming_a_stranger_is_refused_and_convenes_nobody() {
    let home = tempfile::tempdir().unwrap();
    let script: Responder = Arc::new(|ask: &Ask| {
        if let Some(desk) = seat_desk(ask) {
            return seat_records(&desk);
        }
        if answered(ask) {
            return Reply::Say(format!("I could not: {}", tool_result(ask)));
        }
        oc_tool(
            "consult_teammates",
            json!({ "question": ASKED, "with": ["marketing_lead"] }),
        )
    });
    let (base_url, _script) = spawn_script(script).await;
    let (address, runtime) = boot(home.path(), &base_url).await;
    let client = Client::new(address);
    client.sign_in(ADMIN).await;

    client.dm(CEO, "Ask the team whether Friday works.").await;

    let rows = wait_for(&runtime, "the CEO to answer the operator", |rows| {
        said(rows, &format!("dm:{CEO}"))
            .iter()
            .any(|(author, _)| author == CEO)
    })
    .await;

    let answer = said(&rows, &format!("dm:{CEO}"))
        .into_iter()
        .find(|(author, _)| author == CEO)
        .map(|(_, text)| text)
        .expect("the CEO answered");
    assert!(
        answer.contains(ENGINEER) && answer.contains(WRITER),
        "the refusal named who it could have brought in instead: {answer}"
    );
    assert!(
        room_channel(&rows).is_none(),
        "no room was convened on a guess: {:?}",
        rows.iter().map(|row| row.event.kind()).collect::<Vec<_>>()
    );
}

// ---------------------------------------------------------------------------
// 3: a hand-off opens the named teammate's own channel
// ---------------------------------------------------------------------------

/// The operator sends nothing to `dm:writer`, and the writer speaks there
/// anyway. That is the whole claim: a hand-off starts a conversation the
/// operator did not open, in the channel belonging to whoever now owns it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_hand_off_opens_the_named_teammates_own_channel() {
    let home = tempfile::tempdir().unwrap();
    let script: Responder = Arc::new(|ask: &Ask| {
        // The hand-off runs as an episode, so the teammate picking it up is
        // a SEAT: it reads the ask out of the room the two share and reaches
        // the operator with `dm_operator`, the only route a seat has to them.
        if let Some(desk) = seat_desk(ask) {
            return handed_seat(ask, &desk);
        }
        if answered(ask) {
            return Reply::Say(format!("Handed to @{WRITER}, who owns the copy."));
        }
        oc_tool(
            "hand_off",
            json!({
                "to": WRITER,
                "brief": "The operator wants the launch note rewritten.",
            }),
        )
    });
    let (base_url, _script) = spawn_script(script).await;
    let (address, runtime) = boot(home.path(), &base_url).await;
    let client = Client::new(address);
    client.sign_in(ADMIN).await;

    client.dm(ENGINEER, "Can you redo the launch note?").await;

    // **The bare id, not `dm:<id>`.** A DM is addressed as `dm:<teammate>` but
    // stored under the teammate's own id — that is what the chat route
    // normalizes to, and what the console reads back. Journaling the hand-over
    // under the addressed spelling put it in a channel nothing displayed: the
    // teammate ran, did the work and replied, and the operator's console
    // showed an empty conversation.
    // Waits for the room to **close**, not merely for the writer to speak.
    // The bound is the point: two teammates transferring a task have nothing
    // to converge on, so a room on a desk's defaults just runs. Asserting only
    // that the writer answered would pass with the room still open.
    let pair = opencompany::hive::referral::pair_conversation(ENGINEER, WRITER);
    let rows = wait_for(&runtime, "the hand-over room to finish", |rows| {
        episode_ran(rows, &pair)
            && said(rows, WRITER)
                .iter()
                .any(|(author, _)| author == WRITER)
    })
    .await;

    let opened = said(&rows, WRITER);

    // **The operator hears from the teammate that now owns the work, and from
    // nobody else.** The hand-over itself is one teammate speaking to another,
    // so it belongs in their pair channel, not in the operator's conversation
    // — a direct message should read as its own teammate's voice, and what a
    // hand-off owes the operator is the new owner's opening rather than a
    // transcript of two agents arranging it.
    assert_eq!(
        opened.len(),
        1,
        "exactly one row, from the teammate picking it up: {opened:?}"
    );
    assert_eq!(opened[0].0, WRITER, "authored by the one picking it up");
    assert_eq!(opened[0].1, PICKED_UP, "in its own words");

    // And the hand-over is where a2a traffic goes: the pair channel both
    // teammates share, the same `dm:<a>+<b>` an episode's `ask` routes to.
    let handed_over = said(&rows, &pair);
    assert!(
        handed_over
            .iter()
            .any(|(author, text)| author == ENGINEER && text.contains("launch note")),
        "the ask reached the writer agent-to-agent, in the handing teammate's own \
         voice: {handed_over:?}"
    );

    // And it was a real episode there, not a row written to look like one.
    assert!(
        episode_ran(&rows, &pair),
        "the hand-over ran as an episode in the pair channel: {:?}",
        rows.iter().map(|row| row.event.kind()).collect::<Vec<_>>()
    );

    // The handing teammate said so in its own channel, and that is the only
    // row the hand-off itself produced there.
    let handed = said(&rows, &format!("dm:{ENGINEER}"));
    assert!(
        handed
            .iter()
            .any(|(author, text)| author == ENGINEER && text.contains(WRITER)),
        "the engineer told the operator who it handed to: {handed:?}"
    );
}

/// **The room ends whether or not anybody ends it.**
///
/// A hand-over is two teammates transferring a task, and two teammates have
/// nothing to converge on -- so nothing in the exchange naturally produces a
/// `complete_episode`. On a desk's routing defaults (12 rounds at width 5)
/// that room simply runs: observed live, seven minutes and four turns with no
/// end in sight, every one of them paid for.
///
/// So the seats here never fold. The claim is that the room closes anyway, on
/// its own bound, and that the hand-off still stands -- the ask is in the pair
/// channel and the receiver has spoken to the operator, both of which survive
/// the room ending. Reaching the cap is the design, not a failure.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_hand_over_room_ends_even_if_nobody_closes_it() {
    let home = tempfile::tempdir().unwrap();
    let script: Responder = Arc::new(|ask: &Ask| {
        if seat_desk(ask).is_some() {
            // A seat that talks and never folds. The first turn tells the
            // operator, so the hand-off is observably done; every later turn
            // keeps the conversation going, which is what a bound has to stop.
            if answered(ask) {
                return Reply::Say("Still thinking about it.".to_owned());
            }
            return Reply::Call {
                tool: "dm_operator",
                args: json!({ "message": PICKED_UP }),
            };
        }
        if answered(ask) {
            return Reply::Say(format!("Handed to @{WRITER}."));
        }
        oc_tool(
            "hand_off",
            json!({
                "to": WRITER,
                "brief": "The operator wants the launch note rewritten.",
            }),
        )
    });
    let (base_url, _script) = spawn_script(script).await;
    let (address, runtime) = boot(home.path(), &base_url).await;
    let client = Client::new(address);
    client.sign_in(ADMIN).await;

    client.dm(ENGINEER, "Can you redo the launch note?").await;

    let pair = opencompany::hive::referral::pair_conversation(ENGINEER, WRITER);
    let rows = wait_for(
        &runtime,
        "the unfolded room to close on its bound",
        |rows| episode_ran(rows, &pair),
    )
    .await;

    // And the hand-off stands: the operator heard from the teammate that took
    // it on, in its own channel, whatever the room did afterwards.
    let opened = said(&rows, WRITER);
    assert!(
        opened
            .iter()
            .any(|(author, text)| author == WRITER && text == PICKED_UP),
        "the writer still took it on and said so: {opened:?}"
    );
}

// ---------------------------------------------------------------------------
// 5: handing to somebody who does not exist opens nothing
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_hand_off_to_a_stranger_is_refused_and_opens_nothing() {
    let home = tempfile::tempdir().unwrap();
    let script: Responder = Arc::new(|ask: &Ask| {
        if let Some(desk) = seat_desk(ask) {
            return seat_records(&desk);
        }
        if answered(ask) {
            return Reply::Say(format!("I could not hand it on: {}", tool_result(ask)));
        }
        oc_tool(
            "hand_off",
            json!({ "to": "designer", "brief": "Rewrite the launch note." }),
        )
    });
    let (base_url, _script) = spawn_script(script).await;
    let (address, runtime) = boot(home.path(), &base_url).await;
    let client = Client::new(address);
    client.sign_in(ADMIN).await;

    client.dm(ENGINEER, "Can you redo the launch note?").await;

    let rows = wait_for(&runtime, "the engineer to answer", |rows| {
        said(rows, &format!("dm:{ENGINEER}"))
            .iter()
            .any(|(author, _)| author == ENGINEER)
    })
    .await;

    let answer = said(&rows, &format!("dm:{ENGINEER}"))
        .into_iter()
        .find(|(author, _)| author == ENGINEER)
        .map(|(_, text)| text)
        .expect("the engineer answered");
    assert!(
        answer.contains(WRITER) && answer.contains(CEO),
        "the refusal named who it could have handed to: {answer}"
    );
    // Nobody was opened: an invented id must not start a conversation with
    // the operator under somebody else's name.
    assert!(
        said(&rows, "dm:designer").is_empty(),
        "no channel was opened for a teammate that does not exist"
    );
}

// ---------------------------------------------------------------------------
// 5: the tools are actually on a teammate's belt
// ---------------------------------------------------------------------------

/// The cheapest and most load-bearing assertion here, and the one whose
/// absence cost a live run to notice.
///
/// Every test above drives the tools through a *scripted* model, which calls
/// whatever the script says whether or not the teammate was offered it. So
/// they prove the tools work and say nothing about whether a real model is
/// ever shown them. A live run then answered with no tool calls at all, and
/// there was no way to tell "the model chose not to" from "the model never
/// saw them".
///
/// This asks the roster directly, costs no model call, and fails loudly if
/// the belt ever stops carrying them — a broken handle, a missed feature
/// gate, a deps built without a pool.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_teammate_is_offered_both_tools() {
    let home = tempfile::tempdir().unwrap();
    let (base_url, _script) = spawn_script(Arc::new(|_: &Ask| Reply::Say("ok".to_owned()))).await;
    let (address, runtime) = boot(home.path(), &base_url).await;
    let client = Client::new(address);
    client.sign_in(ADMIN).await;

    // The roster is built on first use, not at boot, so somebody has to say
    // something before there is a belt to look at.
    client.dm(ENGINEER, "Morning.").await;
    wait_for(&runtime, "the engineer to have run once", |rows| {
        said(rows, &format!("dm:{ENGINEER}"))
            .iter()
            .any(|(author, _)| author == ENGINEER)
    })
    .await;

    let pool = runtime
        .harness()
        .expect("the company was built with a pool");
    let agent = pool
        .agent(runtime.id(), ENGINEER)
        .await
        .expect("the engineer is on the roster");
    let belt = agent.tool_names();

    assert!(
        belt.iter().any(|name| name == "consult_teammates"),
        "a teammate can convene the colleagues it names: {belt:?}"
    );
    assert!(
        belt.iter().any(|name| name == "hand_off"),
        "a teammate can hand the conversation on: {belt:?}"
    );
}
