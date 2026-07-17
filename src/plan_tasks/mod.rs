//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! User-authored tasks for the daily plan — draft one from a rough note, create it
//! (personal or synced to a tracker), and edit it afterwards.
//!
//! # Why this lives in the daemon
//! Both halves need things only the daemon has: [`draft`] makes an LLM call (the
//! provider is read per-call from `settings.json`), and [`create`]/[`edit`] can hit a
//! tracker's API (auth lives in `~/.meridian/.env`). The tray therefore SHELLS OUT to
//! the `meridian plan-task-*` CLIs rather than doing any of this in-process — the same
//! rule `src/pm_worklog/generate.rs` follows.
//!
//! # The shape
//! ```text
//! note ──draft (LLM)──▶ {title, description, issue_type} ──review/edit──▶ create
//!                                                                            │
//!                                        personal ◀──────────┴──────────▶ tracker
//!                                            │                                │
//!                                            └────▶ pm_tasks row ◀────────────┘
//!                                                        │
//!                                              plan `add` → today's focus
//! ```
//! Storage for both branches is one `pm_tasks` row — see
//! [`meridian_core::task_create`] for why `provider = 'local'` is safe there and which
//! sites must exclude it.
//!
//! # Related
//! - [`crate::pm_worklog::create`] — the tracker-side CREATE this reuses.
//! - [`crate::intelligence::ticket_update`] — the tracker-side EDIT [`edit`] routes to.

pub mod cli;
pub mod create;
pub mod draft;
pub mod edit;
