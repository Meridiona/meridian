//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Daemon-side Jira OAuth glue. The engine — `login`, `ensure_fresh`,
// `JiraReqCtx`, the provider spec, client-id/secret/port resolvers, cloud-id
// discovery — lives in the shared `meridian-oauth` crate and is re-exported
// verbatim below, so every call site (`oauth::jira::login`,
// `oauth::jira::JiraReqCtx`, …) is unchanged.
//
// The ONE piece that stays here is `resolve()`: it depends on the daemon's
// `JiraConfig` (to choose OAuth vs the static API-token fallback), which the
// config-free shared crate can't see. Keeping it daemon-side is what lets the
// shared crate stay dependency-light enough for the tray to embed.

use anyhow::{bail, Result};

use super::store;
use crate::config::JiraConfig;

// Re-export the entire shared Jira surface (login, ensure_fresh, JiraReqCtx,
// client_id/secret/redirect_port, DEFAULT_*) so daemon call sites keep their
// existing `oauth::jira::*` paths.
pub use meridian_oauth::jira::*;

/// Decide how to authenticate Jira requests: prefer the static API token when
/// fully configured, otherwise fall back to a stored OAuth session. API token
/// beats stored OAuth — a set JIRA_API_TOKEN always wins.
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
