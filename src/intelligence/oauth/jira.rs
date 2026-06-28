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

/// True when all three Basic-auth fields are non-empty after trimming. Extracted
/// so the eligibility check can be unit-tested without touching the OAuth store.
fn has_basic_auth(jira: &JiraConfig) -> bool {
    !jira.base_url.trim().is_empty()
        && !jira.email.trim().is_empty()
        && !jira.api_token.trim().is_empty()
}

/// Decide how to authenticate Jira requests: prefer the static API token when
/// fully configured, otherwise fall back to a stored OAuth session. API token
/// beats stored OAuth — a set JIRA_API_TOKEN always wins.
/// This mirrors the industry standard (gh, Vercel CLI, Stripe CLI all follow
/// env-var-first) and lets developers use a stable PAT in .env without being
/// blocked by a stale OAuth session stored in ~/.meridian/oauth/jira.json.
pub async fn resolve(jira: &JiraConfig) -> Result<JiraReqCtx> {
    if has_basic_auth(jira) {
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
    use crate::config::JiraConfig;

    fn cfg(base_url: &str, email: &str, api_token: &str) -> JiraConfig {
        JiraConfig {
            base_url: base_url.into(),
            email: email.into(),
            api_token: api_token.into(),
            project_keys: vec![],
        }
    }

    /// When all three API-token fields are populated, `resolve()` must return
    /// `JiraReqCtx::Basic` immediately — no OAuth store access, no network.
    #[tokio::test]
    async fn api_token_beats_oauth_when_fully_configured() {
        let ctx = resolve(&cfg("https://acme.atlassian.net", "user@acme.com", "tok"))
            .await
            .expect("resolve should succeed with API token");
        assert!(matches!(ctx, JiraReqCtx::Basic { .. }));
    }

    /// Each of the three credential fields is required. Asserting the helper
    /// directly avoids environment-dependent behavior (calling resolve() when
    /// a jira OAuth store exists would hit ensure_fresh() and the network).
    #[test]
    fn basic_auth_requires_all_three_fields() {
        assert!(super::has_basic_auth(&cfg(
            "https://acme.atlassian.net",
            "user@acme.com",
            "tok"
        )));
        assert!(!super::has_basic_auth(&cfg(
            "https://acme.atlassian.net",
            "user@acme.com",
            ""
        )));
        assert!(!super::has_basic_auth(&cfg(
            "https://acme.atlassian.net",
            "",
            "tok"
        )));
        assert!(!super::has_basic_auth(&cfg("", "user@acme.com", "tok")));
    }

    #[test]
    fn whitespace_fields_do_not_qualify_for_basic_auth() {
        assert!(!super::has_basic_auth(&cfg(
            "https://acme.atlassian.net",
            "user@acme.com",
            "   "
        )));
        assert!(!super::has_basic_auth(&cfg(
            "https://acme.atlassian.net",
            "   ",
            "tok"
        )));
        assert!(!super::has_basic_auth(&cfg("   ", "user@acme.com", "tok")));
    }
}
