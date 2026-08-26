//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Jira-specific OAuth wiring on top of the generic PKCE `flow` engine:
//   * `login()`         — the interactive browser flow: browser consent → token
//                         exchange → cloud-id discovery → persist tokens. Runs
//                         in-process in the tray, and from `meridian oauth-login
//                         jira` (the daemon's debug CLI).
//   * `ensure_fresh()`  — daemon-side refresh-before-use (rotating tokens).
//   * `JiraReqCtx`      — resolved per-request auth context (Bearer vs basic).
//
// NOTE: `resolve()` — which picks OAuth-vs-API-token for a request — is the one
// piece that needs the daemon's `JiraConfig`, so it stays daemon-side
// (`src/intelligence/oauth/jira.rs`) and is NOT part of this config-free crate.
//
// OAuth-authenticated Jira calls go through the `api.atlassian.com/ex/jira/{cloudId}`
// gateway with a Bearer token — NOT `{site}.atlassian.net` with basic auth. The
// gateway base and the human `browse` base differ, so `JiraReqCtx` exposes both.

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::sync::OnceLock;
use tokio::sync::Mutex;

use crate::flow::{self, ProviderSpec};
use crate::store::{self, OAuthTokens};

// This mutex serialises refreshes WITHIN a process. It is NOT sufficient on its
// own: the daemon and the tray (and a stray second daemon) each compile
// meridian-oauth and hold independent mutex instances, so two processes could
// both read the same expired token, both POST to Atlassian, and the second POST
// 401 because the first already rotated and consumed the refresh token. The
// cross-process half of the fix lives in ensure_fresh(), which takes an advisory
// FILE lock (`store::lock_provider`) and re-checks expiry under it; this mutex
// stays as the cheap intra-process first gate so threads don't even reach the
// file lock concurrently.
fn refresh_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// Default fixed loopback port for the redirect. Atlassian requires an exact
/// redirect-URI match, so this port (and `http://127.0.0.1:<port>/callback`) must
/// be registered on the OAuth app. Override with `JIRA_OAUTH_REDIRECT_PORT`.
pub const DEFAULT_REDIRECT_PORT: u16 = 9123;

/// Meridian's Atlassian OAuth 2.0 (3LO) client id. Every install uses it, so
/// `meridian oauth-login jira` needs zero config. Override (e.g. for a different
/// app or Jira Data Center) with `JIRA_OAUTH_CLIENT_ID`.
///
/// Re-registering the app (developer.atlassian.com/console/myapps) — the console-only
/// facts that aren't recoverable from this code:
///   * Own it under a **Meridiona** Atlassian account, not a personal one.
///   * Scopes: the classic Jira scopes in `spec()` below (`offline_access` is
///     requested at runtime, not a console checkbox).
///   * Callback (exact match): `http://127.0.0.1:9123/callback` — use the **IP, not
///     `localhost`** (the console greys out Save for `localhost`).
///   * **Distribution → Enable sharing (Distributable) is REQUIRED** before any
///     non-Meridiona user can authorize — a private 3LO app only works for users in
///     the development org; external users hit a "site admin must authorize" block.
///   * Secret → the `JIRA_OAUTH_CLIENT_SECRET` Actions secret (see `client_secret`).
pub const DEFAULT_CLIENT_ID: &str = "sXRB5rwKFX53DUgb9u5LO7gr0pRMwNDS";

/// Meridian's Atlassian OAuth 2.0 (3LO) client secret, **baked in at build time**
/// — never stored in source. Atlassian Cloud's token endpoint ignores PKCE and
/// requires a `client_secret` even for desktop apps (a
/// [known limitation](https://jira.atlassian.com/browse/OAUTH20-2491)), so — unlike
/// a true public PKCE client — we must ship one. The official release build injects
/// it via the `MERIDIAN_JIRA_OAUTH_CLIENT_SECRET` compile-time env (a GitHub Actions
/// secret; see `.github/workflows/release.yml`); plain source builds compile in an
/// empty string, so a source-built binary must supply `JIRA_OAUTH_CLIENT_SECRET` at
/// runtime or use the API-token fallback.
///
/// Because this crate is compiled into BOTH the daemon and the tray, setting the
/// `MERIDIAN_JIRA_OAUTH_CLIENT_SECRET` env during either build bakes the secret
/// into that binary — so the tray's in-process login works in packaged builds.
///
/// It is extractable from the shipped binary by design, but the blast radius of a
/// leak is bounded: the registered redirect is loopback-only (`127.0.0.1:9123`,
/// exact-match enforced) and scopes are narrow, so it is revocable/rotatable in the
/// Atlassian console (rotate the secret and the Actions secret together).
pub const DEFAULT_CLIENT_SECRET: &str = match option_env!("MERIDIAN_JIRA_OAUTH_CLIENT_SECRET") {
    Some(s) => s,
    None => "",
};

/// Resolve the client id to use for `oauth-login`: `JIRA_OAUTH_CLIENT_ID` env
/// override if set and non-blank, else the baked-in default.
pub fn client_id() -> String {
    std::env::var("JIRA_OAUTH_CLIENT_ID")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_CLIENT_ID.to_string())
}

/// Resolve the client secret: `JIRA_OAUTH_CLIENT_SECRET` env override if set and
/// non-blank, else the baked-in default.
pub fn client_secret() -> String {
    std::env::var("JIRA_OAUTH_CLIENT_SECRET")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_CLIENT_SECRET.to_string())
}

const ACCESSIBLE_RESOURCES_URL: &str = "https://api.atlassian.com/oauth/token/accessible-resources";

/// Atlassian OAuth 2.0 (3LO) endpoints + scopes. `read:jira-work` powers the task
/// fetch, `write:jira-work` powers worklog/comment posting, `read:jira-user`
/// powers the `/myself` health probe (`meridian doctor`), and `offline_access` is
/// what yields a refresh token at all.
///
/// Used by `ensure_fresh()` which resolves the secret from env — fine for the
/// daemon. Interactive `login()` takes the secret explicitly to avoid `set_var`.
fn spec() -> ProviderSpec {
    spec_with_secret(client_secret())
}

fn spec_with_secret(secret: String) -> ProviderSpec {
    ProviderSpec {
        authorize_url: "https://auth.atlassian.com/authorize",
        token_url: "https://auth.atlassian.com/oauth/token",
        scopes: "read:jira-work write:jira-work read:jira-user offline_access",
        extra_authorize_params: vec![
            ("audience", "api.atlassian.com".to_string()),
            ("prompt", "consent".to_string()),
        ],
        client_secret: Some(secret),
    }
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Read the redirect port from `JIRA_OAUTH_REDIRECT_PORT`, falling back to the
/// registered default.
pub fn redirect_port() -> u16 {
    std::env::var("JIRA_OAUTH_REDIRECT_PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_REDIRECT_PORT)
}

#[derive(Debug, Deserialize)]
struct AccessibleResource {
    id: String,
    url: String,
    #[serde(default)]
    name: String,
}

/// Look up the Atlassian sites this token can reach. We need exactly one
/// cloud-id and site URL to address the REST gateway; if several are returned we
/// take the first and log the rest.
async fn discover_cloud(access_token: &str) -> Result<(String, String)> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(6))
        .build()?;
    let resp = client
        .get(ACCESSIBLE_RESOURCES_URL)
        .bearer_auth(access_token)
        .header("Accept", "application/json")
        .send()
        .await
        .context("GET accessible-resources")?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        bail!("accessible-resources → {status}: {text}");
    }
    let resources: Vec<AccessibleResource> = serde_json::from_str(&text)
        .with_context(|| format!("parsing accessible-resources: {text}"))?;
    let mut iter = resources.into_iter();
    let first = iter.next().context(
        "no accessible Jira sites for this authorization — is the app granted access to a site?",
    )?;
    let rest: Vec<String> = iter.map(|r| format!("{} ({})", r.name, r.url)).collect();
    if !rest.is_empty() {
        tracing::warn!(
            chosen = %first.url,
            others = ?rest,
            "multiple Atlassian sites authorized — using the first; set the one you want if this is wrong"
        );
    }
    Ok((first.id, first.url))
}

/// Run the interactive browser login and persist the resulting tokens. Returns
/// the chosen site URL for a friendly confirmation message.
///
/// `client_secret` is passed explicitly rather than read from `std::env` so
/// callers (the tray's in-process flow) can source it from `.env` without
/// calling `std::env::set_var` on a Tokio worker thread (POSIX setenv is not
/// thread-safe). Pass [`client_secret()`] when you want the env-var resolution.
pub async fn login(client_id: &str, client_secret: &str, port: u16) -> Result<String> {
    if client_secret.trim().is_empty() {
        bail!(
            "Jira OAuth requires a client secret that is baked in at build time via \
             MERIDIAN_JIRA_OAUTH_CLIENT_SECRET. This is a source build without that \
             secret — set JIRA_OAUTH_CLIENT_SECRET in your environment to supply one, \
             or use the API-token fallback (JIRA_BASE_URL / JIRA_EMAIL / JIRA_API_TOKEN)."
        );
    }
    let tokens = flow::run_authcode_flow(
        client_id,
        &spec_with_secret(client_secret.to_string()),
        port,
    )
    .await?;

    // No refresh token ⇒ `offline_access` wasn't granted (app misconfigured or the
    // scope wasn't consented). Fail NOW with a clear message rather than letting
    // the access token silently expire ~1 h later with no way to refresh.
    if tokens.refresh_token.trim().is_empty() {
        bail!(
            "authorization succeeded but no refresh token was returned — the `offline_access` \
             scope wasn't granted. Add `offline_access` to the OAuth app's permissions and retry."
        );
    }

    let (cloud_id, site_url) = discover_cloud(&tokens.access_token).await?;

    let stored = OAuthTokens {
        provider: "jira".to_string(),
        client_id: client_id.to_string(),
        access_token: tokens.access_token,
        refresh_token: tokens.refresh_token,
        expires_at: now_unix() + tokens.expires_in,
        scopes: tokens.scope,
        cloud_id,
        site_url: site_url.clone(),
    };
    store::save(&stored).context("persisting Jira OAuth tokens")?;
    Ok(site_url)
}

/// Load the stored tokens, refreshing the access token if it's within 120 s of
/// expiry. Persists the rotated refresh token. Returns ready-to-use tokens.
///
/// Refreshes are serialised on TWO levels so the rotating refresh token is never
/// double-spent: a static mutex within this process, and an advisory FILE lock
/// ([`store::lock_provider`]) across every Meridian process. After taking the file
/// lock it RE-LOADS and re-checks expiry — so a process that waited on the lock
/// adopts the peer's freshly-refreshed token instead of POSTing again with the
/// now-consumed one (the old single-process-mutex behaviour caused that 401 loop).
pub async fn ensure_fresh() -> Result<OAuthTokens> {
    let _guard = refresh_lock().lock().await; // intra-process serialisation

    // Fast path: a still-fresh token needs neither a refresh nor the file lock.
    let t = store::load("jira")?;
    if !t.is_expired(now_unix(), 120) {
        return Ok(t);
    }

    // Slow path: enter the cross-process critical section. A peer (a second daemon,
    // the tray's in-process refresh) may be rotating the SAME refresh token right
    // now. Best-effort — if the lock can't be taken we log and proceed rather than
    // turn the lock itself into a new way for Jira auth to fail.
    let _flock = match store::lock_provider("jira").await {
        Ok(g) => Some(g),
        Err(e) => {
            tracing::warn!(
                error = %e,
                "could not acquire OAuth refresh lock — proceeding without cross-process serialisation"
            );
            None
        }
    };

    // Re-load UNDER the lock and re-check: if a peer refreshed while we waited,
    // adopt their token instead of refreshing again with the dead one. This
    // double-check is what actually breaks the race.
    let mut t = store::load("jira")?;
    if !t.is_expired(now_unix(), 120) {
        tracing::debug!("jira token already refreshed by another process — adopting it");
        return Ok(t);
    }

    tracing::debug!("jira OAuth access token expired — refreshing");
    let resp = flow::refresh(&t.client_id, &spec(), &t.refresh_token)
        .await
        .context(
            "refreshing Jira OAuth token — re-run `meridian oauth-login jira` if this persists",
        )?;
    t.access_token = resp.access_token;
    if !resp.refresh_token.is_empty() {
        t.refresh_token = resp.refresh_token; // Atlassian rotates the refresh token
    }
    t.expires_at = now_unix() + resp.expires_in;
    if !resp.scope.is_empty() {
        t.scopes = resp.scope;
    }
    // Save the rotated tokens. If save fails (disk full, permissions), log a
    // critical error and return the in-memory tokens anyway — the access token
    // is valid for ~1h so the current request succeeds. On the next expiry the
    // stored (now-invalid) refresh token will cause a 401 and the user must
    // re-authenticate. A critical log surfaces this before it happens.
    match store::save(&t) {
        Ok(()) => {}
        Err(e) => {
            tracing::error!(
                error = %e,
                "CRITICAL: failed to persist rotated Jira refresh token — access token \
                 valid for ~1h but re-authentication required after expiry. Fix \
                 permissions at ~/.meridian/oauth/ then re-run `meridian oauth-login jira`."
            );
        }
    }
    Ok(t)
}

/// Resolved per-request auth context. OAuth and basic auth differ in BOTH the API
/// base (gateway vs site) and the auth header, so call sites go through this.
pub enum JiraReqCtx {
    OAuth {
        token: String,
        cloud_id: String,
        site_url: String,
    },
    Basic {
        base_url: String,
        email: String,
        api_token: String,
    },
}

impl JiraReqCtx {
    /// Build a REST API URL for `path` (which must start with `/`).
    pub fn api_url(&self, path: &str) -> String {
        match self {
            Self::OAuth { cloud_id, .. } => {
                format!("https://api.atlassian.com/ex/jira/{cloud_id}{path}")
            }
            Self::Basic { base_url, .. } => {
                format!("{}{}", base_url.trim_end_matches('/'), path)
            }
        }
    }

    /// Human-facing site root (e.g. `https://acme.atlassian.net`) — for building
    /// deep links like the create-issue dialog. Uses the site URL under OAuth.
    pub fn site_base(&self) -> String {
        let base = match self {
            Self::OAuth { site_url, .. } => site_url,
            Self::Basic { base_url, .. } => base_url,
        };
        base.trim_end_matches('/').to_string()
    }

    /// Human-facing `browse` URL for an issue key (uses the site URL under OAuth).
    pub fn browse_url(&self, issue_key: &str) -> String {
        let base = match self {
            Self::OAuth { site_url, .. } => site_url,
            Self::Basic { base_url, .. } => base_url,
        };
        format!("{}/browse/{}", base.trim_end_matches('/'), issue_key)
    }

    /// Apply the right auth to a request builder (Bearer vs basic).
    pub fn apply(&self, rb: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match self {
            Self::OAuth { token, .. } => rb.bearer_auth(token),
            Self::Basic {
                email, api_token, ..
            } => rb.basic_auth(email, Some(api_token)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn oauth_ctx() -> JiraReqCtx {
        JiraReqCtx::OAuth {
            token: "tok".into(),
            cloud_id: "cloud-xyz".into(),
            site_url: "https://acme.atlassian.net".into(),
        }
    }

    fn basic_ctx() -> JiraReqCtx {
        JiraReqCtx::Basic {
            base_url: "https://acme.atlassian.net/".into(),
            email: "a@b.com".into(),
            api_token: "tok".into(),
        }
    }

    #[test]
    fn oauth_api_url_uses_gateway() {
        assert_eq!(
            oauth_ctx().api_url("/rest/api/3/search/jql"),
            "https://api.atlassian.com/ex/jira/cloud-xyz/rest/api/3/search/jql"
        );
    }

    #[test]
    fn basic_api_url_uses_site_and_trims_slash() {
        assert_eq!(
            basic_ctx().api_url("/rest/api/3/search/jql"),
            "https://acme.atlassian.net/rest/api/3/search/jql"
        );
    }

    #[test]
    fn browse_url_uses_site_in_both_modes() {
        assert_eq!(
            oauth_ctx().browse_url("KAN-1"),
            "https://acme.atlassian.net/browse/KAN-1"
        );
        assert_eq!(
            basic_ctx().browse_url("KAN-1"),
            "https://acme.atlassian.net/browse/KAN-1"
        );
    }
}
