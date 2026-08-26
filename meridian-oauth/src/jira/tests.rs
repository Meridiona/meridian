//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Tests for the Jira OAuth flow and token handling.
//!
//! Split out of `mod.rs` for the 500-line file cap; they are this module's own unit
//! tests. The `current_if_valid` cases pin the attended/unattended split - the rule
//! that unattended code may USE a valid access token but must never MINT one, which
//! is what stops a background refresh being destroyed by a laptop suspend.

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

/// Seed a jira token store under a private `$HOME` with `expires_at` at
/// `now + offset_secs`. Returns the guard that keeps `$HOME` stable for the
/// duration of the test.
fn seed_store(tag: &str, offset_secs: i64) -> std::sync::MutexGuard<'static, ()> {
    let guard = crate::env_test_guard();
    let dir = std::env::temp_dir().join(format!("meridian_oauth_civ_{tag}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::env::set_var("HOME", &dir);
    store::save(&OAuthTokens {
        provider: "jira".into(),
        client_id: "cid".into(),
        access_token: "access".into(),
        refresh_token: "refresh".into(),
        expires_at: now_unix() + offset_secs,
        scopes: String::new(),
        cloud_id: "cloud-xyz".into(),
        site_url: "https://acme.atlassian.net".into(),
    })
    .expect("seeding the token store should succeed");
    guard
}

/// A comfortably-valid access token is handed straight back, so an unattended
/// sweep can still refresh the ticket cache without touching the network.
#[test]
fn current_if_valid_returns_a_live_token() {
    let _g = seed_store("live", 3600);
    let t = current_if_valid().expect("a token an hour from expiry must be usable");
    assert_eq!(t.access_token, "access");
    assert_eq!(t.cloud_id, "cloud-xyz");
}

/// THE POINT OF THE FUNCTION: an expired token yields `None` rather than
/// triggering a refresh. Spending the rotating refresh token unattended is what
/// permanently kills a grant when the machine suspends mid-POST, so a
/// clock-driven caller must defer instead.
#[test]
fn current_if_valid_refuses_to_mint_when_expired() {
    let _g = seed_store("expired", -10);
    assert!(
        current_if_valid().is_none(),
        "an expired token must NOT be returned - the caller would use it and 401, \
         and the whole point is to defer to the next attended request"
    );
}

/// The skew is applied, not just raw expiry: a token inside the margin counts as
/// expired so a request can't be issued with one that dies mid-flight.
#[test]
fn current_if_valid_applies_the_expiry_skew() {
    let _g = seed_store("skew", EXPIRY_SKEW_SECS - 10);
    assert!(
        current_if_valid().is_none(),
        "a token inside the {EXPIRY_SKEW_SECS}s skew must be treated as expired"
    );
}

/// No store at all is a `None`, never a panic or an error — an install that has
/// never connected Jira must sweep quietly.
#[test]
fn current_if_valid_handles_a_missing_store() {
    let _g = crate::env_test_guard();
    let dir = std::env::temp_dir().join(format!("meridian_oauth_civ_none_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::env::set_var("HOME", &dir);
    assert!(current_if_valid().is_none());
}
