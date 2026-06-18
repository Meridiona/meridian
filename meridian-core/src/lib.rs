//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! meridian-core — the lean shared data layer used by BOTH the daemon and the
//! dashboard/Tauri app: DB row types + read queries + a no-migration opener.
//!
//! Single source of truth: the daemon re-exports these (so its code is
//! unchanged) and the Tauri app depends on this crate directly — neither
//! reimplements the queries, and the UI no longer pulls the daemon's deps.
//!
//! # Layout
//! Code is organized into folders, but the **public API is flat and stable** —
//! this root re-exports each module so consumers name them as
//! `meridian_core::today`, `::intervals`, `::open_existing`, … regardless of
//! where the file lives. Adding/moving a file never changes a caller's path.
//!
//! - [`db`] — the no-migration opener + the raw `active_session` row.
//! - [`readers`] — the ported `/api/*` DB readers (one module per route).
//! - [`util`] — DB-free helpers (interval math, local-day bounds, hygiene mapping).
//! - [`settings`] — the `settings.json` runtime config reader.

// Re-export the pool type so consumers can name it as `meridian_core::SqlitePool`
// without adding `sqlx` to their own Cargo.toml.
pub use sqlx::SqlitePool;

// ── Internal organization ───────────────────────────────────────────────────
mod db;
mod readers;
mod util;

// ── Public config module (kept top-level; daemon re-exports it) ──────────────
/// Runtime settings (settings.json) — shared by the daemon (re-exported) and the app.
pub mod settings;

/// Notification delivery policy + native pending queue (ported from lib/notifications.ts).
pub mod notifications;

// ── Curated public API: flat module paths, stable across file moves ──────────
pub use db::{get_active_session, open_existing, ActiveSession};

pub use util::{date, hygiene, intervals};

pub use readers::{active, coding_agents, integrations, tasks, today, triage, week, worklogs};
