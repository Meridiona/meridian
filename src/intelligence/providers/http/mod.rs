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
//! # Silence is bounded, deliberately
//! Suppressing a transient failure outright would trade one bug for a worse
//! one: a provider blocked by a corporate proxy or a TLS-intercepting firewall
//! is unreachable *persistently*, and the user would get no signal at all while
//! their board quietly went stale — strictly worse than the wrong-but-visible
//! banner they get today. So [`SyncFault::Retry`] is not "stay silent", it is
//! "ask [`crate::intelligence::providers::note_transient_sync_failure`]", which
//! escalates once the last successful sync is old enough that "blip" is no
//! longer a credible explanation. No migration needed:
//! `pm_sync_state.last_synced_at` already records exactly that.
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

mod classify;
pub use classify::{chain, classify, is_transient, HttpStatusError, SyncFault};

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

#[cfg(test)]
mod tests {
    use super::*;

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
