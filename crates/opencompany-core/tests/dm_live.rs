#![cfg(feature = "openhuman")]
//! **Live: does a real model actually reach for these tools?**
//!
//! Everything else about `consult_desk` and `hand_off` is covered against a
//! scripted model, which proves the wiring and proves nothing about the one
//! thing a script cannot: whether a model reading the tool's own description,
//! in a real company, with a real question, decides to use it.
//!
//! So this target is ignored by default. It spends real model calls against a
//! real provider and must never run in CI or by accident. Run it deliberately:
//!
//! ```text
//! set -a; . ~/.config/tinyhivemind/live.env; set +a
//! cargo test -p opencompany-core --features openhuman --test dm_live -- --ignored --nocapture
//! ```
//!
//! It reads `OPENROUTER_API_KEY` and `OPENROUTER_MODEL` from the environment
//! and skips — rather than fails — when they are absent, because a missing
//! key is "not asked for", not "broken".
//!
//! # What it prints, and why that is the point
//!
//! It asserts almost nothing. A model is free to answer a question without
//! consulting anybody, and that is not a bug. What it does is print the whole
//! exchange — every tool the teammate called, and every row that reached the
//! journal — so a person can read whether the behaviour was sensible. The one
//! thing it does assert is that the turn completed and the journal is
//! coherent, because those are ours and not the model's.

mod support;

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use opencompany::CompanyRuntime;
use opencompany::company::CompanyManifest;
use opencompany::company::credentials::Credential;
use opencompany::harness::provider::HostedProviderConfig;
use opencompany::ports::types::{CompanyEvent, CompanyId, EventSeq, StoredEvent};
use opencompany::runtime::RuntimeBuilder;
use opencompany::{AppConfig, AppState};

const ADMIN: &str = "operator@opencompany.local";
const ENGINEER: &str = "engineer";

/// The credentials a live run needs, or `None` when this was not asked for.
fn live() -> Option<(String, String)> {
    let key = std::env::var("OPENROUTER_API_KEY").ok()?;
    let model = std::env::var("OPENROUTER_MODEL").ok()?;
    (!key.trim().is_empty() && !model.trim().is_empty()).then_some((key, model))
}

/// A small real company: two teammates on one desk, so a consult has somebody
/// to consult and a hand-off somebody to hand to.
fn manifest(name: &str) -> String {
    format!(
        r#"
[company]
name = "{name}"
summary = "A live company for reading what a real model does with its colleagues."

[policy]
mode = "full"

[tools]
allow = []

[users]
admins = ["{ADMIN}"]

[[agent]]
id = "ceo"
role = "Chief Executive"
tier = "orchestrator"

[[agent]]
id = "{ENGINEER}"
role = "Engineer"
instructions = "You own how things are built. You are talking to the operator directly."

[[agent]]
id = "writer"
role = "Writer"
instructions = "You own the words the company publishes: release notes, copy, announcements."

[[group_chat]]
id = "engineering"
name = "Engineering"
description = "How things are built, shipped and operated."
members = ["{ENGINEER}", "ceo"]

[group_chat.routing]
round_width = 2
"#
    )
}

async fn boot(
    home: &std::path::Path,
    key: String,
    model: String,
) -> (SocketAddr, Arc<CompanyRuntime>) {
    let id = format!(
        "dm-live-{}",
        uuid::Uuid::new_v4().simple().to_string()[..12].to_owned()
    );
    let mut manifest = CompanyManifest::from_stored_toml(&manifest(&id)).expect("manifest parses");
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
            .with_harness(Arc::new(opencompany::harness::HarnessPool::new()))
            // The platform-injected route, pointed at OpenRouter. The key is
            // held in memory for this process and never journaled, printed or
            // written to the company.
            .with_harness_inference(
                HostedProviderConfig {
                    base_url: "https://openrouter.ai/api/v1".to_owned(),
                    credential: Credential::from_value(key),
                    extra_headers: vec![("X-Title".to_owned(), "opencompany dm-live".to_owned())],
                },
                Some(model),
            )
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

    async fn sign_in(&self, email: &str) {
        let (status, body) = self
            .post("/api/v1/company/auth/request", json!({ "email": email }))
            .await;
        assert_eq!(status, 200, "sign-in refused: {body}");
        let code = body["dev_code"].as_str().expect("a dev code").to_string();
        let (status, body) = self
            .post("/api/v1/company/auth/verify", json!({ "code": code }))
            .await;
        assert_eq!(status, 200, "the login code was refused: {body}");
    }

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

async fn journal(runtime: &Arc<CompanyRuntime>) -> Vec<StoredEvent> {
    runtime
        .events()
        .read_from(runtime.id(), EventSeq::new(0), 4096)
        .await
        .expect("the journal reads")
}

/// Prints the exchange for a person to read, and returns the rows.
///
/// This is the output that matters. The assertions below only check that the
/// machinery held; whether the model behaved sensibly is a judgement, and the
/// transcript is what it is made on.
async fn transcript(runtime: &Arc<CompanyRuntime>, waited: Duration) -> Vec<StoredEvent> {
    let rows = journal(runtime).await;
    eprintln!(
        "\n──────── live exchange ({:.1}s) ────────",
        waited.as_secs_f64()
    );
    for row in &rows {
        match &row.event {
            CompanyEvent::OperatorMessage { chat, text, .. } => {
                eprintln!("[{}] operator: {text}", chat.as_deref().unwrap_or("-"));
            }
            CompanyEvent::AgentReply {
                chat_id,
                agent_id,
                text,
                ..
            } => {
                eprintln!("[{chat_id}] @{agent_id}: {text}");
            }
            CompanyEvent::EpisodeOpened {
                chat_id,
                participants,
                ..
            } => eprintln!("[{chat_id}] — a room opened with {participants:?}"),
            CompanyEvent::EpisodeCompleted {
                chat_id, rounds, ..
            } => {
                eprintln!("[{chat_id}] — the room closed after {rounds} round(s)");
            }
            CompanyEvent::ConversationOpened { asker, askee, .. } => {
                eprintln!("      — @{asker} opened a private conversation with @{askee}");
            }
            _ => {}
        }
    }
    eprintln!("────────────────────────────────────────\n");
    rows
}

/// Waits until the named teammate has answered in its own channel, or gives
/// up. A live turn is slower than a scripted one and a room is several.
async fn wait_for_reply(runtime: &Arc<CompanyRuntime>, agent: &str, within: Duration) -> Duration {
    let started = Instant::now();
    let channel = format!("dm:{agent}");
    loop {
        let answered = journal(runtime).await.iter().any(|row| {
            matches!(
                &row.event,
                CompanyEvent::AgentReply { chat_id, agent_id, .. }
                    if chat_id == &channel && agent_id == agent
            )
        });
        if answered || started.elapsed() >= within {
            return started.elapsed();
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

/// How long a live exchange is given before the test stops waiting. A room is
/// several model calls in sequence, and a real provider is not a loopback.
const LIVE_BUDGET: Duration = Duration::from_secs(180);

/// A question one teammate cannot answer alone, put to it in its own channel.
///
/// Deliberately not "ask the team": the tool is worth having only if a model
/// reaches for it because the question needs the room, not because it was
/// told to. If it answers alone, that is a finding about the description, and
/// the transcript is where it is read.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "spends real model calls; run deliberately with --ignored"]
async fn a_real_model_answering_an_operator_can_reach_its_desk() {
    let Some((key, model)) = live() else {
        eprintln!("no OPENROUTER_API_KEY / OPENROUTER_MODEL in the environment — not asked for");
        return;
    };
    eprintln!("live run against {model}");

    let home = tempfile::tempdir().unwrap();
    let (address, runtime) = boot(home.path(), key, model).await;
    let client = Client::new(address);
    client.sign_in(ADMIN).await;

    client
        .dm(
            ENGINEER,
            "We want to ship the checkout rollout on Friday. Before you answer, \
             get the rest of your desk's view on whether that is realistic — I \
             want their reasoning, not just yours.",
        )
        .await;

    let waited = wait_for_reply(&runtime, ENGINEER, LIVE_BUDGET).await;
    let rows = transcript(&runtime, waited).await;

    // What is ours to guarantee: the teammate answered its operator, and the
    // journal is coherent. Whether it consulted is the model's call, and the
    // transcript above is where that is judged.
    let answered = rows.iter().any(|row| {
        matches!(
            &row.event,
            CompanyEvent::AgentReply { chat_id, agent_id, .. }
                if chat_id == &format!("dm:{ENGINEER}") && agent_id == ENGINEER
        )
    });
    assert!(
        answered,
        "the teammate answered the operator within the budget"
    );

    // Every episode that opened also closed. A live run is the one place an
    // episode can stall for real, and an open-forever room is the failure
    // that would look like success in a transcript.
    let opened: Vec<&str> = rows
        .iter()
        .filter_map(|row| match &row.event {
            CompanyEvent::EpisodeOpened { episode_id, .. } => Some(episode_id.as_str()),
            _ => None,
        })
        .collect();
    for episode in &opened {
        assert!(
            rows.iter().any(|row| matches!(
                &row.event,
                CompanyEvent::EpisodeCompleted { episode_id, .. } if episode_id == episode
            )),
            "episode {episode} opened and never closed"
        );
    }
    eprintln!("rooms opened: {}", opened.len());
}

/// The same shape for a hand-off: a request that plainly belongs to somebody
/// else, and a teammate that owns something adjacent.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "spends real model calls; run deliberately with --ignored"]
async fn a_real_model_can_hand_a_conversation_to_whoever_owns_it() {
    let Some((key, model)) = live() else {
        eprintln!("no OPENROUTER_API_KEY / OPENROUTER_MODEL in the environment — not asked for");
        return;
    };
    eprintln!("live run against {model}");

    let home = tempfile::tempdir().unwrap();
    let (address, runtime) = boot(home.path(), key, model).await;
    let client = Client::new(address);
    client.sign_in(ADMIN).await;

    client
        .dm(
            ENGINEER,
            "Please write the customer-facing release note for the checkout \
             rollout — the wording matters more than the technical detail.",
        )
        .await;

    let waited = wait_for_reply(&runtime, ENGINEER, LIVE_BUDGET).await;
    let rows = transcript(&runtime, waited).await;

    let answered = rows.iter().any(|row| {
        matches!(
            &row.event,
            CompanyEvent::AgentReply { chat_id, agent_id, .. }
                if chat_id == &format!("dm:{ENGINEER}") && agent_id == ENGINEER
        )
    });
    assert!(
        answered,
        "the teammate answered the operator within the budget"
    );

    // If it handed off, the receiving teammate opened its own channel and
    // nobody else's. If it did not, that is the model's call to make and the
    // transcript says so.
    let handed: Vec<&str> = rows
        .iter()
        .filter_map(|row| match &row.event {
            CompanyEvent::AgentReply {
                chat_id, agent_id, ..
            } if chat_id.starts_with("dm:") && chat_id != &format!("dm:{ENGINEER}") => {
                Some(agent_id.as_str())
            }
            _ => None,
        })
        .collect();
    for author in &handed {
        assert!(
            rows.iter().any(|row| matches!(
                &row.event,
                CompanyEvent::AgentReply { chat_id, agent_id, .. }
                    if chat_id == &format!("dm:{author}") && agent_id == author
            )),
            "a teammate only ever opens its own channel, never somebody else's"
        );
    }
    eprintln!("channels opened by a hand-off: {handed:?}");
}
