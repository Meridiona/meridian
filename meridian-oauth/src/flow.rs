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
    let authorize = build_authorize_url(client_id, spec, &redirect_uri, &pkce);

    eprintln!("\nOpening your browser to authorize…");
    eprintln!("If it doesn't open, paste this URL:\n\n{authorize}\n");
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
    post_token(spec.token_url, &body)
        .await
        .context("refreshing OAuth access token")
}

// ---------------------------------------------------------------------------
// Internals
// ---------------------------------------------------------------------------

fn build_authorize_url(
    client_id: &str,
    spec: &ProviderSpec,
    redirect_uri: &str,
    pkce: &pkce::Pkce,
) -> String {
    let mut params: Vec<(&str, String)> = vec![
        ("response_type", "code".to_string()),
        ("client_id", client_id.to_string()),
        ("redirect_uri", redirect_uri.to_string()),
        ("scope", spec.scopes.to_string()),
        ("state", pkce.state.clone()),
        ("code_challenge", pkce.challenge.clone()),
        ("code_challenge_method", "S256".to_string()),
    ];
    for (k, v) in &spec.extra_authorize_params {
        params.push((k, v.clone()));
    }
    let query = params
        .iter()
        .map(|(k, v)| format!("{}={}", encode(k), encode(v)))
        .collect::<Vec<_>>()
        .join("&");
    format!("{}?{}", spec.authorize_url, query)
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
    post_token(spec.token_url, &body)
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

async fn post_token(token_url: &str, body: &serde_json::Value) -> Result<TokenResponse> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(6))
        .build()?;
    let resp = client
        .post(token_url)
        .header("Accept", "application/json")
        .json(body)
        .send()
        .await
        .with_context(|| format!("POST {token_url}"))?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        bail!("token endpoint {token_url} → {status}: {text}");
    }
    serde_json::from_str(&text).with_context(|| format!("parsing token response: {text}"))
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
    eprintln!("\nOpening your browser to authorize…");
    eprintln!("If it doesn't open, paste this URL:\n\n{authorize_url}\n");
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

/// Open `url` in the system browser. macOS-only (`open`); non-fatal if it fails —
/// the URL is also printed for manual paste.
fn open_browser(url: &str) {
    let _ = std::process::Command::new("open").arg(url).spawn();
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
        let pkce = pkce::generate();
        let url = build_authorize_url(
            "client123",
            &spec(),
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
