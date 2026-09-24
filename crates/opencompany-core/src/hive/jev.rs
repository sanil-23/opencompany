//! Jev routing over the TinyHumans System One proxy.
//!
//! A desk episode asks Jev — TypeSafe's System One model — who should speak
//! next: which seats a message needs, whether a broadcast fans out, whether the
//! question is too ambiguous to route at all. `tinyhivemind_typesafe::JevRouter`
//! owns that conversation; it deliberately owns **no HTTP client, credential
//! or runtime**, and asks the host for exactly one thing — a
//! [`SystemOneTransport`] that can put a [`SystemOneRequest`] on the wire and
//! bring a [`SystemOneResponse`] back. This module is that thing.
//!
//! The wire goes to **TinyHumans**, not to TypeSafe directly. The
//! `/agent-integrations/openrouter/systemone` route on the TinyHumans API is a
//! System One proxy: same request body, same response body, but authenticated
//! with the TinyHumans key every hosted and desktop instance already has, so a
//! company needs no second credential to route. The proxy resolves the
//! `jev-latest` alias to a concrete `typesafe/jev-*` id and reports that id in
//! the response's `model`; the router records it and never compares it to the
//! request's, so the alias is safe to keep asking for.
//!
//! Three rules, each the reason for a line below:
//!
//! * **No key, no router.** [`jev_router`] answers `None` when no TinyHumans
//!   credential resolves, and the caller routes by desk lead and explicit
//!   mention instead (`docs/spec/runtime/hive.md`). A router that would fail
//!   every call is worse than none: every episode would pay a timeout before
//!   falling back.
//! * **The key is the one that belongs to the host it is sent to.**
//!   [`JEV_KEY_ENV`] first, then `OPENCOMPANY_INFERENCE_KEY` **only when
//!   inference resolves to the same host as the proxy**, then the TinyHumans
//!   token tiers (projected file over static `TINYHUMANS_API_KEY`) — through
//!   the same [`Credential`] seam the harness provider uses, so a rotated
//!   token file is picked up per request and a 401 invalidates the cached
//!   read. See [`jev_credential`] for why the middle step is conditional.
//! * **The URL is the operator's to move, not to downgrade.**
//!   `OPENCOMPANY_JEV_URL` points a staging box at its own proxy; it must be
//!   `https`, or `http` to a loopback host, for the same reason the analytics
//!   collector has that rule: the credential is a request header on every
//!   call, and a plain-`http` endpoint on a container network would put it on
//!   the wire in the clear.
//!
//! One retry, then the caller's fallback. A `429`/`529` (TypeSafe's documented
//! transient statuses, `classify_retry`) or any `5xx` from the proxy in front
//! gets exactly one immediate second attempt; anything else — a `4xx`, a
//! timeout, a connection refused — is returned at once as
//! [`Error::Transport`] and the driver routes the round without Jev. Routing
//! is on the critical path of every desk round, so a long backoff here would
//! be a long silence in the room.

use std::time::Duration;

use tinyhivemind_typesafe::{
    Error, JevRouter, RetryClass, SystemOneRequest, SystemOneResponse, SystemOneTransport,
    SystemOneTransportFuture, classify_retry,
};

use crate::app::config::EnvSource;
use crate::company::credentials::Credential;
use crate::error::{OpenCompanyError, Result};

/// The TinyHumans System One proxy every instance routes through unless
/// [`JEV_URL_ENV`] says otherwise.
pub const DEFAULT_JEV_URL: &str =
    "https://api.tinyhumans.ai/agent-integrations/openrouter/systemone";

/// The environment variable that moves the proxy: `https`, or `http` to a
/// loopback host. Anything else is refused at construction, not at first use.
pub const JEV_URL_ENV: &str = "OPENCOMPANY_JEV_URL";

/// The environment variable that gives Jev its own credential, outranking
/// every other source.
///
/// It exists because the proxy and the inference endpoint stopped being the
/// same host. A deployment that points `OPENCOMPANY_INFERENCE_URL` at
/// OpenRouter, Together or its own gateway still routes desks through
/// TinyHumans, and before this variable there was no way to say so: the only
/// key Jev could reach was the inference one. See [`jev_credential`].
pub const JEV_KEY_ENV: &str = "OPENCOMPANY_JEV_KEY";

/// TypeSafe's own key, read when [`JEV_KEY_ENV`] says nothing.
///
/// The name `tinyjevclient` reads (`Client::from_env`), so a deployment that
/// already holds a TypeSafe key for the library does not have to restate it
/// under a second name for this host.
pub const TYPESAFE_KEY_ENV: &str = "TYPESAFE_API_KEY";

/// Jev's System One route on TypeSafe's own API, used when the credential is
/// TypeSafe's rather than TinyHumans'.
///
/// Same request and response body as the proxy — `tinyjevclient` posts the
/// identical `{state, model, questions}` to `v1/systemone` and reads back
/// `{model, answers, usage}` — so only the address and the bearer differ.
pub const DEFAULT_TYPESAFE_URL: &str = "https://api.typesafe.ai/v1/systemone";

/// One routing call's ceiling, connect included. A round waits on this before
/// it can start its seats, so it is short: a routing model that has not
/// answered in thirty seconds is one the driver should stop waiting for.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// The host-owned [`SystemOneTransport`]: one `reqwest` client, one proxy URL,
/// one credential. Cheap to clone; the client is a connection pool behind an
/// `Arc`.
#[derive(Clone)]
pub struct TinyHumansSystemOne {
    client: reqwest::Client,
    url: String,
    bearer: Credential,
}

impl std::fmt::Debug for TinyHumansSystemOne {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The credential never prints; its tier is the only thing worth seeing.
        f.debug_struct("TinyHumansSystemOne")
            .field("url", &self.url)
            .field("bearer", &self.bearer.source())
            .finish()
    }
}

impl TinyHumansSystemOne {
    /// A transport to `url` presenting `bearer`. Refuses a URL the credential
    /// may not cross (see [`validate_url`]) and a client that cannot be built.
    pub fn new(url: impl Into<String>, bearer: Credential) -> Result<Self> {
        let url = validate_url(url.into())?;
        let client = client_builder()
            .timeout(REQUEST_TIMEOUT)
            .connect_timeout(Duration::from_secs(10))
            .build()
            .map_err(|error| OpenCompanyError::Config(format!("jev http client: {error}")))?;
        Ok(Self {
            client,
            url,
            bearer,
        })
    }

    /// Convenience over [`Self::new`] for a key already in hand.
    pub fn with_key(url: impl Into<String>, key: impl Into<String>) -> Result<Self> {
        Self::new(url, Credential::from_value(key))
    }

    /// The proxy this transport posts to.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// One attempt: post the request, map every failure onto
    /// [`Error::Transport`] with the status when there is one.
    async fn attempt(
        &self,
        request: &SystemOneRequest,
    ) -> std::result::Result<SystemOneResponse, Error> {
        let mut builder = self.client.post(&self.url).json(request);
        let bearer = self
            .bearer
            .current()
            .await
            .map_err(|error| Error::Transport {
                status: None,
                message: format!("jev credential: {error}"),
            })?;
        if let Some(token) = bearer {
            builder = builder.bearer_auth(token);
        }
        let response = builder.send().await.map_err(|error| Error::Transport {
            status: error.status().map(|status| status.as_u16()),
            message: describe_reqwest(&error),
        })?;
        let status = response.status();
        if !status.is_success() {
            if status == reqwest::StatusCode::UNAUTHORIZED {
                // A projected token file may have rotated under us; the next
                // read comes from disk again. A static key is unaffected.
                self.bearer.invalidate();
            }
            let message = response.text().await.unwrap_or_default();
            return Err(Error::Transport {
                status: Some(status.as_u16()),
                message: truncate(message),
            });
        }
        response
            .json::<SystemOneResponse>()
            .await
            .map_err(|error| Error::Transport {
                status: Some(status.as_u16()),
                message: format!("undecodable System One response: {error}"),
            })
    }
}

impl SystemOneTransport for TinyHumansSystemOne {
    fn evaluate<'a>(&'a self, request: &'a SystemOneRequest) -> SystemOneTransportFuture<'a> {
        Box::pin(async move {
            match self.attempt(request).await {
                Err(Error::Transport {
                    status: Some(status),
                    message,
                }) if should_retry(status) => {
                    tracing::debug!(status, %message, url = %self.url, "jev transport: retrying once");
                    self.attempt(request).await
                }
                outcome => outcome,
            }
        })
    }
}

/// The Jev router for this process, or `None` when nothing can authenticate
/// to the proxy.
///
/// `key` is a credential the caller already holds — a company's own key from
/// its secret store — and outranks the environment. With `None`, the key and
/// the address are [`jev_endpoint`]'s, which resolves them **together**: a
/// key is only valid at the host it was issued for, so choosing one without
/// the other is how a TypeSafe key ends up at TinyHumans and 401s every
/// round. [`JEV_URL_ENV`] still overrides the address whatever the key.
///
/// `Ok(None)` is the ordinary offline answer and the caller logs it once and
/// routes by lead and mention. `Err` is a configuration the operator has to
/// fix — an `http://` proxy on a routable host — and is never silently
/// downgraded to `None`, because "routing without Jev" would then look like
/// the intended state rather than a misconfiguration.
pub fn jev_router(
    env: &dyn EnvSource,
    key: Option<&str>,
) -> Result<Option<JevRouter<TinyHumansSystemOne>>> {
    let (url, resolved) = jev_endpoint(env);
    let credential = match key.map(str::trim).filter(|key| !key.is_empty()) {
        Some(key) => Credential::from_value(key),
        None => resolved,
    };
    if !credential.configured() {
        return Ok(None);
    }
    let transport = TinyHumansSystemOne::new(url, credential)?;
    Ok(Some(JevRouter::new(transport)))
}

/// The address to route through and the credential to present there, resolved
/// as one decision.
///
/// The two were separate, and that was the bug. [`DEFAULT_JEV_URL`] is the
/// **TinyHumans** proxy, which authenticates with a TinyHumans key; a
/// deployment holding a **TypeSafe** key got that key posted to the proxy,
/// where it is not an account, and every round 401'd and fell back. Nothing
/// reported it, because a wrong key is not an absent one and only an absent
/// one turns the router off.
///
/// So the credential picks the address:
///
/// - An explicit Jev key ([`JEV_KEY_ENV`], then [`TYPESAFE_KEY_ENV`]) is
///   TypeSafe's, so it goes to [`DEFAULT_TYPESAFE_URL`] — TypeSafe's own
///   System One route, same wire body as the proxy.
/// - Otherwise the address is the TinyHumans proxy and the credential is a
///   TinyHumans one ([`jev_credential`]).
///
/// [`JEV_URL_ENV`] overrides the address in both cases, which is what points
/// a staging box at its own endpoint without restating the key.
fn jev_endpoint(env: &dyn EnvSource) -> (String, Credential) {
    let override_url = env
        .get(JEV_URL_ENV)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let typesafe = [JEV_KEY_ENV, TYPESAFE_KEY_ENV]
        .into_iter()
        .find_map(|name| {
            env.get(name)
                .map(|key| key.trim().to_string())
                .filter(|key| !key.is_empty())
        });
    match typesafe {
        Some(key) => (
            override_url.unwrap_or_else(|| DEFAULT_TYPESAFE_URL.to_string()),
            Credential::from_value(key),
        ),
        None => {
            let url = override_url.unwrap_or_else(|| DEFAULT_JEV_URL.to_string());
            let credential = jev_credential(env, &url);
            (url, credential)
        }
    }
}

/// The credential to present at `jev_url`, in the order the sources are
/// allowed to answer.
///
/// Reached only when no explicit Jev key was set — [`jev_endpoint`] takes
/// that case, because it also decides the address. So the sources here are
/// the TinyHumans ones:
///
/// 1. `OPENCOMPANY_INFERENCE_KEY` — **only** when managed inference resolves
///    to the same host as `jev_url`.
/// 2. The TinyHumans token tiers: the projected token file, then the static
///    `TINYHUMANS_API_KEY`.
///
/// # Why step 2 is conditional
///
/// It did not use to be. This function's job was done inline by
/// [`hosted_endpoint_from_env_at`](crate::harness::built_in::provider), which
/// reads `OPENCOMPANY_INFERENCE_KEY` unconditionally and whose URL half was
/// thrown away here. That was correct exactly as long as the two were the same
/// endpoint — the module header's "a company needs no second credential to
/// route" — and `OPENCOMPANY_INFERENCE_URL` is the documented seam for making
/// them different. Point inference at OpenRouter, as the hosted-runner and
/// local-development paths both do, and every desk round took that OpenRouter
/// key and put it in an `Authorization` header to `api.tinyhumans.ai`.
///
/// Two things went wrong, and the quieter one is the worse one. The visible
/// failure is that the call 401s, so the router is built, fails every request,
/// pays a retry, and routes by the fallback anyway — the "no key, no router"
/// rule inverted, since a wrong key is not an absent one and nothing reports
/// the difference. The failure that matters is that one provider's secret was
/// sent to a different provider's host, on every round, forever. A credential
/// is scoped to the host it was issued for; sending it anywhere else is a
/// disclosure whether or not the recipient wanted it.
///
/// So the inference key is reused only when it is going back to the host it
/// belongs to. Host equality is the whole test: same host, same account, same
/// key; different host, a key that was never issued for this call. That keeps
/// the managed default — where inference *is* the TinyHumans proxy — needing
/// no second credential, which is the property the header promises, while a
/// deployment that has pointed inference elsewhere resolves Jev from its own
/// sources or routes without it.
fn jev_credential(env: &dyn EnvSource, jev_url: &str) -> Credential {
    let inference_url = crate::harness::built_in::provider::platform_inference_url_at(env, None);
    if same_host(&inference_url, jev_url)
        && let Some(key) = env
            .get("OPENCOMPANY_INFERENCE_KEY")
            .map(|key| key.trim().to_string())
            .filter(|key| !key.is_empty())
    {
        return Credential::from_value(key);
    }
    crate::company::credentials::TinyhumansTokenSource::from_env(env)
        .map(|source| Credential::from_source(std::sync::Arc::new(source)))
        .unwrap_or_default()
}

/// Whether two URLs address the same host, comparing host and port only.
///
/// Scheme and path are deliberately not compared: the proxy route and the
/// inference route are different paths on the same API, which is the case this
/// has to answer "yes" for. A URL that will not parse answers `false` — an
/// unparseable endpoint is not demonstrably the same host, and the safe
/// direction for a question guarding a credential is to withhold it.
fn same_host(left: &str, right: &str) -> bool {
    let host_of = |url: &str| {
        url::Url::parse(url).ok().and_then(|url| {
            url.host_str()
                .map(|host| (host.to_ascii_lowercase(), url.port_or_known_default()))
        })
    };
    match (host_of(left), host_of(right)) {
        (Some(left), Some(right)) => left == right,
        _ => false,
    }
}

/// Whether a failed status earns the one retry: TypeSafe's documented
/// transient statuses per [`classify_retry`], plus any `5xx` — the proxy in
/// front of TypeSafe answers `502`/`503`/`504` for its own upstream hiccups,
/// which are exactly as transient as a `529` and just as worth one more try.
pub fn should_retry(status: u16) -> bool {
    matches!(classify_retry(status), RetryClass::Retryable) || (500..=599).contains(&status)
}

/// The URL rule: parseable, `http(s)`, with a host, and — since the credential
/// rides in a header on every call — `https`, or `http` only to a loopback
/// host. Mirrors the analytics collector rule and reuses its judgement.
pub fn validate_url(url: String) -> Result<String> {
    let parsed = url::Url::parse(&url)
        .map_err(|error| OpenCompanyError::Config(format!("{JEV_URL_ENV}: {error}")))?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none_or(str::is_empty) {
        return Err(OpenCompanyError::Config(format!(
            "{JEV_URL_ENV} must be an http(s) URL with a host, got {url:?}"
        )));
    }
    if !crate::analytics::config::is_secure_endpoint(&url) {
        return Err(OpenCompanyError::Config(format!(
            "{JEV_URL_ENV} is a plain http:// URL to a non-loopback host; the TinyHumans key \
             is a request header on every routing call and would cross the network in the \
             clear. Use https, or a loopback address."
        )));
    }
    Ok(url)
}

/// The TLS-aware client builder the embedded runtime's own backend calls use,
/// so a corporate CA or proxy configured for OpenHuman applies here too.
fn client_builder() -> reqwest::ClientBuilder {
    openhuman_core::util::tls::tls_client_builder()
}

fn describe_reqwest(error: &reqwest::Error) -> String {
    if error.is_timeout() {
        format!("timed out after {}s", REQUEST_TIMEOUT.as_secs())
    } else if error.is_connect() {
        "connection failed".to_string()
    } else {
        // `reqwest::Error`'s Display includes the URL; that is fine here (it
        // holds no secret — the bearer is a header) and names the proxy.
        error.to_string()
    }
}

/// Keeps a provider error body to something a log line can hold.
fn truncate(message: String) -> String {
    const MAX: usize = 512;
    if message.len() <= MAX {
        return message;
    }
    let mut cut = MAX;
    while !message.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}…", &message[..cut])
}

#[cfg(test)]
#[path = "jev_tests.rs"]
mod tests;
