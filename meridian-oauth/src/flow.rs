//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Generic OAuth 2.0 Authorization Code + PKCE engine, shared by the PM providers
// (Jira now, Linear next). It opens the system browser to the provider's consent
// screen, captures the redirect on a fixed loopback port (Atlassian requires an
// exact-match redirect URI, so the port is fixed, not ephemeral), exchanges the
// code for tokens, and refreshes rotating tokens. Provider-specific URLs/scopes
// are supplied via `ProviderSpec`; everything else here is provider-blind.

use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use serde::Deserialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use crate::pkce;

/// Provider-specific OAuth endpoints and scopes. The flow engine is otherwise
/// generic over these.
pub struct ProviderSpec {
    pub authorize_url: &'static str,
    pub token_url: &'static str,
    /// Space-separated scope string (already including `offline_access` where the
    /// provider needs it for a refresh token).
    pub scopes: &'static str,
    /// Extra `/authorize` query params beyond the standard set (e.g. Atlassian's
    /// `audience` and `prompt`).
    pub extra_authorize_params: Vec<(&'static str, String)>,
    /// Confidential-client secret for providers that require one at the token
    /// endpoint. Atlassian Cloud's 3LO token exchange ignores PKCE and demands a
    /// `client_secret` even for desktop apps, so we send it when present. `None`
    /// for true public clients (where PKCE alone authenticates the exchange).
    pub client_secret: Option<String>,
}

/// The token-endpoint response shared by authorization-code exchange and refresh.
#[derive(Debug, Clone, Deserialize)]
pub struct TokenResponse {
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: String,
    /// Lifetime in seconds of `access_token`. Some providers omit the field;
    /// default 0 means expires_at = now, triggering a refresh on next use.
    #[serde(default)]
    pub expires_in: i64,
    #[serde(default)]
    pub scope: String,
}

/// Whether a token-endpoint failure is worth retrying, or means the grant is
/// dead. This is the distinction the background refresh loop needs: a passing
/// network blip must not be surfaced to the user as "re-authenticate".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenFailure {
    /// The grant itself was rejected and won't recover on its own — an OAuth 4xx
    /// (`invalid_grant`, a revoked/expired refresh token, a bad client). The
    /// stored refresh token is dead; the user must re-authenticate.
    Terminal,
    /// A passing condition — a network error, request timeout, HTTP 429, or a
    /// 5xx from the provider. The stored refresh token is still valid and a
    /// later retry will succeed.
    Transient,
}

/// A classified token-endpoint error. It is threaded into the `anyhow` chain
/// (via `?`/`.context`) so the sync loop can tell "re-authenticate" from "try
/// again later" without string-matching messages — see [`is_transient`].
#[derive(Debug)]
pub struct TokenError {
    pub failure: TokenFailure,
    detail: String,
}

impl TokenError {
    fn transient(detail: impl Into<String>) -> Self {
        Self {
            failure: TokenFailure::Transient,
            detail: detail.into(),
        }
    }
    fn terminal(detail: impl Into<String>) -> Self {
        Self {
            failure: TokenFailure::Terminal,
            detail: detail.into(),
        }
    }
    fn is_transient(&self) -> bool {
        matches!(self.failure, TokenFailure::Transient)
    }
}

impl std::fmt::Display for TokenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.detail)
    }
}

impl std::error::Error for TokenError {}

/// Whether an `anyhow` error from the OAuth flow was a *transient* token-endpoint
/// failure (retryable — network/timeout/429/5xx) rather than a dead grant.
/// Walks the whole error chain, so it still finds the [`TokenError`] under the
/// `.context()` layers `refresh`/`ensure_fresh` add. Defaults to `false` when no
/// `TokenError` is present, so an unclassified failure still surfaces the
/// re-authenticate remedy rather than being silently swallowed.
pub fn is_transient(err: &anyhow::Error) -> bool {
    err.chain()
        .find_map(|cause| cause.downcast_ref::<TokenError>())
        .is_some_and(TokenError::is_transient)
}

/// How long to wait for the user to complete the browser consent before giving up.
const CONSENT_TIMEOUT: Duration = Duration::from_secs(300);

/// Run the full interactive Authorization Code + PKCE flow on `redirect_port`.
/// Blocks (async) until the browser redirect arrives or `CONSENT_TIMEOUT` elapses.
pub async fn run_authcode_flow(
    client_id: &str,
    spec: &ProviderSpec,
    redirect_port: u16,
) -> Result<TokenResponse> {
    let redirect_uri = format!("http://127.0.0.1:{redirect_port}/callback");
    let listener = TcpListener::bind(("127.0.0.1", redirect_port))
        .await
        .with_context(|| {
            format!("binding loopback :{redirect_port} for the OAuth redirect — is the port free?")
        })?;

    let pkce = pkce::generate();
    let authorize = build_authorize_url(
        client_id,
        spec.authorize_url,
        spec.scopes,
        &spec.extra_authorize_params,
        &redirect_uri,
        &pkce,
    );

    tracing::info!("opening browser to authorize OAuth flow");
    open_browser(&authorize);

    let (code, returned_state) = tokio::time::timeout(CONSENT_TIMEOUT, accept_redirect(&listener))
        .await
        .map_err(|_| anyhow!("timed out after 5 min waiting for browser authorization"))??;

    if returned_state != pkce.state {
        bail!("OAuth state mismatch — possible CSRF; aborting");
    }

    exchange_code(client_id, spec, &redirect_uri, &code, &pkce.verifier).await
}

/// Exchange a rotating refresh token for a fresh access/refresh pair.
pub async fn refresh(
    client_id: &str,
    spec: &ProviderSpec,
    refresh_token: &str,
) -> Result<TokenResponse> {
    let mut body = serde_json::json!({
        "grant_type": "refresh_token",
        "client_id": client_id,
        "refresh_token": refresh_token,
    });
    with_client_secret(&mut body, spec);
    post_token_retrying(spec.token_url, &body)
        .await
        .context("refreshing OAuth access token")
}

// ---------------------------------------------------------------------------
// Internals
// ---------------------------------------------------------------------------

fn build_authorize_url(
    client_id: &str,
    authorize_url: &str,
    scopes: &str,
    extra_params: &[(&'static str, String)],
    redirect_uri: &str,
    pkce: &pkce::Pkce,
) -> String {
    let mut params: Vec<(&str, String)> = vec![
        ("response_type", "code".to_string()),
        ("client_id", client_id.to_string()),
        ("redirect_uri", redirect_uri.to_string()),
        ("scope", scopes.to_string()),
        ("state", pkce.state.clone()),
        ("code_challenge", pkce.challenge.clone()),
        ("code_challenge_method", "S256".to_string()),
    ];
    for (k, v) in extra_params {
        params.push((k, v.clone()));
    }
    let query = params
        .iter()
        .map(|(k, v)| format!("{}={}", encode(k), encode(v)))
        .collect::<Vec<_>>()
        .join("&");
    format!("{}?{}", authorize_url, query)
}

async fn exchange_code(
    client_id: &str,
    spec: &ProviderSpec,
    redirect_uri: &str,
    code: &str,
    verifier: &str,
) -> Result<TokenResponse> {
    let mut body = serde_json::json!({
        "grant_type": "authorization_code",
        "client_id": client_id,
        "code": code,
        "redirect_uri": redirect_uri,
        "code_verifier": verifier,
    });
    with_client_secret(&mut body, spec);
    post_token_retrying(spec.token_url, &body)
        .await
        .context("exchanging authorization code for tokens")
}

/// Inject `client_secret` into a token-endpoint body when the provider is a
/// confidential client (Atlassian Cloud). No-op for true public clients.
fn with_client_secret(body: &mut serde_json::Value, spec: &ProviderSpec) {
    if let Some(secret) = spec.client_secret.as_deref() {
        if !secret.trim().is_empty() {
            body["client_secret"] = serde_json::Value::String(secret.to_string());
        }
    }
}

/// Total tries [`post_token_retrying`] gives the token endpoint before it gives
/// up on a *transient* failure. Three attempts across ~1.6 s of backoff ride out
/// the momentary network blips and provider 5xx/429s that otherwise surfaced a
/// spurious "re-authenticate" sync error on the very next 30-min poll. A dead
/// grant (4xx) still fails on the first try — retries are spent only on failures
/// that can actually clear.
const TOKEN_POST_ATTEMPTS: usize = 3;

/// Bound an untrusted response body before it goes into a `TokenError` detail
/// (which is then logged, verbatim, on every retry). A captive-portal or proxy
/// interstitial can be an arbitrarily large HTML page; cap it so it can't bloat
/// structured logs. Truncates on a char boundary so a multi-byte sequence is
/// never split.
fn truncate_body(text: &str) -> String {
    const MAX: usize = 300;
    if text.len() <= MAX {
        return text.to_string();
    }
    let mut end = MAX;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{} (truncated, {} bytes total)", &text[..end], text.len())
}

/// Wall-clock ceiling for ONE token-endpoint attempt, in seconds.
///
/// Sits alongside `reqwest`'s own 8 s `.timeout()` rather than replacing it, and
/// the two measure different things on purpose.
const WALL_CLOCK_BUDGET_SECS: i64 = 20;

/// Run `fut`, abandoning it if the WALL CLOCK advances past `budget_secs`.
///
/// # Why `reqwest`'s own timeout is not enough
///
/// Every `.timeout()` in this codebase - and every `tokio::time` primitive -
/// measures `Instant`, which on macOS is `mach_absolute_time` and DOES NOT ADVANCE
/// while the system is asleep. So a request in flight when the lid closes is not
/// cancelled by its 8 s timeout: it hangs for the entire suspend, and on wake the
/// retry goes out with a refresh token that the provider rotated (and stopped
/// honouring) half an hour ago. That is the exact sequence that killed production
/// grants; see [`crate::refresh_journal`]'s header for the measured case.
///
/// The watchdog below still SLEEPS on the monotonic clock - it has to, that is the
/// only timer available - but every time it wakes it compares the WALL clock. A
/// suspend freezes both clocks, so nothing fires mid-suspend; the instant the
/// machine resumes, the first tick sees the wall clock has jumped and gives up
/// immediately. Abandoning promptly on wake is the whole point: it is what leaves
/// enough of the provider's reuse grace to replay the spend and recover the grant,
/// instead of discovering the loss when the window has already closed.
///
/// Abandoning is classified TRANSIENT: an unanswered request says nothing about
/// whether the grant is valid, and treating it as terminal is what turned a
/// suspend into "re-authenticate".
async fn with_wall_clock_deadline<F, T>(budget_secs: i64, fut: F) -> Result<T, TokenError>
where
    F: std::future::Future<Output = Result<T, TokenError>>,
{
    let started = wall_clock_unix();
    // Boxed so the future can be polled repeatedly across successive `timeout`
    // slices: `timeout` takes its future by value, but `&mut Pin<Box<F>>` is
    // itself a future, so borrowing it preserves the in-flight request between
    // slices instead of restarting it.
    let mut fut = Box::pin(fut);
    loop {
        // The MONOTONIC slice is just a heartbeat, not the deadline. Deliberately
        // short so that the wall-clock check below runs promptly after a resume.
        match tokio::time::timeout(Duration::from_millis(500), &mut fut).await {
            // The request answered. Checked before the clock on purpose: a
            // response already in hand must never be discarded, because the
            // refresh token it cost has already been spent.
            Ok(out) => return out,
            Err(_) => {
                let elapsed = wall_clock_unix().saturating_sub(started);
                if elapsed > budget_secs {
                    return Err(TokenError::transient(format!(
                        "token endpoint did not answer within {elapsed}s of wall-clock \
                         time (a system suspend, or a stalled connection); abandoning \
                         so the spend can be replayed while the provider still \
                         honours the previous token"
                    )));
                }
            }
        }
    }
}

/// Wall-clock unix seconds. `0` if the clock is before the epoch, which only
/// happens on a badly misconfigured machine and would otherwise panic.
fn wall_clock_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// [`post_token`] with bounded retry-with-backoff for transient failures. Only
/// [`TokenFailure::Transient`] errors are retried; a terminal rejection returns
/// immediately so a genuinely dead token isn't hidden behind seconds of backoff.
#[tracing::instrument(skip(body), fields(token_url = %token_url))]
async fn post_token_retrying(
    token_url: &str,
    body: &serde_json::Value,
) -> Result<TokenResponse, TokenError> {
    // One client for the whole retry loop: rebuilding it per attempt would throw
    // away connection pooling/keep-alive, which is exactly what a retry wants.
    // The 8 s timeout is per request (each `send`), not a total budget.
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(8))
        .build()
        .map_err(|e| TokenError::transient(format!("building HTTP client: {e}")))?;
    let mut backoff = Duration::from_millis(400);
    for attempt in 1..=TOKEN_POST_ATTEMPTS {
        match with_wall_clock_deadline(WALL_CLOCK_BUDGET_SECS, post_token(&client, token_url, body))
            .await
        {
            Ok(resp) => return Ok(resp),
            Err(e) if e.is_transient() && attempt < TOKEN_POST_ATTEMPTS => {
                tracing::warn!(
                    attempt,
                    max = TOKEN_POST_ATTEMPTS,
                    error = %e,
                    "OAuth token endpoint transient failure - retrying after backoff"
                );
                tokio::time::sleep(backoff).await;
                backoff *= 3; // 400ms → 1200ms
            }
            Err(e) => return Err(e),
        }
    }
    // The `attempt < TOKEN_POST_ATTEMPTS` guard means the final attempt always
    // takes a return arm above.
    unreachable!("post_token_retrying loop returns on the last attempt")
}

/// POST a grant to the token endpoint once and classify any failure as transient
/// or terminal (see [`TokenError`]). Takes a shared `client` (built once by
/// [`post_token_retrying`]) whose 8 s timeout applies per attempt.
async fn post_token(
    client: &reqwest::Client,
    token_url: &str,
    body: &serde_json::Value,
) -> Result<TokenResponse, TokenError> {
    let resp = match client
        .post(token_url)
        .header("Accept", "application/json")
        .json(body)
        .send()
        .await
    {
        Ok(r) => r,
        // Connect refused, DNS failure, TLS error, or the 8 s timeout — the
        // endpoint was unreachable, which says nothing about the token's validity.
        Err(e) => return Err(TokenError::transient(format!("POST {token_url}: {e}"))),
    };
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        let detail = format!(
            "token endpoint {token_url} → {status}: {}",
            truncate_body(&text)
        );
        // 429 (rate limited) and 5xx are the provider briefly faltering — retry.
        // Every other non-2xx is a real rejection: a 4xx `invalid_grant` / bad
        // client / revoked-or-rotated refresh token the user must act on.
        return Err(if status.as_u16() == 429 || status.is_server_error() {
            TokenError::transient(detail)
        } else {
            TokenError::terminal(detail)
        });
    }
    // A 2xx whose body isn't valid JSON is almost always a proxy or captive-portal
    // interstitial standing in for the real payload, not a malformed token grant —
    // treat it as transient so a later retry (off the hostile network) can succeed.
    serde_json::from_str(&text).map_err(|e| {
        TokenError::transient(format!(
            "parsing token response: {e}: {}",
            truncate_body(&text)
        ))
    })
}

/// Accept exactly one inbound connection, parse the `GET /callback?...` request
/// line, return `(code, state)`, and reply with a friendly close-this-tab page.
async fn accept_redirect(listener: &TcpListener) -> Result<(String, String)> {
    loop {
        let (mut socket, _) = listener
            .accept()
            .await
            .context("accepting OAuth redirect")?;
        let mut buf = vec![0u8; 8192];
        let n = socket
            .read(&mut buf)
            .await
            .context("reading redirect request")?;
        let req = String::from_utf8_lossy(&buf[..n]);
        let Some(first_line) = req.lines().next() else {
            continue;
        };
        // "GET /callback?code=...&state=... HTTP/1.1"
        let Some(target) = first_line.split_whitespace().nth(1) else {
            continue;
        };
        // Ignore non-callback probes (e.g. /favicon.ico) — keep listening.
        let Some(query) = target.split('?').nth(1) else {
            let _ = respond(&mut socket, "Waiting for authorization…").await;
            continue;
        };

        let mut code = None;
        let mut state = None;
        let mut error = None;
        for pair in query.split('&') {
            let mut it = pair.splitn(2, '=');
            match (it.next(), it.next()) {
                (Some("code"), Some(v)) => code = Some(decode(v)),
                (Some("state"), Some(v)) => state = Some(decode(v)),
                (Some("error"), Some(v)) => error = Some(decode(v)),
                _ => {}
            }
        }

        if let Some(err) = error {
            let _ = respond(&mut socket, "Authorization failed. You can close this tab.").await;
            bail!("provider returned OAuth error: {err}");
        }
        match (code, state) {
            (Some(c), Some(s)) => {
                let _ = respond(
                    &mut socket,
                    "Meridian is now connected. You can close this tab.",
                )
                .await;
                return Ok((c, s));
            }
            _ => {
                let _ = respond(&mut socket, "Missing code/state. You can close this tab.").await;
                bail!("redirect missing code or state");
            }
        }
    }
}

async fn respond(socket: &mut tokio::net::TcpStream, message: &str) -> Result<()> {
    let html = format!(
        "<!doctype html><html><head><meta charset=utf-8><title>Meridian</title></head>\
         <body style=\"font-family:system-ui;text-align:center;padding-top:4rem\">\
         <h2>{message}</h2></body></html>"
    );
    respond_raw(socket, &html).await
}

async fn respond_raw(socket: &mut tokio::net::TcpStream, html: &str) -> Result<()> {
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        html.len(),
        html
    );
    socket.write_all(response.as_bytes()).await?;
    socket.flush().await?;
    Ok(())
}

/// Run the Trello fragment-relay flow. Trello delivers the token in the URL
/// fragment (`#token=...`) which the HTTP server cannot read directly. This
/// serves a small JS relay page at `/callback` that reads the hash and fetches
/// `/capture?t=TOKEN`, which the server captures.
pub async fn run_fragment_relay_flow(authorize_url: &str, port: u16) -> Result<String> {
    let listener = TcpListener::bind(("127.0.0.1", port))
        .await
        .with_context(|| {
            format!("binding loopback :{port} for the Trello token relay — is the port free?")
        })?;
    tracing::info!("opening browser to authorize Trello OAuth flow");
    open_browser(authorize_url);
    tokio::time::timeout(CONSENT_TIMEOUT, accept_fragment_relay(&listener))
        .await
        .map_err(|_| anyhow!("timed out after 5 min waiting for browser authorization"))?
}

/// Accept the two-request fragment relay sequence:
///   1. GET /callback          → serve JS relay page (reads hash, fetches /capture)
///   2. GET /capture?t=TOKEN   → extract token, confirm success, return token
async fn accept_fragment_relay(listener: &TcpListener) -> Result<String> {
    loop {
        let (mut socket, _) = listener
            .accept()
            .await
            .context("accepting Trello relay connection")?;
        let mut buf = vec![0u8; 8192];
        let n = socket
            .read(&mut buf)
            .await
            .context("reading Trello relay request")?;
        let req = String::from_utf8_lossy(&buf[..n]);
        let Some(first_line) = req.lines().next() else {
            continue;
        };
        let Some(target) = first_line.split_whitespace().nth(1) else {
            continue;
        };

        if target.starts_with("/capture") {
            // Second request: JS relayed the token as ?t=TOKEN
            let token = target.split('?').nth(1).and_then(|q| {
                q.split('&').find_map(|pair| {
                    let mut it = pair.splitn(2, '=');
                    match (it.next(), it.next()) {
                        (Some("t"), Some(v)) => Some(decode(v)),
                        _ => None,
                    }
                })
            });
            match token {
                Some(t) if !t.is_empty() => {
                    let _ = respond(&mut socket, "Trello connected! You can close this tab.").await;
                    return Ok(t);
                }
                _ => {
                    let _ = respond(&mut socket, "Token missing. You can close this tab.").await;
                    bail!("Trello relay /capture received no token");
                }
            }
        } else if target.starts_with("/callback") {
            // First request: serve the JS relay page that reads the URL fragment
            // and fetches /capture?t={token}. The fragment is never sent to the
            // server by the browser, so JS must relay it.
            let relay_html = "\
<!doctype html><html><head><meta charset=utf-8><title>Meridian</title></head>\
<body style=\"font-family:system-ui;text-align:center;padding-top:4rem\">\
<h2>Connecting Trello\u{2026}</h2>\
<script>\
var h=window.location.hash;\
var t=h&&h.startsWith('#token=')?h.slice(7):'';\
if(t){fetch('/capture?t='+encodeURIComponent(t)).then(function(){document.querySelector('h2').textContent='Trello connected! You can close this tab.';});}\
else{document.querySelector('h2').textContent='No token in URL. Try again.';}\
</script></body></html>";
            let _ = respond_raw(&mut socket, relay_html).await;
            // Keep listening — the JS relay will arrive on the next connection.
        } else {
            // Ignore stray probes (favicon, etc.)
            let _ = respond(&mut socket, "Waiting for authorization\u{2026}").await;
        }
    }
}

/// Open `url` in the system browser. Non-fatal if the launch fails — the URL
/// is always logged too, so a failed/headless launch still leaves the user a
/// way to continue by pasting it manually.
///
/// Windows launches via `rundll32.exe url.dll,FileProtocolHandler`, not
/// `cmd /C start` or `explorer.exe`. The authorize URLs passed here always carry
/// `&`-separated query params (client_id, redirect_uri, state, scope), and
/// `cmd.exe` re-parses its whole command line for shell metacharacters like `&`
/// regardless of Win32 argv quoting — it would split the URL at the first `&` and
/// run the tail as a second command. `explorer.exe` avoids that shell re-parsing
/// but is unreliable for these long OAuth URLs: when it fails to parse the
/// argument as a URL it silently falls back to opening a file window (the user's
/// Documents folder) instead of the browser. `rundll32.exe
/// url.dll,FileProtocolHandler` hands the URL straight to the registered protocol
/// handler (ShellExecute "open") as a single literal argv argument — no shell
/// re-parsing — and reliably opens the default browser with the full query string
/// intact.
fn open_browser(url: &str) {
    tracing::info!(
        url,
        "if it doesn't open automatically, open this URL yourself"
    );
    #[cfg(target_os = "windows")]
    let result = std::process::Command::new("rundll32")
        .arg("url.dll,FileProtocolHandler")
        .arg(url)
        .spawn();
    #[cfg(target_os = "macos")]
    let result = std::process::Command::new("open").arg(url).spawn();
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let result = std::process::Command::new("xdg-open").arg(url).spawn();
    if let Err(e) = result {
        tracing::warn!(error = %e, "failed to launch the system browser");
    }
}

/// RFC 3986 percent-encoding for a query-component value (unreserved chars pass
/// through; everything else is `%XX`).
fn encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Decode a percent-encoded query value (also turning `+` into a space).
fn decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hi = (bytes[i + 1] as char).to_digit(16);
                let lo = (bytes[i + 2] as char).to_digit(16);
                if let (Some(h), Some(l)) = (hi, lo) {
                    out.push((h * 16 + l) as u8);
                    i += 3;
                    continue;
                }
                out.push(b'%');
                i += 1;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `is_transient` must find the [`TokenError`] even under the `.context()`
    /// layers `refresh`/`ensure_fresh` wrap it in — that chain walk is the whole
    /// point (a top-level-only downcast would miss it and every refresh failure
    /// would read as terminal again). Terminal errors, and errors carrying no
    /// `TokenError` at all, must both report `false`.
    #[test]
    fn is_transient_classifies_through_context_layers() {
        let transient = anyhow::Error::new(TokenError::transient("502 bad gateway"))
            .context("refreshing OAuth access token");
        assert!(is_transient(&transient));

        let terminal = anyhow::Error::new(TokenError::terminal("invalid_grant"))
            .context("refreshing OAuth access token");
        assert!(!is_transient(&terminal));

        // No TokenError in the chain → default to terminal (surface the remedy).
        let unclassified = anyhow::anyhow!("disk error").context("loading token store");
        assert!(!is_transient(&unclassified));
    }

    fn spec() -> ProviderSpec {
        ProviderSpec {
            authorize_url: "https://auth.example.com/authorize",
            token_url: "https://auth.example.com/token",
            scopes: "read:jira-work offline_access",
            extra_authorize_params: vec![("audience", "api.atlassian.com".to_string())],
            client_secret: None,
        }
    }

    #[test]
    fn encode_handles_reserved_chars() {
        assert_eq!(encode("a b"), "a%20b");
        assert_eq!(encode("read:jira-work x"), "read%3Ajira-work%20x");
        assert_eq!(
            encode("http://127.0.0.1:9123/callback"),
            "http%3A%2F%2F127.0.0.1%3A9123%2Fcallback"
        );
        assert_eq!(encode("AZaz09-_.~"), "AZaz09-_.~");
    }

    #[test]
    fn decode_inverts_encode_for_codes() {
        assert_eq!(decode("a%20b"), "a b");
        assert_eq!(decode("abc-_123"), "abc-_123");
        assert_eq!(decode("x%2Fy"), "x/y");
    }

    #[test]
    fn with_client_secret_injects_when_present() {
        let mut body = serde_json::json!({ "grant_type": "authorization_code" });
        let mut s = spec();
        s.client_secret = Some("sek".to_string());
        with_client_secret(&mut body, &s);
        assert_eq!(body["client_secret"], "sek");
    }

    #[test]
    fn with_client_secret_skips_when_absent_or_blank() {
        // None → no field added.
        let mut body = serde_json::json!({ "grant_type": "authorization_code" });
        with_client_secret(&mut body, &spec());
        assert!(body.get("client_secret").is_none());

        // Blank → still no field (Atlassian would reject an empty secret anyway).
        let mut blank = spec();
        blank.client_secret = Some("   ".to_string());
        with_client_secret(&mut body, &blank);
        assert!(body.get("client_secret").is_none());
    }

    #[test]
    fn authorize_url_has_pkce_and_scope_params() {
        let s = spec();
        let pkce = pkce::generate();
        let url = build_authorize_url(
            "client123",
            s.authorize_url,
            s.scopes,
            &s.extra_authorize_params,
            "http://127.0.0.1:9123/callback",
            &pkce,
        );
        assert!(url.starts_with("https://auth.example.com/authorize?"));
        assert!(url.contains("response_type=code"));
        assert!(url.contains("client_id=client123"));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains(&format!("code_challenge={}", pkce.challenge)));
        assert!(url.contains("scope=read%3Ajira-work%20offline_access"));
        assert!(url.contains("redirect_uri=http%3A%2F%2F127.0.0.1%3A9123%2Fcallback"));
        assert!(url.contains("audience=api.atlassian.com"));
        assert!(url.contains(&format!("state={}", pkce.state)));
    }
}
