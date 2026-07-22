//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Writing user-authored tasks into `pm_tasks` — the storage half of "create a task
//! from the daily plan".
//!
//! # What this is
//! Two kinds of row are born here, and they share ONE insert ([`insert_task`]):
//! - a **personal task** (`provider = 'local'`) — a task the user authored that
//!   lives only in Meridian, for people with no tracker (or work that isn't
//!   ticket-worthy);
//! - a **shadow row** for a ticket we just created on a real tracker, when the
//!   post-create sync hasn't landed the authoritative row yet (see
//!   `src/plan_tasks/create.rs` for why the ordering matters).
//!
//! # `'local'` is a RESERVED SENTINEL PROVIDER
//! `pm_tasks` is otherwise a pure provider mirror (migration 038). Storing our own
//! rows in it is safe **only** because every sync's prune is provider-scoped
//! (`DELETE FROM pm_tasks WHERE provider = 'jira' AND …`, and likewise for linear /
//! github / trello / azure_devops); no sync can see, refresh, or delete another
//! provider's rows. That is a structural requirement of multi-tracker support, not a
//! coincidence — but it means `provider` now carries a second meaning ("is this
//! synced at all"), so the sites that must EXCLUDE `'local'` are listed here and
//! must stay in sync:
//!
//! - `src/intelligence/task_triage/store.rs` `load_board` — board hygiene must not
//!   offer tracker fixes for a personal task.
//! - `src/intelligence/task_triage/store.rs` `prune_orphans` — its empty-board guard
//!   protects every human curation decision from a transient sync gap; a local row
//!   would keep the count non-zero forever and silently disable it.
//! - `src/intelligence/mod.rs` `pm_tasks_present` — a non-zero count there means
//!   "a tracker actually works", which a personal task must not fake.
//!
//! Everything else reads `pm_tasks` unscoped ON PURPOSE — that is the payoff:
//! `plan::build_available`, `plan::load_plan`, `task_detail`, `today`, `tasks` and
//! `src/daily_plan.rs`'s nudge all pick personal tasks up for free. Deliberately
//! NOT excluded: `src/pm_worklog/generate.rs`'s worklog matcher, which now treats a
//! personal task as a valid match target and posts to it (writing onto the row
//! itself, or promoting it to a real ticket if a tracker is connected) instead of
//! refusing it.
//!
//! # Who calls this
//! `src/plan_tasks/{create,edit}.rs` (daemon) → the `meridian plan-task-create` /
//! `plan-task-edit` CLIs → the tray's `create_plan_task` / `edit_plan_task` → the
//! daily plan's task composer.
//!
//! # Related
//! - [`crate::plan`] — the daily plan these tasks are added to; its `apply_plan_action`
//!   (`add`) is what puts a freshly-created key into today.

use crate::SqlitePool;
use anyhow::{Context, Result};
use tracing::Instrument;

/// The reserved sentinel provider for user-authored, unsynced tasks. Never a real
/// tracker id — see the module header for the invariant this rests on.
pub const LOCAL_PROVIDER: &str = "local";

/// The `LOCAL-<n>` key prefix. `<n>` is `MAX(<n>) + 1` over existing personal tasks.
/// Keys are never reused — nothing here deletes a task (see the note at the bottom of
/// this module), so the max only ever grows. That matters: a reissued key would let a
/// new task inherit the old one's `app_sessions` / `daily_plan` history.
const LOCAL_KEY_PREFIX: &str = "LOCAL-";

/// One task row to create. `provider` is [`LOCAL_PROVIDER`] for a personal task, or a
/// real tracker id for a shadow row standing in for a just-created ticket.
#[derive(Debug, Clone)]
pub struct NewTask {
    pub key: String,
    pub provider: String,
    pub title: String,
    pub description: String,
    pub issue_type: String,
    pub url: String,
}

/// Insert one task row. The ONE insert both creation paths converge on.
///
/// `ON CONFLICT DO NOTHING` on the `task_key` UNIQUE index makes a double-submit a
/// no-op rather than an error — the caller already holds the key it wanted.
/// `is_terminal = 0` and a non-empty `status_raw` matter: they are what keep the task
/// on the board (`build_available` drops terminal rows) and stop the card rendering
/// as done.
#[tracing::instrument(skip(pool), fields(key = %task.key, provider = %task.provider))]
pub async fn insert_task(pool: &SqlitePool, task: &NewTask, now: &str) -> Result<()> {
    sqlx::query(
        "INSERT INTO pm_tasks \
            (task_key, provider, title, description_text, issue_type, url, \
             status_raw, is_terminal, project_key, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, 'To Do', 0, '', ?) \
         ON CONFLICT(task_key) DO NOTHING",
    )
    .bind(&task.key)
    .bind(&task.provider)
    .bind(&task.title)
    .bind(&task.description)
    .bind(&task.issue_type)
    .bind(&task.url)
    .bind(now)
    .execute(pool)
    .instrument(tracing::debug_span!("task_create.write.pm_tasks"))
    .await
    .with_context(|| format!("inserting task {}", task.key))?;
    tracing::info!("task_create: row inserted");
    Ok(())
}

/// Create a personal task and return its minted key.
///
/// The mint and the insert share ONE transaction: `SELECT MAX(...)` then `INSERT` is
/// a TOCTOU race, and SQLite's write lock is what serialises it. The `task_key`
/// UNIQUE index is the backstop — a loser retries with a fresh number rather than
/// failing the user's create.
#[tracing::instrument(skip(pool))]
pub async fn create_local_task(
    pool: &SqlitePool,
    title: &str,
    description: &str,
    issue_type: &str,
    now: &str,
) -> Result<String> {
    // Bounded retry: only a genuine concurrent create can lose here, and the next
    // number is free. Three attempts is far beyond what one writer can contend with.
    let mut last_err = None;
    for _ in 0..3 {
        match try_create_local(pool, title, description, issue_type, now).await {
            Ok(key) => {
                tracing::info!(key = %key, "task_create: personal task created");
                return Ok(key);
            }
            Err(e) => last_err = Some(e),
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow::anyhow!("could not create the task - try again")))
}

/// One mint+insert attempt, inside a single transaction.
async fn try_create_local(
    pool: &SqlitePool,
    title: &str,
    description: &str,
    issue_type: &str,
    now: &str,
) -> Result<String> {
    let mut tx = pool
        .begin()
        .await
        .context("opening the create transaction")?;

    // Highest existing LOCAL-<n>, counting DELETED keys is impossible (they're gone),
    // so this is "highest live + 1". Keys are not reused; see LOCAL_KEY_PREFIX.
    let max_n: i64 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(CAST(SUBSTR(task_key, 7) AS INTEGER)), 0) \
         FROM pm_tasks WHERE provider = ? AND task_key LIKE 'LOCAL-%'",
    )
    .bind(LOCAL_PROVIDER)
    .fetch_one(&mut *tx)
    .await
    .context("minting the next personal task key")?;

    let key = format!("{LOCAL_KEY_PREFIX}{}", max_n + 1);
    sqlx::query(
        "INSERT INTO pm_tasks \
            (task_key, provider, title, description_text, issue_type, url, \
             status_raw, is_terminal, project_key, updated_at) \
         VALUES (?, ?, ?, ?, ?, '', 'To Do', 0, '', ?)",
    )
    .bind(&key)
    .bind(LOCAL_PROVIDER)
    .bind(title)
    .bind(description)
    .bind(issue_type)
    .bind(now)
    .execute(&mut *tx)
    .await
    .context("inserting the personal task")?;

    tx.commit().await.context("committing the new task")?;
    Ok(key)
}

/// The provider that owns `task_key`, or `None` if it isn't a known task. Callers
/// route on this — see `src/plan_tasks/edit.rs`.
#[tracing::instrument(skip(pool))]
pub async fn provider_of(pool: &SqlitePool, task_key: &str) -> Result<Option<String>> {
    let row: Option<(String,)> = sqlx::query_as("SELECT provider FROM pm_tasks WHERE task_key = ?")
        .bind(task_key)
        .fetch_optional(pool)
        .await
        .context("resolving the task's provider")?;
    Ok(row.map(|(p,)| p))
}

/// True once `task_key` has a `pm_tasks` row. Used to decide whether a just-created
/// ticket needs a shadow row (`src/plan_tasks/create.rs`).
#[tracing::instrument(skip(pool))]
pub async fn task_exists(pool: &SqlitePool, task_key: &str) -> Result<bool> {
    let row: Option<(i64,)> = sqlx::query_as("SELECT 1 FROM pm_tasks WHERE task_key = ?")
        .bind(task_key)
        .fetch_optional(pool)
        .await
        .context("checking whether the task landed")?;
    Ok(row.is_some())
}

/// Rewrite a personal task's title and/or description.
///
/// **Scoped `WHERE provider = 'local'` deliberately**: this must never be pointable at
/// a board ticket. A tracker's copy is authoritative and is edited through
/// `intelligence::ticket_update` instead — writing `pm_tasks` directly for a synced
/// row would be silently clobbered by the next sync's UPSERT. Returns whether a row
/// changed, so the caller can tell "not a personal task" from "no change".
#[tracing::instrument(skip(pool))]
pub async fn update_task_text(
    pool: &SqlitePool,
    task_key: &str,
    title: Option<&str>,
    description: Option<&str>,
    now: &str,
) -> Result<bool> {
    let res = sqlx::query(
        "UPDATE pm_tasks \
         SET title = COALESCE(?, title), \
             description_text = COALESCE(?, description_text), \
             updated_at = ? \
         WHERE task_key = ? AND provider = ?",
    )
    .bind(title)
    .bind(description)
    .bind(now)
    .bind(task_key)
    .bind(LOCAL_PROVIDER)
    .execute(pool)
    .instrument(tracing::debug_span!("task_create.write.update_text"))
    .await
    .context("updating the personal task")?;
    Ok(res.rows_affected() > 0)
}

/// Mark a personal task done / not-done. The tracker equivalent is
/// `ticket_update`'s `close`/`reopen`; same `provider = 'local'` scoping rationale as
/// [`update_task_text`]. A thin wrapper over [`set_local_status`] for the two callers
/// (the plan checkbox, `src/plan_tasks/done.rs`) that only ever mean "done" or "To Do".
#[tracing::instrument(skip(pool))]
pub async fn set_local_terminal(
    pool: &SqlitePool,
    task_key: &str,
    done: bool,
    now: &str,
) -> Result<bool> {
    set_local_status(
        pool,
        task_key,
        if done { "Done" } else { "To Do" },
        done,
        now,
    )
    .await
}

/// One status a personal task can be moved to. Unlike a real tracker, there is no
/// workflow to ask — a personal task always offers exactly this fixed three-status
/// lifecycle (todo/in_progress/done), mirroring the canonical categories a tracker's
/// own statuses are normalised into (see `src/intelligence/ticket_update/statuses.rs`'s
/// `StatusOption`) so the UI's `StatusPicker` treats a personal task identically to a
/// tracker one.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LocalStatusOption {
    pub id: &'static str,
    pub name: &'static str,
    pub category: &'static str,
}

/// The personal-task lifecycle, in display order. `id` doubles as the wire value the
/// UI's `StatusPicker` sends back on a pick — there is no separate tracker-assigned id
/// to preserve, unlike Jira's transition ids.
pub const LOCAL_STATUSES: &[LocalStatusOption] = &[
    LocalStatusOption {
        id: "todo",
        name: "To Do",
        category: "todo",
    },
    LocalStatusOption {
        id: "in_progress",
        name: "In Progress",
        category: "in_progress",
    },
    LocalStatusOption {
        id: "done",
        name: "Done",
        category: "done",
    },
];

/// Resolve a chosen status against [`LOCAL_STATUSES`]: an exact id match wins, else a
/// case-insensitive name match — the same id-or-name contract
/// `ticket_update::statuses::resolve_choice` uses for a real tracker, which is what
/// lets the UI's Undo pass back the previous status's NAME uniformly for either kind
/// of task.
pub fn resolve_local_status(choice: &str) -> Option<&'static LocalStatusOption> {
    LOCAL_STATUSES
        .iter()
        .find(|o| o.id.eq_ignore_ascii_case(choice))
        .or_else(|| {
            LOCAL_STATUSES
                .iter()
                .find(|o| o.name.eq_ignore_ascii_case(choice))
        })
}

/// A personal task's current `(status_raw, is_terminal)`, or `None` if `task_key`
/// isn't a personal task (unknown key, or owned by a real tracker).
#[tracing::instrument(skip(pool))]
pub async fn local_task_current(
    pool: &SqlitePool,
    task_key: &str,
) -> Result<Option<(String, bool)>> {
    let row: Option<(String, i64)> = sqlx::query_as(
        "SELECT status_raw, is_terminal FROM pm_tasks WHERE task_key = ? AND provider = ?",
    )
    .bind(task_key)
    .bind(LOCAL_PROVIDER)
    .fetch_optional(pool)
    .await
    .context("reading the personal task's current status")?;
    Ok(row.map(|(status, terminal)| (status, terminal != 0)))
}

/// Move a personal task to `status_name` (one of [`LOCAL_STATUSES`]'s display names),
/// with `is_terminal` set from that status's category — the same two columns
/// [`set_local_terminal`] used to write directly, now shared with the richer
/// todo/in_progress/done picker.
#[tracing::instrument(skip(pool))]
pub async fn set_local_status(
    pool: &SqlitePool,
    task_key: &str,
    status_name: &str,
    is_terminal: bool,
    now: &str,
) -> Result<bool> {
    let res = sqlx::query(
        "UPDATE pm_tasks SET is_terminal = ?, status_raw = ?, updated_at = ? \
         WHERE task_key = ? AND provider = ?",
    )
    .bind(i64::from(is_terminal))
    .bind(status_name)
    .bind(now)
    .bind(task_key)
    .bind(LOCAL_PROVIDER)
    .execute(pool)
    .await
    .context("updating the personal task's status")?;
    Ok(res.rows_affected() > 0)
}

// NOTE: there is deliberately NO delete. A personal task is disposed of exactly the
// way a board ticket is — mark it done ([`set_local_terminal`]), which drops it from
// `build_available`. Removing a task from a day is the plan's `remove` action, which
// touches `daily_plan` and leaves the task itself alone.
//
// This is not just YAGNI: because keys are minted as `MAX(<n>) + 1` over the live
// rows, deleting the highest task would let the NEXT mint reissue its key, and the
// new task would silently inherit the old one's `app_sessions` history. No delete
// means no reuse, which is what makes the "never reused" guarantee above true.
