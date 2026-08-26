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
/// so the eligibility check can be unit-tested without touching the OAuth store,
/// and so callers outside this module (e.g. the sync-error remedy mapping in
/// `providers::jira`) can tell which auth path a failure came from.
pub(crate) fn has_basic_auth(jira: &JiraConfig) -> bool {
    !jira.base_url.trim().is_empty()
        && !jira.email.trim().is_empty()
        && !jira.api_token.trim().is_empty()
}

/// The Basic-auth context from config. Shared by both resolvers so the two cannot
/// disagree about which config fields make up a request context — a field added to
/// [`JiraReqCtx::Basic`] has exactly one place to be filled in.
fn basic_ctx(jira: &JiraConfig) -> JiraReqCtx {
    JiraReqCtx::Basic {
        base_url: jira.base_url.clone(),
        email: jira.email.clone(),
        api_token: jira.api_token.clone(),
    }
}

/// The OAuth context from a token the caller has already obtained. Deliberately takes
/// the tokens rather than fetching them: that is the whole difference between the two
/// resolvers ([`resolve`] may mint a new pair, `resolve_unattended` must not), and
/// keeping the fetch out of here means neither can acquire one by accident.
fn oauth_ctx(t: meridian_oauth::store::OAuthTokens) -> JiraReqCtx {
    JiraReqCtx::OAuth {
        token: t.access_token,
        cloud_id: t.cloud_id,
        site_url: t.site_url,
    }
}

/// Decide how to authenticate Jira requests: prefer the static API token when
/// fully configured, otherwise fall back to a stored OAuth session. API token
/// beats stored OAuth — a set JIRA_API_TOKEN always wins.
/// This mirrors the industry standard (gh, Vercel CLI, Stripe CLI all follow
/// env-var-first) and lets developers use a stable PAT in .env without being
/// blocked by a stale OAuth session stored in ~/.meridian/oauth/jira.json.
///
/// **May MINT a new refresh token, so only call this behind a real user action.**
/// See [`resolve_unattended`] for the clock-driven counterpart and why the
/// distinction is load-bearing.
pub async fn resolve(jira: &JiraConfig) -> Result<JiraReqCtx> {
    if has_basic_auth(jira) {
        tracing::debug!(auth_method = "api_token", "resolving Jira auth");
        return Ok(basic_ctx(jira));
    }
    if store::exists("jira") {
        tracing::debug!(auth_method = "oauth", "resolving Jira auth");
        return Ok(oauth_ctx(ensure_fresh().await?));
    }
    bail!(
        "no Jira auth available — run `meridian oauth-login jira`, \
         or set JIRA_BASE_URL / JIRA_EMAIL / JIRA_API_TOKEN"
    )
}

/// [`resolve`] for callers running on a CLOCK rather than behind a user action:
/// returns auth only if it can be had without spending the rotating refresh token.
/// `None` means "skip this pass", never an error.
///
/// # Why a separate resolver
///
/// Refreshing an Atlassian OAuth token is a single-use exchange: the old refresh
/// token dies the instant the new one is issued, so a lost response leaves the
/// grant recoverable only inside a 10-minute window. When that POST is fired by a
/// timer with nobody at the machine, a closing laptop lid destroys the grant
/// permanently — which is precisely how a production install lost Jira for five
/// days (refresh at 18:26:55, 28-minute suspend, retry instant on wake at
/// 18:55:29 but 18 minutes too late).
///
/// So unattended code may USE a valid access token but must never MINT one. This
/// is the same discipline that makes Claude Code's MCP connections durable against
/// the identical protocol: it only ever refreshes on the tail of something a human
/// just asked for.
///
/// API-token (basic) auth is returned unconditionally — a static token has no
/// expiry to race and no rotating credential to lose, so there is nothing to
/// protect it from.
///
/// Deferring is close to free: a sweep that finds an expired token is by
/// definition running while nobody is using the machine, so there is little new
/// activity to match, and the next attended request refreshes properly.
///
/// # Related
/// - [`resolve`] — the attended counterpart, which may refresh.
/// - [`meridian_oauth::jira::current_if_valid`] — the non-minting token read.
pub fn resolve_unattended(jira: &JiraConfig) -> Option<JiraReqCtx> {
    if has_basic_auth(jira) {
        tracing::debug!(
            auth_method = "api_token",
            "resolving Jira auth (unattended)"
        );
        return Some(basic_ctx(jira));
    }
    if !store::exists("jira") {
        return None;
    }
    // `current_if_valid`, never `ensure_fresh` — this is the line that makes the
    // function unattended-safe, and swapping it would silently reintroduce the
    // timer-driven refresh that killed a production grant.
    let t = meridian_oauth::jira::current_if_valid()?;
    tracing::debug!(auth_method = "oauth", "resolving Jira auth (unattended)");
    Some(oauth_ctx(t))
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
