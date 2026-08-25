//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Generic OAuth 2.0 Authorization Code + PKCE engine, shared by the PM providers
// (Jira now, Linear next). It opens the system browser to the provider's consent
// screen, captures the redirect on a fixed loopback port (Atlassian requires an
// exact-match redirect URI, so the port is fixed, not ephemeral), exchanges the
// code for tokens, and refreshes rotating tokens. Provider-specific URLs/scopes
// are supplied via `ProviderSpec`; everything else here is provider-blind.
//
// THE RULE THAT MATTERS HERE: a `refresh_token` grant is NOT idempotent. Atlassian
// rotates the refresh token on every use and applies reuse detection - presenting a
// refresh token it has already consumed revokes the whole token family, permanently.
// So a refresh POST may only be re-sent when we can PROVE the first one never
// reached the server. `TokenFailure::Ambiguous` and `GrantKind` below are that
// proof obligation made explicit; `should_retry` is where it is enforced.

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

/// What a token-endpoint failure says about (a) whether the user must
/// re-authenticate and (b) whether the SAME grant may be presented again.
///
/// Those are two different questions and conflating them is what broke real
/// installs: the original two-variant version treated every non-4xx as
/// "transient, retry immediately", including failures where the request had
/// already been delivered and acted on. See [`Ambiguous`](Self::Ambiguous).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenFailure {
    /// The grant itself was rejected and won't recover on its own — an OAuth 4xx
    /// (`invalid_grant`, a revoked/expired refresh token, a bad client). The
    /// stored refresh token is dead; the user must re-authenticate.
    Terminal,
    /// A passing condition where the endpoint provably did NOT act on the grant:
    /// the connection was never established (`reqwest::Error::is_connect()`, which
    /// covers a connect timeout), or the server refused the request outright with
    /// a 429 before processing it. The stored refresh token is untouched, so
    /// re-sending it is safe.
    Transient,
    /// The connection was made but no usable answer came back — a response
    /// timeout, a mid-response reset, a 5xx from an edge, or a 2xx whose body
    /// didn't parse. **We cannot know whether the grant was consumed.**
    ///
    /// For a rotating refresh token that is the dangerous case, and it is not
    /// hypothetical: on 2026-08-20 a laptop woke from sleep, the first refresh
    /// POST failed with `error sending request`, the automatic retry 400 ms later
    /// presented the same refresh token, and Atlassian answered
    /// `403 unauthorized_client: refresh_token is invalid` — the first attempt HAD
    /// reached Atlassian and rotated the token; only the response was lost. The
    /// retry turned a recoverable blip into a permanently revoked grant, and the
    /// user's Jira stayed disconnected until they re-authorised by hand.
    ///
    /// Treated as non-terminal for user-facing purposes (the token may well still
    /// be fine, so no "Reconnect" banner), but never retried for a rotating grant
    /// — see [`should_retry`].
    Ambiguous,
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
    /// A failure that happened BEFORE any grant left this machine — today, only
    /// "a peer process is holding the refresh lock". Exposed (unlike the other
    /// constructors) so `jira::ensure_fresh` can decline to refresh and still
    /// have [`is_transient`] answer `true`, keeping the "Reconnect Jira" remedy
    /// reserved for a grant the provider actually rejected.
    pub fn unavailable(detail: impl Into<String>) -> Self {
        Self::transient(detail)
    }

    fn ambiguous(detail: impl Into<String>) -> Self {
        Self {
            failure: TokenFailure::Ambiguous,
            detail: detail.into(),
        }
    }
    fn terminal(detail: impl Into<String>) -> Self {
        Self {
            failure: TokenFailure::Terminal,
            detail: detail.into(),
        }
    }
    /// "Not a dead grant" — true for both [`TokenFailure::Transient`] and
    /// [`TokenFailure::Ambiguous`]. This drives the *user-facing* remedy only:
    /// an ambiguous failure must not tell the user to reconnect, because the
    /// stored token may still be perfectly valid. Whether the grant may be
    /// re-sent is a separate question, answered by [`should_retry`].
    fn is_transient(&self) -> bool {
        !matches!(self.failure, TokenFailure::Terminal)
    }
}

impl std::fmt::Display for TokenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.detail)
    }
}

impl std::error::Error for TokenError {}

/// Whether an `anyhow` error from the OAuth flow was a token-endpoint failure the
/// user cannot act on (network/timeout/429/5xx — [`TokenFailure::Transient`] or
/// [`TokenFailure::Ambiguous`]) rather than a dead grant. Note this is NOT the
/// same as "safe to retry": an ambiguous failure answers `true` here and is still
/// never re-sent for a rotating grant (see [`should_retry`]).
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
    post_token_retrying(spec.token_url, &body, GrantKind::Rotating)
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
    post_token_retrying(spec.token_url, &body, GrantKind::Interactive)
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

/// Which kind of grant a token POST is carrying — the input to [`should_retry`].
///
/// The distinction is about what a *re-sent* request costs, not about the
/// endpoint or the wire format (both are identical).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GrantKind {
    /// `grant_type=refresh_token` against a provider that ROTATES the token.
    /// Re-sending a grant the server already consumed is not merely useless: with
    /// reuse detection (Atlassian has it) it revokes the entire token family, so
    /// a blip the daemon would have shrugged off next tick becomes a permanent
    /// disconnection the user must fix by hand. Only ever retried when the
    /// request provably never reached the server.
    Rotating,
    /// `grant_type=authorization_code`. Also single-use, but nothing durable is
    /// destroyed by re-sending it: the user is standing at the browser, and the
    /// worst case is one more click. Keeps the original retry breadth so a blip
    /// mid-consent doesn't fail a login that would have worked.
    Interactive,
}

/// Whether a failed attempt may be re-sent with the SAME grant.
///
/// The whole point of this function is that it takes [`GrantKind`]: "is this
/// error passing?" and "may I send this grant again?" are different questions,
/// and answering the second with the first is what revoked real users' Jira
/// grants. Pure, so the policy is unit-testable without a network.
fn should_retry(kind: GrantKind, failure: TokenFailure) -> bool {
    match (kind, failure) {
        // A dead grant fails identically every time; retries would only delay
        // the user's remedy behind seconds of backoff.
        (_, TokenFailure::Terminal) => false,
        // Provably not delivered (or explicitly refused unprocessed) — the grant
        // is untouched, so re-sending it is exactly as safe as sending it once.
        (_, TokenFailure::Transient) => true,
        // May already have been consumed and rotated server-side. Re-sending is
        // the difference between "probably fine next tick" and "revoked".
        (GrantKind::Rotating, TokenFailure::Ambiguous) => false,
        (GrantKind::Interactive, TokenFailure::Ambiguous) => true,
    }
}

/// Total tries [`post_token_retrying`] gives the token endpoint before it gives
/// up on a retry-safe failure. Three attempts across ~1.6 s of backoff ride out
/// the momentary network blips and provider 5xx/429s that otherwise surfaced a
/// spurious "re-authenticate" sync error on the very next 30-min poll. A dead
/// grant (4xx) still fails on the first try, and so does an ambiguous one on a
/// rotating grant — retries are spent only where [`should_retry`] allows.
pub(crate) const TOKEN_POST_ATTEMPTS: usize = 3;

/// Per-attempt cap on the entire token request (connect + TLS + request +
/// response). Deliberately generous. The 8 s this replaced was itself a way to
/// LOSE a rotated refresh token: Atlassian would process the grant, the response
/// would miss the cut-off, and the reply carrying the new token was discarded —
/// indistinguishable, from here, from a request that never landed.
pub(crate) const TOKEN_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Cap on connection establishment alone. Splitting this out of
/// [`TOKEN_REQUEST_TIMEOUT`] is what makes "never delivered" distinguishable from
/// "no usable answer": a connect timeout surfaces as
/// `reqwest::Error::is_connect()` and is therefore retry-safe, while a response
/// timeout does not and must not be. Without the split, the single most common
/// real-world failure — the first request after a laptop wakes, before the
/// network is up — was classified as retryable when it was not.
pub(crate) const TOKEN_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

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

/// [`post_token`] with bounded retry-with-backoff, gated by [`should_retry`].
/// A terminal rejection returns immediately so a genuinely dead token isn't
/// hidden behind seconds of backoff — and so does an ambiguous failure on a
/// rotating grant, because re-sending it is what makes the grant dead.
#[tracing::instrument(skip(body), fields(token_url = %token_url))]
async fn post_token_retrying(
    token_url: &str,
    body: &serde_json::Value,
    kind: GrantKind,
) -> Result<TokenResponse, TokenError> {
    // One client for the whole retry loop: rebuilding it per attempt would throw
    // away connection pooling/keep-alive, which is exactly what a retry wants.
    // Both timeouts are per request (each `send`), not a total budget.
    let client = reqwest::Client::builder()
        .connect_timeout(TOKEN_CONNECT_TIMEOUT)
        .timeout(TOKEN_REQUEST_TIMEOUT)
        .build()
        .map_err(|e| TokenError::transient(format!("building HTTP client: {e}")))?;
    let mut backoff = Duration::from_millis(400);
    for attempt in 1..=TOKEN_POST_ATTEMPTS {
        match post_token(&client, token_url, body).await {
            Ok(resp) => return Ok(resp),
            Err(e) if should_retry(kind, e.failure) && attempt < TOKEN_POST_ATTEMPTS => {
                tracing::warn!(
                    attempt,
                    max = TOKEN_POST_ATTEMPTS,
                    error = %e,
                    "OAuth token endpoint transient failure - retrying after backoff"
                );
                tokio::time::sleep(backoff).await;
                backoff *= 3; // 400ms → 1200ms
            }
            Err(e) => {
                // The case worth being able to find in the fleet: the grant may
                // have been consumed server-side and the rotated token lost with
                // the response. We deliberately do NOT retry (that is what
                // revokes the family), so the next scheduled attempt either
                // succeeds with the stored token or returns a clean 4xx.
                if kind == GrantKind::Rotating && e.failure == TokenFailure::Ambiguous {
                    tracing::warn!(
                        attempt,
                        error = %e,
                        "OAuth refresh got no usable response - the provider may have already \
                         rotated the refresh token; NOT retrying (a retry would revoke the grant)"
                    );
                }
                return Err(e);
            }
        }
    }
    // The `attempt < TOKEN_POST_ATTEMPTS` guard means the final attempt always
    // takes a return arm above.
    unreachable!("post_token_retrying loop returns on the last attempt")
}

/// Classify a `reqwest` send failure by whether the request can have been
/// delivered. `is_connect()` is the only signal that answers "no" — it covers a
/// refused connection, a DNS failure, a TLS handshake failure and a connect
/// timeout, all of which happen strictly before any bytes of the grant are sent.
/// Everything else (response timeout, connection reset mid-flight, body error)
/// leaves the question open, so it is [`TokenFailure::Ambiguous`].
fn classify_send_error(e: &reqwest::Error) -> TokenFailure {
    if e.is_connect() {
        TokenFailure::Transient
    } else {
        TokenFailure::Ambiguous
    }
}

/// Classify a non-2xx token-endpoint status.
///
/// 429 is the one non-2xx we know was refused WITHOUT the grant being processed,
/// so it stays retry-safe. A 5xx is not that: an edge or gateway can answer 502
/// after the upstream already consumed and rotated the token, so it downgrades to
/// [`TokenFailure::Ambiguous`] rather than the blanket "transient" it used to be.
/// Every other non-2xx is a real rejection: a 4xx `invalid_grant` / bad client /
/// revoked-or-rotated refresh token the user must act on.
fn classify_status(status: reqwest::StatusCode) -> TokenFailure {
    if status.as_u16() == 429 {
        TokenFailure::Transient
    } else if status.is_server_error() {
        TokenFailure::Ambiguous
    } else {
        TokenFailure::Terminal
    }
}

/// POST a grant to the token endpoint once and classify any failure (see
/// [`TokenFailure`]). Takes a shared `client` (built once by
/// [`post_token_retrying`]) whose timeouts apply per attempt.
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
        // Says nothing about the token's VALIDITY either way — but it does say
        // something about whether the grant may have been spent, which is what
        // `classify_send_error` reads off the error.
        Err(e) => {
            let detail = format!("POST {token_url}: {e}");
            return Err(match classify_send_error(&e) {
                TokenFailure::Transient => TokenError::transient(detail),
                _ => TokenError::ambiguous(detail),
            });
        }
    };
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        let detail = format!(
            "token endpoint {token_url} → {status}: {}",
            truncate_body(&text)
        );
        return Err(match classify_status(status) {
            TokenFailure::Transient => TokenError::transient(detail),
            TokenFailure::Ambiguous => TokenError::ambiguous(detail),
            TokenFailure::Terminal => TokenError::terminal(detail),
        });
    }
    // A 2xx whose body isn't valid JSON is almost always a proxy or captive-portal
    // interstitial standing in for the real payload, not a malformed token grant.
    // AMBIGUOUS, not transient: we cannot tell an interstitial (grant untouched)
    // from a real 2xx we failed to read (grant spent, rotated token lost), and
    // for a rotating grant the second reading is the one that must win.
    serde_json::from_str(&text).map_err(|e| {
        TokenError::ambiguous(format!(
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

    /// An ambiguous failure must NOT tell the user to reconnect. The stored
    /// refresh token may well still be valid (a lost response says nothing about
    /// it), so the remedy is "wait for the next tick", exactly as for a plain
    /// transient. This is the half of the split that faces the user.
    #[test]
    fn ambiguous_failures_are_not_surfaced_as_dead_grants() {
        let ambiguous = anyhow::Error::new(TokenError::ambiguous("response timed out"))
            .context("refreshing OAuth access token");
        assert!(is_transient(&ambiguous));
    }

    /// The half of the split that faces the PROVIDER, and the one that regressed
    /// in production: a rotating refresh grant may only be re-sent when the
    /// request provably never landed. Re-sending after a lost response is what
    /// trips Atlassian's refresh-token reuse detection and revokes the family.
    #[test]
    fn a_rotating_grant_is_never_retried_after_an_ambiguous_failure() {
        assert!(!should_retry(GrantKind::Rotating, TokenFailure::Ambiguous));
        // Provably-undelivered and explicitly-refused-unprocessed stay retryable,
        // so a genuine network blip still doesn't cost a sync cycle.
        assert!(should_retry(GrantKind::Rotating, TokenFailure::Transient));
        // A dead grant fails identically every time — don't bury the remedy.
        assert!(!should_retry(GrantKind::Rotating, TokenFailure::Terminal));
    }

    /// An interactive code exchange keeps the wider retry breadth: nothing
    /// durable is destroyed by re-sending it, and the user is at the browser.
    #[test]
    fn an_interactive_grant_still_retries_ambiguous_failures() {
        assert!(should_retry(
            GrantKind::Interactive,
            TokenFailure::Ambiguous
        ));
        assert!(should_retry(
            GrantKind::Interactive,
            TokenFailure::Transient
        ));
        assert!(!should_retry(
            GrantKind::Interactive,
            TokenFailure::Terminal
        ));
    }

    /// 429 means the request was refused BEFORE the grant was processed, so it
    /// stays retry-safe. A 5xx does not carry that guarantee — an edge can answer
    /// 502 after the upstream already rotated the token — so it must not be
    /// retried for a rotating grant. 4xx stays terminal.
    #[test]
    fn status_classification_separates_429_from_5xx() {
        use reqwest::StatusCode;
        assert_eq!(
            classify_status(StatusCode::TOO_MANY_REQUESTS),
            TokenFailure::Transient
        );
        assert_eq!(
            classify_status(StatusCode::BAD_GATEWAY),
            TokenFailure::Ambiguous
        );
        assert_eq!(
            classify_status(StatusCode::INTERNAL_SERVER_ERROR),
            TokenFailure::Ambiguous
        );
        // The exact status the revoked-grant incident returned.
        assert_eq!(
            classify_status(StatusCode::FORBIDDEN),
            TokenFailure::Terminal
        );
        assert_eq!(
            classify_status(StatusCode::UNAUTHORIZED),
            TokenFailure::Terminal
        );
        assert_eq!(
            classify_status(StatusCode::BAD_REQUEST),
            TokenFailure::Terminal
        );
        // ...and 5xx must not be retried for a rotating grant, which is the
        // behaviour change this classification exists to produce.
        assert!(!should_retry(
            GrantKind::Rotating,
            classify_status(StatusCode::BAD_GATEWAY)
        ));
    }

    /// A connection that was never established cannot have delivered the grant,
    /// so it is retry-safe; anything else leaves the question open. Built from a
    /// real `reqwest` error (a connect failure to a closed port) rather than a
    /// hand-made one, so this pins the actual `is_connect()` behaviour of the
    /// version we ship, not our belief about it.
    #[tokio::test]
    async fn a_refused_connection_is_retry_safe() {
        // Port 1 on loopback: nothing listens, and the connection is refused
        // during connect — no request bytes are ever written.
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(2))
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap();
        let err = client
            .post("http://127.0.0.1:1/oauth/token")
            .json(&serde_json::json!({"grant_type": "refresh_token"}))
            .send()
            .await
            .expect_err("nothing listens on 127.0.0.1:1");
        assert!(err.is_connect(), "expected a connect error, got: {err}");
        assert_eq!(classify_send_error(&err), TokenFailure::Transient);
        assert!(should_retry(GrantKind::Rotating, classify_send_error(&err)));
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
