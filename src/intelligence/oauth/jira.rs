//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Jira-specific OAuth wiring on top of the generic PKCE `flow` engine:
//   * `login()`         — the interactive `meridian oauth-login jira` flow:
//                         browser consent → token exchange → cloud-id discovery →
//                         persist tokens.
//   * `ensure_fresh()`  — daemon-side refresh-before-use (rotating tokens).
//   * `resolve()`       — pick the auth context for a request: OAuth if a token
//                         store exists, else fall back to a static API token.
//
// OAuth-authenticated Jira calls go through the `api.atlassian.com/ex/jira/{cloudId}`
// gateway with a Bearer token — NOT `{site}.atlassian.net` with basic auth. The
// gateway base and the human `browse` base differ, so `JiraReqCtx` exposes both.

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::sync::OnceLock;
use tokio::sync::Mutex;

use super::flow::{self, ProviderSpec};
use super::store::{self, OAuthTokens};
use crate::config::JiraConfig;

// Static mutex serializes concurrent token refreshes to avoid race conditions.
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
/// empty string, so a source-built daemon must supply `JIRA_OAUTH_CLIENT_SECRET` at
/// runtime or use the API-token fallback.
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
fn spec() -> ProviderSpec {
    ProviderSpec {
        authorize_url: "https://auth.atlassian.com/authorize",
        token_url: "https://auth.atlassian.com/oauth/token",
        scopes: "read:jira-work write:jira-work read:jira-user offline_access",
        extra_authorize_params: vec![
            ("audience", "api.atlassian.com".to_string()),
            ("prompt", "consent".to_string()),
        ],
        client_secret: Some(client_secret()),
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
pub async fn login(client_id: &str, port: u16) -> Result<String> {
    let tokens = flow::run_authcode_flow(client_id, &spec(), port).await?;

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
        client_secret: client_secret(),
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
/// Serializes concurrent refresh requests via a static mutex to avoid
/// double-refreshing and losing the rotated refresh token.
pub async fn ensure_fresh() -> Result<OAuthTokens> {
    let _guard = refresh_lock().lock().await;
    let mut t = store::load("jira")?;
    if !t.is_expired(now_unix(), 120) {
        return Ok(t);
    }
    tracing::debug!("jira OAuth access token expired — refreshing");
    // Prefer the secret stored at login time so the daemon can refresh even when
    // JIRA_OAUTH_CLIENT_SECRET is absent from its process env (e.g. bundle installs
    // where the daemon reads ~/.meridian/app/.env, not ~/.meridian/.env).
    let mut refresh_spec = spec();
    if !t.client_secret.is_empty() {
        refresh_spec.client_secret = Some(t.client_secret.clone());
    }
    let resp = flow::refresh(&t.client_id, &refresh_spec, &t.refresh_token)
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
    store::save(&t).context("persisting refreshed Jira OAuth tokens")?;
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

/// Decide how to authenticate Jira requests: prefer OAuth when a token store
/// exists (the user has logged in), otherwise fall back to the static API token.
/// API token beats stored OAuth — a set JIRA_API_TOKEN always wins.
/// This mirrors the industry standard (gh, Vercel CLI, Stripe CLI all follow
/// env-var-first) and lets developers use a stable PAT in .env without being
/// blocked by a stale OAuth session stored in ~/.meridian/oauth/jira.json.
pub async fn resolve(jira: &JiraConfig) -> Result<JiraReqCtx> {
    if !jira.base_url.is_empty() && !jira.email.is_empty() && !jira.api_token.is_empty() {
        tracing::debug!(auth_method = "api_token", "resolving Jira auth");
        return Ok(JiraReqCtx::Basic {
            base_url: jira.base_url.clone(),
            email: jira.email.clone(),
            api_token: jira.api_token.clone(),
        });
    }
    if store::exists("jira") {
        tracing::debug!(auth_method = "oauth", "resolving Jira auth");
        let t = ensure_fresh().await?;
        return Ok(JiraReqCtx::OAuth {
            token: t.access_token,
            cloud_id: t.cloud_id,
            site_url: t.site_url,
        });
    }
    bail!(
        "no Jira auth available — run `meridian oauth-login jira`, \
         or set JIRA_BASE_URL / JIRA_EMAIL / JIRA_API_TOKEN"
    )
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
