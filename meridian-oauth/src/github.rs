//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// GitHub OAuth **device flow** (RFC 8628). GitHub does not offer a loopback
// Authorization-Code + PKCE flow for the desktop use case the way Atlassian does;
// its recommended path for a headless/native app is the device flow: request a
// short user_code, show it to the user, have them enter it at
// https://github.com/login/device, and poll the token endpoint until they
// authorize. This is a PUBLIC client — the OAuth App has NO client secret for
// device flow, so nothing sensitive is baked into the binary.
//
// Unlike the previous approach (shelling out to the `gh` CLI, which ran the
// device flow in a headless subprocess whose one-time code the user never saw —
// so it always timed out), this runs in-process in the tray: the tray requests
// the code, surfaces user_code + verification_uri to the dashboard UI, opens the
// browser, and polls for the token. See `tray/src-tauri/src/commands/integrations.rs`.
//
// GitHub user-to-server tokens from an OAuth App do NOT expire by default (unless
// the app opts into expiring tokens), and the device flow issues no refresh
// token, so — like Trello — there is no refresh path here. The daemon consumes
// the token via `GITHUB_TOKEN` in `~/.meridian/.env` (see `src/config.rs`
// `parse_github`), so the tray writes it there rather than to the JSON store.
//
// Requires an OAuth App with **Device Flow enabled** (Settings → Developer
// settings → OAuth Apps → your app → "Enable Device Flow"). The public client_id
// is baked in at build time via `MERIDIAN_GITHUB_OAUTH_CLIENT_ID` (a GitHub
// Actions var; see `.github/workflows/release.yml`), overridable at runtime with
// `GITHUB_OAUTH_CLIENT_ID` for source builds / a custom app.

use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use serde::Deserialize;

/// GitHub's device-authorization endpoint — issues the `device_code`/`user_code` pair.
const DEVICE_CODE_URL: &str = "https://github.com/login/device/code";
/// GitHub's token endpoint — polled with the `device_code` until the user authorizes.
const TOKEN_URL: &str = "https://github.com/login/oauth/access_token";

/// Scopes needed for issue/PR reads and GitHub Projects v2 node-ID listing.
/// Space-separated per the OAuth `scope` param (the `gh` CLI used commas — that
/// was a `gh`-specific arg format, not the OAuth wire format).
///
/// `project` (read-WRITE), not `read:project`: creating a task from the day plan
/// calls `addProjectV2ItemById` to put the new issue on the user's board
/// (`crate`-side, `src/pm_worklog/create_github.rs`), and that mutation is
/// refused under `read:project` with an HTTP 200 + a `errors[].message` naming
/// the missing scope. `project` subsumes `read:project`, so nothing that worked
/// before needs the narrower one. A token minted before this change keeps only
/// `read:project` — reads carry on working and the board add degrades to a
/// warning, so the user is nudged to reconnect rather than broken by it.
pub const REQUIRED_SCOPES: &str = "repo read:org project";

/// Meridian's GitHub OAuth App client id — a PUBLIC identifier (device flow has
/// no client secret), baked in at build time so the in-app browser connect needs
/// zero config. Empty in a plain source build; override with `GITHUB_OAUTH_CLIENT_ID`.
///
/// Re-registering the app (github.com → Settings → Developer settings → OAuth
/// Apps) — the console-only facts not recoverable from this code:
///   * Own it under the **Meridiona** org, not a personal account.
///   * **Enable Device Flow** must be checked, or `/login/device/code` 404s /
///     returns `device_flow_disabled`.
///   * No callback URL is used by the device flow (it's still required to save
///     the app — any placeholder is fine).
///   * There is NO client secret to ship — do not add one here.
pub const DEFAULT_CLIENT_ID: &str = match option_env!("MERIDIAN_GITHUB_OAUTH_CLIENT_ID") {
    Some(s) => s,
    None => "",
};

/// Resolve the client id: `GITHUB_OAUTH_CLIENT_ID` env override if set and
/// non-blank, else the baked-in default.
pub fn client_id() -> String {
    std::env::var("GITHUB_OAUTH_CLIENT_ID")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_CLIENT_ID.to_string())
}

/// The device-authorization grant the UI needs to show the user. `verification_uri`
/// is where they enter `user_code` (`https://github.com/login/device`);
/// `device_code` is the opaque handle the token poll uses; `interval`/`expires_in`
/// pace and bound the poll.
#[derive(Debug, Clone)]
pub struct DeviceCode {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub interval: u64,
    pub expires_in: u64,
}

/// Raw device-code endpoint response — `error` present when the request itself
/// failed (e.g. bad client_id, device flow disabled).
#[derive(Debug, Deserialize)]
struct DeviceCodeRaw {
    device_code: Option<String>,
    user_code: Option<String>,
    verification_uri: Option<String>,
    interval: Option<u64>,
    expires_in: Option<u64>,
    error: Option<String>,
    error_description: Option<String>,
}

/// Raw token-poll response — GitHub returns HTTP 200 with either an
/// `access_token` (success) or an `error` (`authorization_pending`, `slow_down`,
/// `expired_token`, `access_denied`, …).
#[derive(Debug, Deserialize)]
struct TokenPollRaw {
    access_token: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
    /// On `slow_down`, GitHub returns the new (larger) minimum interval.
    interval: Option<u64>,
}

fn http_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .context("building HTTP client for GitHub device flow")
}

/// Request a device/user code pair from GitHub for `client_id` and `scopes`.
/// This is the first, synchronous half of the flow — the caller shows the
/// returned `user_code`/`verification_uri` to the user, then drives
/// [`poll_for_token`] in the background.
pub async fn request_device_code(client_id: &str, scopes: &str) -> Result<DeviceCode> {
    if client_id.trim().is_empty() {
        bail!(
            "GitHub OAuth is not configured in this build (no client id). A packaged \
             release bakes in MERIDIAN_GITHUB_OAUTH_CLIENT_ID; for a source build, set \
             GITHUB_OAUTH_CLIENT_ID in your .env, or use the Personal Access Token path instead."
        );
    }
    let raw: DeviceCodeRaw = http_client()?
        .post(DEVICE_CODE_URL)
        .header("Accept", "application/json")
        .form(&[("client_id", client_id), ("scope", scopes)])
        .send()
        .await
        .with_context(|| format!("POST {DEVICE_CODE_URL}"))?
        .error_for_status()
        .context("GitHub device-code request failed")?
        .json()
        .await
        .context("parsing GitHub device-code response")?;

    if let Some(err) = raw.error {
        let desc = raw
            .error_description
            .map(|d| format!(" — {d}"))
            .unwrap_or_default();
        // A misconfigured app most commonly surfaces here.
        bail!("GitHub device-code request rejected: {err}{desc}");
    }

    Ok(DeviceCode {
        device_code: raw
            .device_code
            .context("GitHub device-code response missing device_code")?,
        user_code: raw
            .user_code
            .context("GitHub device-code response missing user_code")?,
        verification_uri: raw
            .verification_uri
            .unwrap_or_else(|| "https://github.com/login/device".to_string()),
        interval: raw.interval.unwrap_or(5).max(1),
        expires_in: raw.expires_in.unwrap_or(900),
    })
}

/// Poll GitHub's token endpoint until the user authorizes (returns the
/// `access_token`), denies, or the code expires. Sleeps `interval` seconds
/// between polls, honouring `slow_down` back-off, and gives up after
/// `expires_in` seconds. Blocking-free (async sleeps).
pub async fn poll_for_token(
    client_id: &str,
    device_code: &str,
    interval: u64,
    expires_in: u64,
) -> Result<String> {
    let client = http_client()?;
    let mut interval = interval.max(1);
    let deadline = Instant::now() + Duration::from_secs(expires_in.max(1));

    loop {
        if Instant::now() >= deadline {
            bail!(
                "GitHub authorization timed out — the one-time code expired before you approved it"
            );
        }
        tokio::time::sleep(Duration::from_secs(interval)).await;

        // Transient failures (wifi blip, laptop sleep/wake, a GitHub 5xx that
        // doesn't parse as TokenPollRaw) are retried silently — the `expires_in`
        // deadline is the authoritative backstop for giving up, not a single I/O
        // error during the polling window.
        let resp = match client
            .post(TOKEN_URL)
            .header("Accept", "application/json")
            .form(&[
                ("client_id", client_id),
                ("device_code", device_code),
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
            ])
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                tracing::debug!(error = %e, "GitHub token-poll request failed (transient — retrying)");
                continue;
            }
        };
        let raw: TokenPollRaw = match resp.json().await {
            Ok(r) => r,
            Err(e) => {
                tracing::debug!(error = %e, "GitHub token-poll response parse error (transient — retrying)");
                continue;
            }
        };

        if let Some(token) = raw.access_token.filter(|t| !t.is_empty()) {
            return Ok(token);
        }
        match raw.error.as_deref() {
            // Still waiting on the user — keep polling.
            Some("authorization_pending") => continue,
            // Polling too fast — GitHub tells us the new floor (or +5s).
            Some("slow_down") => {
                interval = raw.interval.unwrap_or(interval + 5).max(interval + 1);
                continue;
            }
            Some("expired_token") => {
                bail!("GitHub authorization timed out — the one-time code expired. Try connecting again.")
            }
            Some("access_denied") => bail!("GitHub authorization was denied in the browser"),
            Some(other) => {
                let desc = raw
                    .error_description
                    .map(|d| format!(" — {d}"))
                    .unwrap_or_default();
                bail!("GitHub device flow error: {other}{desc}");
            }
            None => bail!("unexpected empty response from GitHub token endpoint"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Both assertions live in ONE test on purpose: they mutate the same
    // process-global `GITHUB_OAUTH_CLIENT_ID`, and Rust runs tests in parallel,
    // so splitting them lets one clobber the other's env var mid-run (a flaky
    // race that only surfaced under full-workspace parallelism). Kept serial here.
    #[test]
    fn client_id_env_override_rules() {
        let _env = crate::env_test_guard();
        // A real override wins.
        std::env::set_var("GITHUB_OAUTH_CLIENT_ID", "Ov23liEXAMPLE");
        assert_eq!(client_id(), "Ov23liEXAMPLE");

        // A blank override falls back to the baked-in default (empty in source builds).
        std::env::set_var("GITHUB_OAUTH_CLIENT_ID", "   ");
        assert_eq!(client_id(), DEFAULT_CLIENT_ID.to_string());

        std::env::remove_var("GITHUB_OAUTH_CLIENT_ID");
    }

    #[test]
    fn device_code_raw_parses_success_shape() {
        let raw: DeviceCodeRaw = serde_json::from_str(
            r#"{"device_code":"dc","user_code":"WXYZ-1234","verification_uri":"https://github.com/login/device","expires_in":900,"interval":5}"#,
        )
        .unwrap();
        assert_eq!(raw.device_code.as_deref(), Some("dc"));
        assert_eq!(raw.user_code.as_deref(), Some("WXYZ-1234"));
        assert!(raw.error.is_none());
    }

    #[test]
    fn token_poll_raw_parses_pending_and_success() {
        let pending: TokenPollRaw =
            serde_json::from_str(r#"{"error":"authorization_pending"}"#).unwrap();
        assert!(pending.access_token.is_none());
        assert_eq!(pending.error.as_deref(), Some("authorization_pending"));

        let ok: TokenPollRaw = serde_json::from_str(
            r#"{"access_token":"gho_abc","token_type":"bearer","scope":"repo"}"#,
        )
        .unwrap();
        assert_eq!(ok.access_token.as_deref(), Some("gho_abc"));
        assert!(ok.error.is_none());
    }
}
