//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Shared OAuth/token engine for Meridian's PM providers.
//!
//! Extracted from the daemon's `src/intelligence/oauth/` so the **tray** can run
//! the interactive browser login **in-process** (no `meridian oauth-login`
//! subprocess) while the **daemon** keeps doing refresh-before-use on the same
//! token store. Both crates depend on this one; the daemon re-exports these
//! modules unchanged (`pub use meridian_oauth::{flow, pkce, store, jira, trello}`)
//! so every existing call site keeps compiling.
//!
//! Deliberately **config-free**: this crate has NO dependency on the daemon's
//! `crate::config`. The one piece that needs `JiraConfig` — `jira::resolve()`,
//! which picks OAuth-vs-API-token for a request — stays daemon-side. Everything
//! here takes plain params (client id/secret, port, provider spec) so it is
//! reusable from the tray, the daemon, and tests alike.
//!
//! Modules:
//! - [`pkce`] — per-flow verifier/challenge/state (S256).
//! - [`store`] — `~/.meridian/oauth/<provider>.json`, 0600, refresh-aware.
//! - [`flow`] — generic loopback-redirect engine (browser → code → tokens →
//!   refresh) + the Trello fragment-relay variant.
//! - [`jira`] — Atlassian 3LO wiring: `login`, `ensure_fresh`, `JiraReqCtx`.
//! - [`trello`] — Trello token grant (fragment relay): `login`, `load_token`.
//! - [`github`] — GitHub OAuth device flow (public client, no secret):
//!   `request_device_code`, `poll_for_token`.

pub mod flow;
pub mod github;
pub mod jira;
pub mod pkce;
pub mod store;
pub mod trello;

/// Classify whether an OAuth failure was a transient (retryable) token-endpoint
/// blip rather than a dead grant — so the sync loop keeps quiet on a network
/// hiccup instead of nagging the user to re-authenticate. See [`flow::TokenError`].
pub use flow::is_transient;

/// Serialises tests that mutate process-global env vars (`HOME`,
/// `GITHUB_OAUTH_CLIENT_ID`, …). `cargo test` runs tests in parallel and POSIX
/// `setenv` is process-wide (and not thread-safe), so without this two
/// env-mutating tests race: one clobbers the other's `HOME` mid-run and the
/// path/lock resolution reads the wrong directory (a flaky failure that only
/// surfaced under full-workspace parallelism). Every such test holds this guard
/// for its whole body. Poison-tolerant — a panicking test already fails on its
/// own; don't cascade that into every later test.
#[cfg(test)]
pub(crate) fn env_test_guard() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|p| p.into_inner())
}
