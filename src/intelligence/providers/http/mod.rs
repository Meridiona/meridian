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
//! All five PM connectors, at both their client construction and their sync
//! failure paths:
//! - [`crate::intelligence::providers::github`] — `refresh_if_stale` (viewer
//!   fetch + all-projects-failed) and `fetch::{fetch_viewer_login, fetch_project_items}`.
//! - [`crate::intelligence::providers::jira`] — `refresh_if_stale` (auth
//!   resolve + fetch failure) and `fetch::search`.
//! - [`crate::intelligence::providers::linear`] — `refresh_if_stale` and its
//!   GraphQL fetch.
//! - [`crate::intelligence::providers::trello`] — `refresh_if_stale` and its
//!   cards fetch.
//! - [`crate::intelligence::providers::azure_devops`] — `force_refresh` (WIQL
//!   failure) and `fetch::{run_wiql, fetch_batch}`.
//!
//! # Related
//! - [`crate::intelligence::providers::stamp_sync_error`] — what we call only
//!   once a failure is classified terminal.
//! - [`meridian_oauth::flow::is_transient`] — the OAuth-token-endpoint sibling.

use std::sync::OnceLock;
use std::time::Duration;

mod classify;
pub use classify::{chain, classify, is_transient, HttpStatusError, SyncFault, TruncatedBodyError};

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

/// Read a provider response body, rejecting a non-success status first and an
/// incomplete transfer second.
///
/// Every provider fetch path repeats this dance — capture the status, read the
/// body, turn a non-2xx into a typed [`HttpStatusError`] — and each hand-rolled
/// copy is a place the classification can silently regress. `endpoint` is the
/// human label both typed errors carry.
///
/// # Why the body read is not `unwrap_or_default()`
/// That is what every call site did, and it is a silent evidence-destroying
/// step: a reset connection, an HTTP/2 GOAWAY, or the request deadline firing
/// mid-body all produce a `reqwest::Error` that was replaced with `""`, which
/// then reached `serde_json` and failed as "EOF while parsing a value at line 1
/// column 0". By then nothing in the chain identified the failure as a network
/// one, so [`classify`] reported it terminally and the user was told to
/// reconnect a working GitHub token. The error is preserved as a
/// [`TruncatedBodyError`] instead — and an empty body on a 2xx gets the same
/// treatment, because a provider that returns no bytes at all has not answered.
///
/// **On a non-2xx the body read is still best-effort.** The status is what
/// classifies there, and a status error with an unread body is strictly better
/// than losing the status to a body-read failure.
///
/// # Only for endpoints that MUST return a body
/// The empty check treats no bytes as a failure, so this is wrong for anything
/// that can legitimately answer `204 No Content` — which several of the
/// `ticket_update` write paths can. Those need a variant that tolerates an
/// empty success; adopt this one only on reads that always expect a payload.
///
/// # Who calls this
/// [`crate::intelligence::providers::github::fetch`]'s viewer and project-item
/// walks. The other provider fetch paths still inline the same steps, complete
/// with `unwrap_or_default()`, and should adopt this.
pub async fn read_success_body(resp: reqwest::Response, endpoint: &str) -> anyhow::Result<String> {
    let status = resp.status();
    let code = status.as_u16();

    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(HttpStatusError::new(code, endpoint, &body).into());
    }

    let text = resp
        .text()
        .await
        .map_err(|e| TruncatedBodyError::unreadable(code, endpoint, e))?;
    if text.is_empty() {
        return Err(TruncatedBodyError::empty(code, endpoint).into());
    }
    Ok(text)
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

    /// Serve one canned HTTP/1.1 response on loopback and return its URL.
    ///
    /// Deliberately raw bytes rather than a server crate: the cases under test
    /// are malformed-at-the-wire (a body that stops short of its declared
    /// `Content-Length`), which a well-behaved server will not produce.
    ///
    /// # The request MUST be drained before answering
    /// Closing a socket that still has unread bytes in its receive buffer is an
    /// ABORTIVE close on Windows — the peer gets an RST and the client fails
    /// with `WSAECONNABORTED (10053)` at `.send()`, before it can read a single
    /// byte of the canned response. POSIX is more forgiving and delivers the
    /// buffered response first, so an undrained fixture passes on macOS/Linux
    /// and fails only on the Windows runner. Reading the request first, then
    /// shutting down gracefully, is what makes the close a FIN on every
    /// platform — and a FIN is precisely what the truncated case needs, since
    /// an RST would fail the whole request instead of cutting the body short.
    async fn serve_once(raw: &'static [u8]) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind loopback");
        let addr = listener.local_addr().expect("local addr");
        tokio::spawn(async move {
            if let Ok((mut stream, _)) = listener.accept().await {
                use tokio::io::{AsyncReadExt, AsyncWriteExt};
                // Drain the request. These are single small GETs, so stop at the
                // end of the headers rather than waiting for a close the client
                // has no reason to send.
                let mut buf = [0u8; 4096];
                while let Ok(n) = stream.read(&mut buf).await {
                    if n == 0 || buf[..n].windows(4).any(|w| w == b"\r\n\r\n") {
                        break;
                    }
                }
                let _ = stream.write_all(raw).await;
                let _ = stream.flush().await;
                // Graceful FIN rather than a drop — see the note above.
                let _ = stream.shutdown().await;
            }
        });
        format!("http://{addr}/")
    }

    /// THE regression. GitHub answered 200 with nothing in it, and the sync
    /// raised a terminal "Reconnect GitHub in Settings - Integrations" banner
    /// for a credential that was never broken.
    ///
    /// The empty body reached `serde_json::from_str`, which failed with "EOF
    /// while parsing a value at line 1 column 0" — an error chain carrying
    /// neither a status nor a transport error, so [`is_transient`] fell through
    /// to its `false` default and [`classify`] reported it. An empty 2xx is a
    /// truncated transfer; it must retry.
    #[tokio::test]
    async fn an_empty_body_on_success_is_transient() {
        let url = serve_once(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n").await;
        let resp = client().get(&url).send().await.expect("request");
        let err = read_success_body(resp, "GitHub GraphQL")
            .await
            .expect_err("an empty 200 body must not be handed back as success");
        assert!(
            is_transient(&err),
            "an empty 200 must retry, not raise a credentials banner: {err:#}"
        );
    }

    /// The other half, and the one a plain `?` does not fix: the connection
    /// dies part-way through the body. `reqwest` reports that as `Kind::Decode`,
    /// which `classify::reqwest_is_transient` excludes ON PURPOSE — a
    /// body we cannot PARSE is a contract mismatch worth surfacing. A body we
    /// could not READ is not; it is the same truncated transfer as above.
    #[tokio::test]
    async fn a_truncated_body_on_success_is_transient() {
        let url = serve_once(b"HTTP/1.1 200 OK\r\nContent-Length: 4096\r\n\r\n{\"data\":").await;
        let resp = client().get(&url).send().await.expect("request");
        let err = read_success_body(resp, "GitHub GraphQL")
            .await
            .expect_err("a body that stops short of Content-Length must fail");
        assert!(
            is_transient(&err),
            "a truncated body read must retry: {err:#}"
        );
    }

    /// The status path must be untouched by the above: a dead credential still
    /// has to reach the user, and its body still has to reach the banner.
    #[tokio::test]
    async fn a_non_success_status_still_reports_with_its_body() {
        let url =
            serve_once(b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 15\r\n\r\nBad credentials")
                .await;
        let resp = client().get(&url).send().await.expect("request");
        let err = read_success_body(resp, "GitHub GraphQL viewer")
            .await
            .expect_err("a 401 must not be handed back as success");
        assert!(!is_transient(&err), "a 401 must stay terminal: {err:#}");
        let detail = chain(&err);
        assert!(detail.contains("401"), "status lost: {detail}");
        assert!(detail.contains("Bad credentials"), "body lost: {detail}");
    }

    /// An ordinary 200 is still just its body — the empty/truncated guards must
    /// not start rejecting healthy responses.
    #[tokio::test]
    async fn a_normal_success_body_is_returned_verbatim() {
        let url =
            serve_once(b"HTTP/1.1 200 OK\r\nContent-Length: 16\r\n\r\n{\"data\":{\"x\":1}}").await;
        let resp = client().get(&url).send().await.expect("request");
        let body = read_success_body(resp, "GitHub GraphQL")
            .await
            .expect("a healthy 200 must be returned");
        assert_eq!(body, "{\"data\":{\"x\":1}}");
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
