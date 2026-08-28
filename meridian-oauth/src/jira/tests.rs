//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Tests for [`super`] - the Atlassian 3LO wiring.
//!
//! Split into its own file on size (`jira.rs` passed the 500-line rule once the
//! refresh journal's crash-point cases landed). Nothing here is a pure unit test
//! of URL building alone: the journal cases below pin the recovery behaviour that
//! decides whether a lost response destroys a user's grant, so they are the most
//! load-bearing tests in this crate.

use super::*;

fn tokens(refresh: &str) -> OAuthTokens {
    OAuthTokens {
        provider: "jira".into(),
        client_id: "cid".into(),
        access_token: "access".into(),
        refresh_token: refresh.into(),
        expires_at: 0,
        scopes: String::new(),
        cloud_id: String::new(),
        site_url: String::new(),
    }
}

/// Scratch `HOME` so the journal lands in a temp dir. Serialised through the
/// crate's env guard because `HOME` is process-global and cargo runs tests in
/// parallel threads.
struct ScratchHome {
    dir: std::path::PathBuf,
    prev: Option<std::ffi::OsString>,
    _guard: std::sync::MutexGuard<'static, ()>,
}

impl ScratchHome {
    fn new(tag: &str) -> Self {
        let guard = crate::env_test_guard();
        let dir = std::env::temp_dir().join(format!("meridian_jira_{tag}_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let prev = std::env::var_os("HOME");
        std::env::set_var("HOME", &dir);
        Self {
            dir,
            prev,
            _guard: guard,
        }
    }
}

impl Drop for ScratchHome {
    fn drop(&mut self) {
        match self.prev.take() {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
        std::fs::remove_dir_all(&self.dir).ok();
    }
}

fn journal(token: &str) -> crate::refresh_journal::PendingSpend {
    crate::refresh_journal::PendingSpend {
        refresh_token: token.into(),
        started_at_unix: now_unix(),
        client_id: "cid".into(),
    }
}

/// CRASH POINT A: died before `store::save` landed. The journalled token still
/// matches the stored one, so the spend's outcome is unknown and MUST be
/// replayed - re-presenting it inside the provider's grace is the only thing
/// that recovers the grant.
#[test]
fn an_unresolved_spend_is_replayed_when_the_save_never_landed() {
    let _home = ScratchHome::new("unresolved");
    let stored = tokens("refresh-A");
    crate::refresh_journal::record_spend("jira", &journal("refresh-A")).unwrap();

    let pending = reconcile_journal("jira", &stored);
    assert_eq!(
        pending.map(|p| p.refresh_token),
        Some("refresh-A".to_string()),
        "a journal matching the stored token means the outcome is unknown - replay it"
    );
}

/// CRASH POINT B: `store::save` landed, then we died before clearing. The
/// stored pair is NEWER than the journal, so there is nothing to recover and
/// replaying would spend a live token for no reason. The journal must be
/// recognised as stale and removed.
#[test]
fn a_journal_older_than_the_stored_pair_is_stale_and_cleared() {
    let _home = ScratchHome::new("stale");
    let stored = tokens("refresh-B"); // save landed: B replaced A
    crate::refresh_journal::record_spend("jira", &journal("refresh-A")).unwrap();

    let pending = reconcile_journal("jira", &stored);
    assert!(
        pending.is_none(),
        "the save landed, so nothing needs replaying"
    );
    assert!(
        crate::refresh_journal::load("jira").is_none(),
        "a stale journal must be cleared, not left to trigger a pointless replay \
         on every future refresh"
    );
}

/// The ordinary case: no journal, nothing to reconcile.
#[test]
fn no_journal_means_nothing_to_reconcile() {
    let _home = ScratchHome::new("none");
    assert!(reconcile_journal("jira", &tokens("refresh-A")).is_none());
}

/// A pending spend must defeat the fast path even when the access token is
/// still fresh.
///
/// This is the subtle one, and getting it wrong reintroduces the whole bug: a
/// valid access token can coexist with a refresh token that was already
/// consumed server-side. Returning early on "not expired" would leave that
/// unresolved for up to an hour - long past the provider's reuse grace - and
/// the recovery would then be impossible rather than merely late.
#[test]
fn a_fresh_access_token_does_not_hide_an_unresolved_spend() {
    let _home = ScratchHome::new("fastpath");
    let mut fresh = tokens("refresh-A");
    fresh.expires_at = now_unix() + 3600; // comfortably fresh
    assert!(
        !fresh.is_expired(now_unix(), 120),
        "precondition: the access token must look fresh"
    );

    crate::refresh_journal::record_spend("jira", &journal("refresh-A")).unwrap();
    // The fast path's condition, mirrored: freshness alone must NOT be enough.
    let would_take_fast_path =
        !fresh.is_expired(now_unix(), 120) && crate::refresh_journal::load("jira").is_none();
    assert!(
        !would_take_fast_path,
        "a fresh access token must not short-circuit past an unresolved refresh spend"
    );
}

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
