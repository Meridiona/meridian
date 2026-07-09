//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Browser-based OAuth 2.0 (Authorization Code + PKCE) for PM providers. The
// engine (flow, pkce, store, provider specs, login, refresh) lives in the shared
// `meridian-oauth` crate so the **tray** can run the interactive login
// in-process (no `meridian oauth-login` subprocess) while the **daemon** does
// refresh-before-use on the same token store. We re-export those modules here so
// every existing daemon call site (`crate::intelligence::oauth::{store,flow,…}`)
// keeps working unchanged.
//
//   pkce/store/flow/trello — re-exported verbatim from `meridian-oauth`.
//   jira   — re-exports the shared Jira wiring AND adds `resolve()`, the one
//            piece that needs the daemon's `JiraConfig` (so it can't live in the
//            config-free shared crate).
//
// GitHub has no daemon-side OAuth module: it connects via the in-app browser
// device flow (`meridian_oauth::github`, driven by the tray's start_oauth),
// which writes `GITHUB_TOKEN` to `.env` for the daemon to read.

pub use meridian_oauth::{flow, pkce, store, trello};

pub mod jira;
