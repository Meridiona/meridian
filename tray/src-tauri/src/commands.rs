//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Tauri command surface, grouped by domain.
//!
//! This module is the root for every `#[tauri::command]` the tray exposes. The
//! commands live in domain submodules; this file re-exports each command at the
//! `crate::commands::*` path so `lib.rs`'s `invoke_handler!` and other callers
//! name them flatly regardless of which submodule they sit in.
//!
//! - [`dashboard`] — the ported `/api/*` DB reads (active/today/week/tasks/…).
//! - [`daemon`]    — daemon lifecycle (restart/pause/resume) + status probes.
//! - [`system`]    — OS/window actions (open URLs, System Settings panes).
//! - [`health`]    — the `/api/health` check (also reused by [`crate::poll`]).
//! - [`logs`]      — the `/api/logs` tail.
//! - [`openobserve`] — the `/api/openobserve` service status probe.
//! - [`integrations`] — which trackers are connected (`/api/integrations`).
//! - [`parents`]   — valid parent tickets for the hygiene "link a parent" fix.
//! - [`version`]   — installed vs. published version (`/api/version`).
//!
//! # Related
//! - [`crate::install`] — install-mode + db-path resolution the commands consume.
//! - [`crate::sys`] — shared uid / notify / ui_base helpers.

pub mod daemon;
pub mod dashboard;
pub mod health;
pub mod integrations;
pub mod logs;
pub mod openobserve;
pub mod parents;
pub mod system;
pub mod version;

// Glob re-exports so callers use `crate::commands::<fn>` regardless of submodule.
// Globs (not explicit names) are required: the `#[tauri::command]` macro emits
// hidden sibling items (`__cmd__*`) that `generate_handler!` resolves through
// this path, and only a glob carries them along with the command fn.
pub use daemon::*;
pub use dashboard::*;
pub use health::*;
pub use integrations::*;
pub use logs::*;
pub use openobserve::*;
pub use parents::*;
pub use system::*;
pub use version::*;
