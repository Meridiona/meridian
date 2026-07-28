//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Shared HTTP client and transient-failure classification for PM provider sync.
//!
//! Two concerns that look separate but are the same concern: how we talk to a
//! provider's API, and how we decide whether a failed conversation is the
//! user's problem or the network's.
//!
//! # Why this exists
//! Every provider sync used to build `reqwest::Client::new()` per call and then
//! treat ANY failure as a terminal, user-facing fault. Both halves were wrong:
//!
//! 1. **No timeouts.** `reqwest`'s defaults are `timeout: no timeout` and
//!    `connect_timeout: None`, so a connect that hung never came back and the
//!    provider simply stopped syncing with no error at all.
//! 2. **No transient/terminal distinction.** A DNS blip, a captive portal, a
//!    laptop resuming from sleep mid-tick — each raised a persistent
//!    "GitHub sync failing / Set GITHUB_TOKEN in .env" banner that told the
//!    user to re-do credentials that were never broken. [`crate::intelligence::providers::jira`]'s
//!    OAuth-refresh path already learned this (see the `is_transient` guard in
//!    `jira::refresh_if_stale` and the comment above it: raising a terminal
//!    fault for a network blip is "exactly what made this fault flap on and off
//!    at random"). That guard was only ever applied to refreshing the token,
//!    never to using it — so every data-fetch path kept the original bug.
//!
//! [`is_transient`] is the shared answer, and unlike
//! [`meridian_oauth::flow::is_transient`] — which only recognises a
//! `TokenError` from the token endpoint and returns `false` for everything else
//! — it classifies the transport errors a data fetch actually produces. The two
//! are complementary, not interchangeable: dropping the OAuth one onto a fetch
//! path compiles, runs, and returns `false` every single time.
//!
//! # The deliberate trade-off
//! A transient failure raises NO user-facing notice; the stale cache is kept and
//! the next tick retries. So a provider that is unreachable *forever* now goes
//! quiet rather than showing a (misleading) credentials banner. That is the
//! same trade-off `jira::refresh_if_stale` already shipped, and this matches it
//! on purpose rather than inventing a second policy. Escalating after N
//! consecutive transient failures would need persisted state (a `pm_sync_state`
//! column, so a migration) and is left as a follow-up.
//!
//! # Who calls this
//! - [`crate::intelligence::providers::github`] — `refresh_if_stale` (viewer
//!   fetch + all-projects-failed) and `fetch::{fetch_viewer_login, fetch_project_items}`.
//! - [`crate::intelligence::providers::jira`] — `refresh_if_stale` (fetch
//!   failure) and `fetch::search`.
//!
//! # Related
//! - [`crate::intelligence::providers::stamp_sync_error`] — what we call only
//!   once a failure is classified terminal.
//! - [`meridian_oauth::flow::is_transient`] — the OAuth-token-endpoint sibling.

use std::sync::OnceLock;
use std::time::Duration;

/// Total deadline for one provider request, from connect to body complete.
///
/// Generous on purpose: this exists to bound a hang, not to police latency. A
/// large Jira JQL search over a slow link is a legitimate slow request and must
/// not be cut off, so this is set well above any observed healthy call.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

/// Deadline for the connect phase alone. Much tighter than [`REQUEST_TIMEOUT`]
/// because a connect that has not completed in this long is not slow, it is
/// broken — the DNS/captive-portal/unreachable-host cases this module exists
/// for. Failing fast here turns a silent stall into a classified transient
/// error that retries on the next tick.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// How long an idle pooled connection may linger before being dropped.
///
/// Only relevant because the client is now SHARED across ticks (it previously
/// was not, so every tick started with an empty pool and this could not
/// happen). A pooled connection that a NAT, proxy, or the server has silently
/// closed fails on reuse; bounding idle lifetime well below the multi-minute
/// sync interval means a fresh connection is established each tick instead.
const POOL_IDLE_TIMEOUT: Duration = Duration::from_secs(30);

/// `SO_KEEPALIVE` interval — surfaces a dead peer rather than blocking until
/// [`REQUEST_TIMEOUT`]. Paired with [`POOL_IDLE_TIMEOUT`] against the same
/// silently-dropped-connection failure mode.
const TCP_KEEPALIVE: Duration = Duration::from_secs(30);

/// The process-wide client for PM provider sync.
///
/// Cloning a `reqwest::Client` is cheap (it is an `Arc` internally) and shares
/// the connection pool, so callers should call this per request rather than
/// caching the returned value.
///
/// Note this deliberately does NOT disable proxy detection: a user behind a
/// corporate proxy needs reqwest's env/system proxy support to reach the
/// provider at all.
pub fn client() -> reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT
        .get_or_init(|| build_client(REQUEST_TIMEOUT, CONNECT_TIMEOUT))
        .clone()
}

/// Build a provider client with explicit timeouts. Split out from [`client`] so
/// tests can drive the same builder with short deadlines — if the `.timeout()`
/// wiring is ever dropped, `request_timeout_is_wired_and_classified_transient`
/// fails rather than every install silently regaining unbounded hangs.
///
/// Falls back to `Client::new()` if the builder fails (only possible on TLS
/// backend init failure): an un-timeouted client is strictly better than no
/// sync at all, and the failure is logged.
fn build_client(request_timeout: Duration, connect_timeout: Duration) -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(request_timeout)
        .connect_timeout(connect_timeout)
        .pool_idle_timeout(POOL_IDLE_TIMEOUT)
        .tcp_keepalive(TCP_KEEPALIVE)
        .build()
        .unwrap_or_else(|e| {
            tracing::error!(error = %e, "provider HTTP client builder failed - falling back to defaults (no timeouts)");
            reqwest::Client::new()
        })
}

/// A provider answered, but with a non-success status.
///
/// A typed error rather than a formatted string because the STATUS is what
/// decides transient vs terminal, and a `bail!("... → {status}: {body}")` throws
/// that away — leaving [`is_transient`] nothing to match on but prose.
#[derive(Debug)]
pub struct HttpStatusError {
    /// The HTTP status code the provider returned.
    pub status: u16,
    /// Response body, clamped to [`Self::BODY_CLAMP`]. Provider error bodies are
    /// echoed into a user-facing notice and into telemetry, so an unbounded
    /// body is both a UI and an egress problem.
    pub body: String,
    /// Human label for the endpoint, e.g. `"Jira /search/jql"`.
    endpoint: String,
}

impl HttpStatusError {
    /// Maximum retained response-body length.
    const BODY_CLAMP: usize = 200;

    /// Build from a status, endpoint label, and raw response body.
    pub fn new(status: u16, endpoint: impl Into<String>, body: impl AsRef<str>) -> Self {
        let body = body.as_ref();
        // Clamp on a char boundary — provider error bodies are arbitrary UTF-8
        // and slicing mid-codepoint would panic.
        let clamped: String = body.chars().take(Self::BODY_CLAMP).collect();
        Self {
            status,
            body: clamped,
            endpoint: endpoint.into(),
        }
    }

    /// Whether this status is worth retrying rather than reporting.
    ///
    /// `5xx` is server-side and by definition not something the user's
    /// credentials caused. `429` is rate limiting, `408`/`425` are the server
    /// asking for a retry. Everything else — notably `401`/`403` (dead or
    /// unscoped credentials) and `404` (a board that no longer exists) — is
    /// terminal and SHOULD reach the user, because nothing self-heals.
    pub fn is_transient(&self) -> bool {
        matches!(self.status, 408 | 425 | 429) || (500..=599).contains(&self.status)
    }
}

impl std::fmt::Display for HttpStatusError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} returned {}: {}",
            self.endpoint, self.status, self.body
        )
    }
}

impl std::error::Error for HttpStatusError {}

/// What a provider sync should DO about a failure.
///
/// Both variants carry `detail` already formatted with `{err:#}` — the full
/// `anyhow` source chain. That formatting lives here, in one place, on purpose:
/// with a plain `{err}` only the outermost `.context()` survives, so a connect
/// failure reached the user and the telemetry backend as the bare string
/// `"POST /graphql viewer"` with the real cause stripped. Four call sites each
/// doing their own `format!` could each regress that independently;
/// [`classify`] cannot.
#[derive(Debug)]
pub enum SyncFault {
    /// Retryable. Log it, keep the stale cache, raise NOTHING — the next tick
    /// will very likely succeed.
    Retry {
        /// Full `anyhow` chain, for the log line only.
        detail: String,
    },
    /// Terminal. Raise a user-facing notice; nothing here self-heals.
    Report {
        /// Full `anyhow` chain, for the log line and the notice body.
        detail: String,
    },
}

impl SyncFault {
    /// Force [`SyncFault::Retry`] for a failure a DIFFERENT classifier already
    /// judged retryable — specifically [`meridian_oauth::is_transient`], which
    /// understands token-endpoint failures this module cannot see. Keeps detail
    /// formatting inside this module so the guarantee above still holds.
    pub fn retry(err: &anyhow::Error) -> Self {
        Self::Retry {
            detail: format!("{err:#}"),
        }
    }
}

/// Decide what to do about a provider-sync failure, and format its detail.
///
/// This is the single entry point the provider sync paths should use; prefer it
/// over calling [`is_transient`] directly, so the chain formatting cannot drift.
pub fn classify(err: &anyhow::Error) -> SyncFault {
    if is_transient(err) {
        SyncFault::retry(err)
    } else {
        SyncFault::Report {
            detail: format!("{err:#}"),
        }
    }
}

/// Whether a provider-sync failure is worth retrying quietly rather than
/// raising a user-facing fault.
///
/// Walks the whole `anyhow` chain, so it still finds the cause under the
/// `.context()` layers the fetch paths add. **Defaults to `false`**: an error
/// this cannot classify surfaces to the user rather than being silently
/// swallowed, matching [`meridian_oauth::flow::is_transient`]'s stance.
pub fn is_transient(err: &anyhow::Error) -> bool {
    for cause in err.chain() {
        if let Some(status) = cause.downcast_ref::<HttpStatusError>() {
            return status.is_transient();
        }
        if let Some(e) = cause.downcast_ref::<reqwest::Error>() {
            return reqwest_is_transient(e);
        }
    }
    false
}

/// Classify a `reqwest` transport error.
///
/// `is_builder()` is checked FIRST and is always terminal: it means we failed to
/// construct the request, which in practice is a credential containing a byte
/// that is illegal in an HTTP header (a trailing `\r` from a hand-edited
/// `.env`). That never fixes itself by retrying, and it is precisely the case
/// where "check your token" is the correct remedy — so it must not be swallowed
/// as a network blip.
///
/// `is_decode()` and `is_body()` are deliberately absent: a response we could
/// not parse is a contract mismatch worth surfacing, not a retry.
fn reqwest_is_transient(e: &reqwest::Error) -> bool {
    if e.is_builder() {
        return false;
    }
    e.is_timeout() || e.is_connect() || e.is_request()
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── status classification ────────────────────────────────────────────────

    #[test]
    fn server_errors_and_rate_limits_are_transient() {
        for status in [500, 502, 503, 504, 599, 429, 408, 425] {
            let e = HttpStatusError::new(status, "GitHub GraphQL", "boom");
            assert!(e.is_transient(), "{status} should be transient");
        }
    }

    /// The other half of the contract, and the one that actually protects the
    /// user: a dead credential MUST still raise a notice. If this ever flips,
    /// a revoked token silently stops syncing forever with no indication.
    #[test]
    fn auth_and_client_errors_stay_terminal() {
        for status in [400, 401, 403, 404, 410, 422, 200, 302] {
            let e = HttpStatusError::new(status, "Jira /search/jql", "nope");
            assert!(!e.is_transient(), "{status} must stay terminal");
        }
    }

    #[test]
    fn status_error_is_found_through_context_layers() {
        let err = anyhow::Error::new(HttpStatusError::new(503, "Jira /search/jql", "down"))
            .context("POST /search/jql")
            .context("jira fetch failed");
        assert!(is_transient(&err), "chain walk must reach the status error");
    }

    #[test]
    fn terminal_status_is_found_through_context_layers() {
        let err = anyhow::Error::new(HttpStatusError::new(401, "GitHub GraphQL", "bad creds"))
            .context("POST /graphql viewer");
        assert!(!is_transient(&err));
    }

    /// Guards the clamp AND the char-boundary handling — a multi-byte body
    /// sliced by byte index would panic in production on any non-ASCII error
    /// page.
    #[test]
    fn body_is_clamped_on_a_char_boundary() {
        let long = "é".repeat(500);
        let e = HttpStatusError::new(500, "endpoint", &long);
        assert_eq!(e.body.chars().count(), HttpStatusError::BODY_CLAMP);
        assert!(e.to_string().contains("500"));
    }

    // ── the truncation regression ────────────────────────────────────────────

    /// THE regression guard. `format!("{e}")` on an `anyhow` error prints only
    /// the outermost `.context()`, so the real cause never reached the user or
    /// OpenObserve — a GitHub connect failure arrived as the bare, useless
    /// string "POST /graphql viewer". If anyone reverts [`classify`] to `{e}`,
    /// this fails.
    #[test]
    fn detail_carries_the_whole_cause_chain_not_just_the_outer_context() {
        let err = anyhow::Error::new(HttpStatusError::new(503, "Jira /search/jql", "maintenance"))
            .context("POST /search/jql");
        let SyncFault::Retry { detail } = classify(&err) else {
            panic!("503 must be classified Retry");
        };
        assert!(detail.contains("POST /search/jql"), "outer context lost");
        assert!(
            detail.contains("503"),
            "root-cause status lost - `{{e}}` truncation has regressed: {detail}"
        );
        assert!(detail.contains("maintenance"), "root-cause body lost");
    }

    /// Same guarantee on the terminal side: a user who IS being shown a banner
    /// must be shown the cause, not just the endpoint we happened to be calling.
    #[test]
    fn a_reported_fault_also_carries_the_whole_chain() {
        let err = anyhow::Error::new(HttpStatusError::new(
            401,
            "GitHub GraphQL viewer",
            "Bad credentials",
        ))
        .context("POST /graphql viewer");
        let SyncFault::Report { detail } = classify(&err) else {
            panic!("401 must be classified Report");
        };
        assert!(detail.contains("POST /graphql viewer"));
        assert!(detail.contains("401"), "status lost from the banner text");
        assert!(
            detail.contains("Bad credentials"),
            "cause lost from the banner text"
        );
    }

    /// The production symptom, end to end, with a REAL transport error rather
    /// than a hand-built chain: the reported bug was a banner whose entire body
    /// was the context string.
    #[tokio::test]
    async fn a_real_connect_failure_reports_more_than_its_context() {
        const CONTEXT: &str = "POST /graphql viewer";
        let err = connect_refused_error().await;
        let SyncFault::Retry { detail } = classify(&err) else {
            panic!("a refused connection must be classified Retry");
        };
        assert!(detail.contains(CONTEXT), "outer context lost: {detail}");
        assert!(
            detail.len() > CONTEXT.len() + 5,
            "detail is just the context - the cause chain was dropped: {detail}"
        );
    }

    /// `SyncFault::retry` is the escape hatch for `meridian_oauth`-classified
    /// failures; it must format identically or the Jira auth path silently
    /// regains the truncation.
    #[test]
    fn the_forced_retry_constructor_formats_the_same_chain() {
        let err = anyhow::Error::new(HttpStatusError::new(500, "token endpoint", "oops"))
            .context("refreshing jira token");
        let (SyncFault::Retry { detail } | SyncFault::Report { detail }) = SyncFault::retry(&err);
        assert!(detail.contains("refreshing jira token"));
        assert!(detail.contains("500"), "forced retry lost the cause chain");
    }

    // ── default stance ───────────────────────────────────────────────────────

    /// An error carrying neither a status nor a `reqwest::Error` must surface.
    /// This is what stops a future refactor from making every unclassified
    /// failure invisible.
    #[test]
    fn an_unclassified_error_is_terminal_by_default() {
        let err = anyhow::anyhow!("something went wrong").context("jira fetch failed");
        assert!(!is_transient(&err));
    }

    /// The regression this module exists to prevent: `meridian_oauth`'s
    /// classifier only recognises its own `TokenError`, so it answers `false`
    /// for every transport error. Wiring IT onto a fetch path would look like a
    /// fix and be a no-op. If someone ever "consolidates" the two, this fails.
    #[tokio::test]
    async fn the_oauth_classifier_does_not_cover_transport_errors() {
        let err = connect_refused_error().await;
        assert!(
            !meridian_oauth::is_transient(&err),
            "if this now returns true the two classifiers may be merged"
        );
        assert!(is_transient(&err), "ours must still classify it");
    }

    // ── real transport errors (offline: loopback only) ───────────────────────

    /// Produce a genuine `reqwest::Error` from a refused connection. Port 1 on
    /// loopback has no listener, so this needs no network and no fixture.
    async fn connect_refused_error() -> anyhow::Error {
        let err = client()
            .get("http://127.0.0.1:1/")
            .send()
            .await
            .expect_err("connecting to a closed loopback port must fail");
        anyhow::Error::new(err).context("POST /graphql viewer")
    }

    #[tokio::test]
    async fn a_refused_connection_is_transient() {
        let err = connect_refused_error().await;
        assert!(
            is_transient(&err),
            "connect failure must not raise a credentials banner: {err:#}"
        );
    }

    /// A credential with a byte that is illegal in a header value fails at
    /// request-BUILD time and surfaces from `.send()` looking exactly like a
    /// transport failure. It must stay terminal — this is the one case where
    /// "check your token" is the right advice.
    #[tokio::test]
    async fn a_malformed_credential_is_terminal_not_transient() {
        let err = client()
            .get("http://127.0.0.1:1/")
            // A trailing CR is what a hand-edited or CRLF `.env` produces.
            .header("Authorization", "Bearer ghp_token\r")
            .send()
            .await
            .expect_err("an invalid header value must fail the request");
        assert!(err.is_builder(), "expected a builder error, got: {err}");
        let err = anyhow::Error::new(err).context("POST /graphql viewer");
        assert!(
            !is_transient(&err),
            "a malformed credential must still reach the user"
        );
    }

    /// Proves `.timeout()` is actually wired into [`build_client`]: the peer
    /// accepts the TCP connection and then says nothing, so only a request
    /// timeout can end this. Without the timeout the test hangs rather than
    /// failing, which is itself the signal.
    #[tokio::test]
    async fn request_timeout_is_wired_and_classified_transient() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind loopback");
        let addr = listener.local_addr().expect("local addr");
        // Accept and hold the socket open without ever responding.
        tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((stream, _)) = listener.accept().await {
                held.push(stream);
            }
        });

        let short = Duration::from_millis(150);
        let err = build_client(short, short)
            .get(format!("http://{addr}/"))
            .send()
            .await
            .expect_err("a peer that never responds must time out");
        assert!(err.is_timeout(), "expected a timeout, got: {err}");

        let err = anyhow::Error::new(err).context("POST /search/jql");
        assert!(
            is_transient(&err),
            "a timeout must be retried, not reported"
        );
    }

    /// The timeouts must stay bounded and ordered. A zero value disables the
    /// timeout in reqwest, which would silently restore the unbounded-hang bug.
    #[test]
    fn timeout_constants_are_bounded_and_ordered() {
        assert!(!REQUEST_TIMEOUT.is_zero(), "zero disables the timeout");
        assert!(!CONNECT_TIMEOUT.is_zero(), "zero disables the timeout");
        assert!(
            CONNECT_TIMEOUT < REQUEST_TIMEOUT,
            "connect deadline must fit inside the total deadline"
        );
        assert!(
            REQUEST_TIMEOUT <= Duration::from_secs(120),
            "an unbounded-in-practice timeout defeats the purpose"
        );
    }
}
