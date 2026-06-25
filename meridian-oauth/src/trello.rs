//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Trello token flow. Trello uses an older "token" grant (not Authorization Code
// + PKCE) that delivers the token in the URL fragment — `#token=...` — which the
// HTTP server cannot read directly. A JS relay page captures it.
//
// The `key` param here is Meridian's Trello Power-Up app key — baked in just
// like Jira's client_id. It is NOT per-user; users never see or manage it.
// Override with `TRELLO_APP_KEY` for a custom Power-Up.
//
// Trello tokens created with `expiration=never` do not expire, so `expires_at`
// is set to a far-future constant (year 2100) and no refresh path is needed.

use anyhow::{Context, Result};

use crate::{flow, store};

/// Empty sentinel — the real key is never stored in source. Set TRELLO_APP_KEY
/// in the bundle .env at package time (scripts/package-release.sh injects it).
pub const DEFAULT_APP_KEY: &str = "";

/// Fixed loopback port for the Trello token relay. Must be registered as an
/// allowed origin on the Power-Up admin page.
pub const DEFAULT_REDIRECT_PORT: u16 = 9123;

/// Year 2100 Unix timestamp — used as a practical "never expires" sentinel for
/// Trello tokens (avoids i64::MAX overflow in the is_expired skew arithmetic).
const TRELLO_EXPIRES_NEVER: i64 = 4_102_444_800;

/// Resolve the app key: TRELLO_APP_KEY env override if set, else baked-in default.
pub fn app_key() -> String {
    std::env::var("TRELLO_APP_KEY")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_APP_KEY.to_string())
}

/// Resolve the redirect port: TRELLO_OAUTH_REDIRECT_PORT env override if valid, else default.
pub fn redirect_port() -> u16 {
    std::env::var("TRELLO_OAUTH_REDIRECT_PORT")
        .ok()
        .and_then(|v| v.parse::<u16>().ok())
        .unwrap_or(DEFAULT_REDIRECT_PORT)
}

/// Interactive browser token flow. Opens the Trello authorization page, waits
/// for the user to grant access, and persists the token to
/// `~/.meridian/oauth/trello.json`. Returns `Ok(())` on success.
pub async fn login(app_key: &str, port: u16) -> Result<()> {
    if app_key.is_empty() {
        anyhow::bail!(
            "No Trello app key configured. Register a Power-Up at \
             https://trello.com/power-ups/admin, add http://127.0.0.1:{port}/ as an \
             allowed origin, then set TRELLO_APP_KEY=<your-key> in your .env."
        );
    }
    let return_url = format!("http://127.0.0.1:{port}/callback");
    let authorize_url = format!(
        "https://trello.com/1/authorize\
         ?expiration=never\
         &name=Meridian\
         &scope=read%2Cwrite\
         &response_type=token\
         &key={}\
         &callback_method=fragment\
         &return_url={}",
        url_encode(app_key),
        url_encode(&return_url),
    );

    let token = flow::run_fragment_relay_flow(&authorize_url, port)
        .await
        .context("Trello token relay")?;

    let tokens = store::OAuthTokens {
        provider: "trello".to_string(),
        client_id: app_key.to_string(),
        access_token: token,
        refresh_token: String::new(),
        expires_at: TRELLO_EXPIRES_NEVER,
        scopes: "read,write".to_string(),
        cloud_id: String::new(),
        site_url: String::new(),
    };
    store::save(&tokens).context("saving Trello token")?;
    Ok(())
}

/// Load the stored Trello user token. Errors if `meridian oauth-login trello`
/// has not been run.
pub fn load_token() -> Result<String> {
    let tokens = store::load("trello").context("loading Trello token")?;
    Ok(tokens.access_token)
}

/// RFC 3986 percent-encoding for a query-string value.
fn url_encode(s: &str) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_encode_handles_special_chars() {
        assert_eq!(url_encode("read,write"), "read%2Cwrite");
        assert_eq!(
            url_encode("http://127.0.0.1:9123/callback"),
            "http%3A%2F%2F127.0.0.1%3A9123%2Fcallback"
        );
        assert_eq!(url_encode("AZaz09-_.~"), "AZaz09-_.~");
    }
}
