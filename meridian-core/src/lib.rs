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
pub mod adapters;
pub mod canonical_task;

mod db;
/// The user's centralised AI-provider choice (which LLM runs their pipeline).
pub mod llm_provider;
mod readers;
mod util;

/// Small crash-safe filesystem helpers (atomic JSON write) shared by the
/// daemon, the app config layer, and the tray.
pub mod fs_utils;

// ── Public config module (kept top-level; daemon re-exports it) ──────────────
/// Runtime settings (settings.json) — shared by the daemon (re-exported) and the app.
pub mod settings;

/// Notification delivery policy + native pending queue (ported from lib/notifications.ts).
pub mod notifications;

/// The `~/.meridian/plan_auto_opened` marker format — written by the tray's
/// daily planner auto-open, read by the daemon's plan-nudge hold-back.
pub mod plan_marker;

/// In-process capture-frame writer (Gap-2 Bucket 2). Inverted ownership: the
/// tray writes `capture_frames`, the daemon's ETL reads it.
pub mod capture;

// ── Curated public API: flat module paths, stable across file moves ──────────
pub use db::{get_active_session, open_existing, ActiveSession};

pub use capture::{
    insert_capture_frame, insert_capture_ui_event, insert_pause_gap, CaptureFrameInsert,
    CaptureUiEventInsert,
};

pub use util::{date, hygiene, intervals};

pub use readers::{
    active, coding_agents, current_task, day_tasks, hour_status, hour_text, integrations, notices,
    plan, proposed, task_detail, tasks, today, triage, week, worklogs,
};

pub use canonical_task::{CanonicalTask, PersonRef, Priority, Provider, StatusCategory, TaskKind};

pub use llm_provider::LlmProvider;

pub use adapters::ProviderAdapter;
