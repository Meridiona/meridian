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
//! - [`notices`]   — clear a fault banner (`/api/notices/[id]` DELETE).
//! - [`notifications`] — the in-app banner dismiss write.
//! - [`parents`]   — valid parent tickets for the hygiene "link a parent" fix.
//! - [`settings`]  — runtime settings read + write (`/api/settings` GET/PUT).
//! - [`tasks`]     — board re-sync action (`/api/tasks/sync`, spawns `meridian`).
//! - [`triage`]    — cleanup working set + the decision/ignore DB writes.
//! - [`setup`]     — first-run detection, permission probes, MLX status/start.
//! - [`version`]   — installed vs. published version (`/api/version`).
//! - [`worklogs`]  — worklog review read + edit/approve/reject/unapprove writes.
//!
//! # Related
//! - [`crate::install`] — install-mode + db-path resolution the commands consume.
//! - [`crate::sys`] — shared uid / notify / ui_base helpers.
//! - [`crate::mlx_server`] — the MLX process manager the setup commands drive.

pub mod daemon;
pub mod dashboard;
pub mod health;
pub mod integrations;
pub mod logs;
pub mod notices;
pub mod notifications;
pub mod openobserve;
pub mod parents;
pub mod settings;
pub mod setup;
pub mod system;
pub mod tasks;
pub mod triage;
pub mod version;
pub mod worklogs;

// Glob re-exports so callers use `crate::commands::<fn>` regardless of submodule.
// Globs (not explicit names) are required: the `#[tauri::command]` macro emits
// hidden sibling items (`__cmd__*`) that `generate_handler!` resolves through
// this path, and only a glob carries them along with the command fn.
pub use daemon::*;
pub use dashboard::*;
pub use health::*;
pub use integrations::*;
pub use logs::*;
pub use notices::*;
pub use notifications::*;
pub use openobserve::*;
pub use parents::*;
pub use settings::*;
pub use setup::*;
pub use system::*;
pub use tasks::*;
pub use triage::*;
pub use version::*;
pub use worklogs::*;
