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

pub mod flow;
pub mod jira;
pub mod pkce;
pub mod store;
pub mod trello;
