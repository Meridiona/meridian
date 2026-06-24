//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! The ported `/api/*` DB readers — each module is a faithful Rust port of one
//! Next.js route handler (a SELECT → response-shaping pipeline over `meridian.db`).
//!
//! Every reader here is re-exported at the crate root (`meridian_core::today`,
//! `::tasks`, …) so consumers (the daemon, the tray commands, the tests) name
//! them unchanged; `readers` is internal organization only. Response types are
//! co-located with the query that produces them.
//!
//! # Related
//! - [`crate::util`] — the DB-free math/mapping helpers these readers reuse.
//! - [`crate::db`] — the no-migration opener + the raw `active_session` row.

/// The `/api/active` dashboard view of the active session (ported from active/route.ts).
pub mod active;

/// The `/api/coding-agents` daily agent totals (ported from coding-agents/route.ts).
pub mod coding_agents;

/// The menu-bar pill's "current task" + progress-ring fill (tray-only; no route).
pub mod current_task;

/// The DB half of `/api/integrations` (pm_sync_state errors; ported from integrations/route.ts).
pub mod integrations;

/// The `/api/tasks` per-task time + hygiene payload (ported from tasks/route.ts).
pub mod tasks;

/// The `/api/notices/[id]` DELETE — clear a fault banner (ported from notices/[id]/route.ts).
pub mod notices;

/// The `/api/plan` GET + POST — daily plan board scoring + writes (ported from plan/route.ts).
pub mod plan;

/// The `/api/plan/task` single-ticket detail (ported from plan/task/route.ts).
pub mod task_detail;

/// The `/api/today` dashboard payload, computed in Rust (ported from today/route.ts).
pub mod today;

/// The `/api/triage` cleanup working set (ported from triage/route.ts).
pub mod triage;

/// The `/api/week` 7-day summary, computed in Rust (ported from week/route.ts).
pub mod week;

/// The `/api/worklogs` day review payload (ported from worklogs/route.ts).
pub mod worklogs;
