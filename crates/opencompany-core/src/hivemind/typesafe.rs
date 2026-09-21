//! The live System One transport: this host's side of TypeSafe routing.
//!
//! [`tinyhivemind_typesafe::SystemOneTransport`] is the port a semantic routing
//! decision is asked through. The library builds the exact question — which of
//! these candidates should take this message — and this module is the only
//! thing that puts it on a wire. Everything above it stays transport-free, which
//! is why the crate can be exercised end to end against a fixture and why this
//! file is the only place a credential appears.
//!
//! # Why the base URL is a field
//!
//! Resolved once at construction rather than derived per request, so a test can
//! point the transport at a local stub without the production path carrying a
//! test-only branch. `ChargebeeClient` holds its own for the same reason and
//! this matches it deliberately.
//!
//! # Why redirects are refused
//!
//! Every request carries the API key as a bearer token. reqwest follows
//! redirects by default **and re-sends the `Authorization` header**, so a 30x
//! pointing at `http://` would put the key on the wire in clear text. A scheme
//! check would still leave same-scheme redirection to an unintended host, so
//! refusing redirects outright is both stricter and simpler. System One does not
//! redirect, so it costs nothing.
//!
//! # Failure is typed, not swallowed
//!
//! Every failure path returns [`Error::Transport`] carrying the status when one
//! exists. The library's routing fold reads that as an unusable evaluation and
//! falls back to the caller's deterministic destination *without* inventing a
//! recipient — so a transport outage degrades routing to the fallback rather
//! than to a confident wrong answer.

use std::time::Duration;

use tinyhivemind_typesafe::{
    Error, SystemOneRequest, SystemOneResponse, SystemOneTransport, SystemOneTransportFuture,
};

/// The System One endpoint this host talks to when none is configured.
pub const DEFAULT_BASE_URL: &str = "https://api.typesafe.ai/v1/systemone";

/// The routing credential. Absent means semantic routing is off.
pub const API_KEY_ENV: &str = "OPENCOMPANY_TYPESAFE_API_KEY";
/// Overrides [`DEFAULT_BASE_URL`], for staging or a stub.
pub const BASE_URL_ENV: &str = "OPENCOMPANY_TYPESAFE_BASE_URL";
/// Overrides [`DEFAULT_MODEL`].
pub const MODEL_ENV: &str = "OPENCOMPANY_TYPESAFE_MODEL";
/// The Jev model this host asks for unless the environment says otherwise.
///
/// The moving alias, not a version — and deliberately, after pinning failed.
///
/// `jev-1.13` looked like the honest pin: `tinyhivemind`'s PR #55 records "live
/// Jev 1.13 routing" and the TypeSafe adapter's own tests assert
/// `model_identity == "jev-1.13"` round-tripping back. Neither is the service's
/// list of servable ids, and the endpoint answers that pin with
///
/// ```text
/// 400 {"detail":{"error_type":"api_usage_error","message":"Unknown model: jev-1.13"}}
/// ```
///
/// A refused model fails closed — routing declines and the mechanical responder
/// takes the message — which is the safe direction and also an invisible one:
/// the handoff still lands, so a broken pin reads as a healthy fallback from
/// outside. That is what the transport's error logging exists to catch.
///
/// A response carries the version that actually answered, so the way to pin
/// honestly is to read `model_identity` off a successful call and set
/// `OPENCOMPANY_TYPESAFE_MODEL` to it. Until somebody does, the alias is the
/// only id known to be servable.
pub const DEFAULT_MODEL: &str = "jev-latest";

/// How long one routing question may take before it is a transport failure.
///
/// A routing Choice sits in front of a turn the room is waiting on, so an
/// unbounded wait is a stalled desk rather than a slow one. Thirty seconds
/// matches the bound the other outbound clients in this crate use.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Where the bearer token on each request comes from.
///
/// Two tiers, because routing has two ways to be paid for and they differ in
/// more than the string. A BYO System One provider hands over a fixed key. The
/// managed platform may instead project a **rotating**, audience-bound identity
/// into a file, so the credential has to be re-read rather than captured once —
/// which is what [`TinyhumansTokenSource::current`] does, cache window and all.
#[derive(Clone)]
enum Credential {
    /// `OPENCOMPANY_TYPESAFE_API_KEY`: this company's own provider.
    Own(String),
    /// The platform credential every other managed call already presents.
    /// Behind an `Arc` on that type's own instruction: the projected-file
    /// tier keeps a cache, and cloning the source would clone the cache and
    /// defeat it — so every clone of this transport shares one reader.
    Managed(std::sync::Arc<crate::company::credentials::TinyhumansTokenSource>),
}

impl Credential {
    /// The token to present on the next request.
    async fn bearer(&self) -> Result<String, Error> {
        match self {
            Self::Own(key) => Ok(key.clone()),
            // Re-read per request, not captured: a projected file rotates in
            // place, and a transport holding the first read would keep
            // presenting a token the platform has already retired.
            Self::Managed(source) => source.current().await.map_err(|error| Error::Transport {
                status: None,
                message: format!("the platform routing credential could not be read: {error}"),
            }),
        }
    }
}

/// Where OpenRouter itself serves System One.
///
/// The endpoint the managed proxy forwards to — `systemOne.ts` posts to
/// `${OPENROUTER_BASE_URL}/systemone` — so a company holding its OWN OpenRouter
/// key can reach the same evaluator directly, without the platform in the
/// middle and without a second credential to provision.
///
/// Whether that key's account is entitled to Jev is not knowable from here. A
/// refusal arrives as a non-success status, `route_semantic` declines, and the
/// handoff falls to the mechanical responder — the same degrade as any other
/// routing failure, and now a logged one.
pub const OPENROUTER_SYSTEM_ONE_URL: &str = "https://openrouter.ai/api/v1/systemone";

/// Where `api.tinyhumans.ai` serves System One.
///
/// Appended to the platform API base. The backend registers this alias
/// deliberately — its own route docs say it "lets TypeSafe-compatible clients
/// use `https://api.tinyhumans.ai/agent-integrations/openrouter` as their base
/// URL while continuing to append `/v1/systemone` themselves", which is exactly
/// what this transport does.
pub const MANAGED_SYSTEM_ONE_PATH: &str = "/agent-integrations/openrouter/v1/systemone";

/// The platform API base the managed tier routes against.
///
/// `TINYHUMANS_API_URL` so a staging instance points at its own backend,
/// matching how `RuntimeConfig` resolves the same value — read from the
/// environment here rather than threaded through, because this transport is
/// built from the environment and the variable is the same one.
fn managed_api_url() -> String {
    std::env::var("TINYHUMANS_API_URL")
        .ok()
        .map(|value| value.trim().trim_end_matches('/').to_owned())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| crate::app::config::DEFAULT_API_URL.to_owned())
}

/// A live System One transport bound to one endpoint and one credential.
#[derive(Clone)]
pub struct TypeSafeTransport {
    http: reqwest::Client,
    credential: Credential,
    base_url: String,
}

/// Prints the endpoint and **redacts the API key**.
///
/// A derived `Debug` would put a live credential into any log line that
/// formatted a transport. Matches `ChargebeeClient`, which hand-writes its own
/// for exactly this reason.
impl std::fmt::Debug for TypeSafeTransport {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TypeSafeTransport")
            .field("base_url", &self.base_url)
            .finish_non_exhaustive()
    }
}

impl TypeSafeTransport {
    /// Builds a transport against the real System One endpoint.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Transport`] when the HTTP client cannot be built, which
    /// can only mean a malformed TLS or timeout configuration here.
    pub fn new(api_key: String) -> Result<Self, Error> {
        Self::with_base_url(api_key, DEFAULT_BASE_URL.to_owned())
    }

    /// Builds a transport against an explicit endpoint, with no trailing slash.
    ///
    /// # Errors
    ///
    /// As [`new`](Self::new).
    pub fn with_base_url(api_key: String, base_url: String) -> Result<Self, Error> {
        Self::with_credential(Credential::Own(api_key), base_url)
    }

    /// The constructor both tiers share, once the credential is decided.
    ///
    /// # Errors
    ///
    /// As [`with_base_url`](Self::with_base_url).
    fn with_credential(credential: Credential, base_url: String) -> Result<Self, Error> {
        // **The endpoint must be encrypted, or loopback.**
        //
        // `evaluate` posts the routing JSON with the API key as a bearer
        // header on every request, so a plain-`http` endpoint on a container
        // network would put the credential on the wire in the clear. Refusing
        // the URL beats warning about it: by the time a warning is read the
        // key has already been sent. Refusing redirects does not cover this —
        // that protects the SECOND hop, and this is the first.
        //
        // The same rule, and the same function, as the analytics collector
        // (`OPENCOMPANY_ANALYTICS_ENDPOINT`), whose reasoning this repeats
        // verbatim: `https` anywhere, `http` only to a loopback host. A second
        // spelling of "is this safe to send a secret to" is how the two would
        // drift, and one of them would be the lenient one.
        //
        // Loopback stays allowed because the offline tests serve a canned
        // response on an ephemeral 127.0.0.1 port, which is the shape
        // `chargebee::client_tests` uses and puts nothing on a network.
        // (CodeRabbit on #2412, CWE-319.)
        if !crate::analytics::config::is_secure_endpoint(&base_url) {
            return Err(Error::Transport {
                status: None,
                message: format!(
                    "System One endpoint `{base_url}` is not encrypted: the API key rides every \
                     request as a bearer header, so `{BASE_URL_ENV}` must be `https`, or `http` \
                     to a loopback host"
                ),
            });
        }
        let http = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            // See the module docs: the bearer token would survive a redirect.
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|error| Error::Transport {
                status: None,
                message: format!("System One client could not be built: {error}"),
            })?;
        Ok(Self {
            http,
            credential,
            base_url,
        })
    }

    /// The endpoint this transport asks, for a startup log line.
    #[must_use]
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Build a transport against OpenRouter's own System One endpoint.
    ///
    /// The third tier: a company that brought its own OpenRouter key reaches
    /// Jev directly with it. Same evaluator the managed proxy forwards to, one
    /// hop shorter.
    ///
    /// # Errors
    ///
    /// As [`with_base_url`](Self::with_base_url).
    pub fn with_openrouter_key(api_key: String) -> Result<Self, Error> {
        Self::with_credential(
            Credential::Own(api_key),
            OPENROUTER_SYSTEM_ONE_URL.to_owned(),
        )
    }

    /// Build a transport from the process environment, or `None` when this
    /// instance has no routing credential.
    ///
    /// `None` is a routing feature that is *off*, not a failure: a company
    /// without a key routes by explicit `@id` exactly as it did before semantic
    /// routing existed. That is the same direction
    /// [`built_in::selector`](crate::harness::built_in::selector) takes — "the
    /// worst case of the new rung is the old rung" — and it is why this returns
    /// an `Option` rather than erroring at boot.
    ///
    /// Environment rather than [`SecretStore`](crate::ports::secrets::SecretStore)
    /// for now: routing is one credential for the instance rather than per
    /// company, and a per-company key can move here later without changing a
    /// caller. `OPENCOMPANY_TYPESAFE_BASE_URL` overrides the endpoint for a
    /// staging or stubbed System One.
    ///
    /// # Errors
    ///
    /// As [`new`](Self::new), when a key is present but the client cannot be
    /// built.
    pub fn from_env() -> Result<Option<Self>, Error> {
        let trimmed = |name: &str| {
            std::env::var(name)
                .ok()
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty())
        };

        // **A company's own provider outranks the platform's.**
        //
        // Same precedence `OPENCOMPANY_INFERENCE_KEY` takes over
        // `TINYHUMANS_API_KEY`: an instance that went to the trouble of
        // configuring its own System One endpoint means it, and a managed
        // credential lying around in the environment must not quietly win.
        if let Some(api_key) = trimmed(API_KEY_ENV) {
            let base_url = trimmed(BASE_URL_ENV).unwrap_or_else(|| DEFAULT_BASE_URL.to_owned());
            return Self::with_credential(Credential::Own(api_key), base_url).map(Some);
        }

        // **Otherwise the managed platform serves it, if this instance has one.**
        //
        // `api.tinyhumans.ai` proxies System One at
        // `/agent-integrations/openrouter/v1/systemone`, and that alias exists
        // for exactly this: its own docs say it "lets TypeSafe-compatible
        // clients use `https://api.tinyhumans.ai/agent-integrations/openrouter`
        // as their base URL while continuing to append `/v1/systemone`
        // themselves". It authenticates with the same bearer identity every
        // other managed call presents, and accepts the same Jev ids this host
        // pins (`jev-latest`, `jev-1.13.0`, optionally `typesafe/`-prefixed).
        //
        // So routing arrives with managed inference rather than needing a
        // third credential of its own — which is the question this feature left
        // open, answered in the direction that costs an operator nothing.
        // Still `None` when neither exists: routing off, not a failure.
        let Some(source) = crate::company::credentials::TinyhumansTokenSource::from_env(
            &crate::app::config::ProcessEnv,
        ) else {
            return Ok(None);
        };
        let base_url = format!("{}{MANAGED_SYSTEM_ONE_PATH}", managed_api_url());
        Self::with_credential(Credential::Managed(std::sync::Arc::new(source)), base_url).map(Some)
    }
}

/// The routing model this host pins.
///
/// Pinned rather than taking [`JevRouter`](tinyhivemind_typesafe::JevRouter)'s
/// own `jev-latest` default, for the reason every other version in this repo is
/// pinned: a silently-updated router changes which teammate receives a handoff
/// with no diff to review and no way to tell a behaviour change from a bad day.
/// `OPENCOMPANY_TYPESAFE_MODEL` moves it without a rebuild.
#[must_use]
pub fn routing_model() -> String {
    std::env::var(MODEL_ENV)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_MODEL.to_owned())
}

impl SystemOneTransport for TypeSafeTransport {
    fn evaluate<'a>(&'a self, request: &'a SystemOneRequest) -> SystemOneTransportFuture<'a> {
        Box::pin(async move {
            // Resolved per request, because the managed tier may be a rotating
            // projected file. The own-key tier is a clone of a `String`.
            let bearer = self.credential.bearer().await?;
            let response = self
                .http
                .post(&self.base_url)
                .bearer_auth(&bearer)
                .json(request)
                .send()
                .await
                .map_err(|error| {
                    // `route_semantic` matches `let Ok(..) else` and drops this,
                    // so a failed route degrades to the mechanical pick with no
                    // reason attached. Said here, where the reason still exists:
                    // a broken credential and a healthy fallback are otherwise
                    // indistinguishable from the outside.
                    tracing::warn!(
                        %error,
                        endpoint = %self.base_url,
                        "[typesafe] the routing request did not reach System One"
                    );
                    Error::Transport {
                        status: None,
                        message: error.to_string(),
                    }
                })?;

            let status = response.status();
            if !status.is_success() {
                // The body is the provider's own diagnostic and is carried
                // verbatim: a routing refusal that says only "400" is a support
                // ticket, and nothing here is in a position to summarise it.
                let message = response.text().await.unwrap_or_default();
                // The provider's own words. A routing refusal that says only
                // "400" is a support ticket; this is the line that names a
                // retired model id, an unentitled key or a schema drift.
                tracing::warn!(
                    status = status.as_u16(),
                    endpoint = %self.base_url,
                    body = %message,
                    "[typesafe] System One refused the routing request"
                );
                return Err(Error::Transport {
                    status: Some(status.as_u16()),
                    message,
                });
            }

            response.json::<SystemOneResponse>().await.map_err(|error| {
                tracing::warn!(
                    %error,
                    status = status.as_u16(),
                    "[typesafe] System One answered with a body this host cannot decode"
                );
                Error::Transport {
                    status: Some(status.as_u16()),
                    // A success status with a body this host cannot decode
                    // is a schema drift, not an outage, and the two are
                    // worth telling apart in a log — so the status rides
                    // along.
                    message: format!("System One response did not decode: {error}"),
                }
            })
        })
    }
}

#[cfg(test)]
#[path = "typesafe_tests.rs"]
mod tests;
