//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! The daily plan's task-composer commands (tray side).
//!
//! # What this is
//! Three commands backing "create your own task" on the daily plan:
//! - [`draft_plan_task`] — shape a rough note into `{title, description, issue_type}`
//!   with the user's configured LLM.
//! - [`create_plan_task`] — create the task (personal, or filed on a tracker) and add
//!   it to the day's plan.
//! - [`edit_plan_task`] — rewrite a task's title/description, wherever it lives.
//! - [`set_plan_task_done`] — tick/untick a task, wherever it lives.
//! - [`delete_plan_task`] — hide a personal task everywhere it's listed. Refuses
//!   anything synced to a tracker — that ticket isn't Meridian's to delete.
//!
//! # Why these SHELL OUT
//! The chosen LLM provider (`settings.json`) and tracker auth (`~/.meridian/.env`)
//! both live in the daemon, so — exactly like [`crate::commands::worklog_generate`] —
//! these spawn the `meridian` CLI rather than talking to a model or a tracker
//! in-process. The `run_meridian_json` helper is reused from
//! [`crate::commands::cli_exec`] rather than re-implemented; the CLI logs before
//! its result line, which is why the LAST non-empty stdout line is the payload.
//!
//! # Who calls this
//! Registered in `lib.rs`'s `invoke_handler!`; consumed by the plan's task composer
//! via `ui/lib/bridge.ts` — `invoke('draft_plan_task', {note})` (flat args, camelCase
//! across the bridge) and `mutate('…','create_plan_task', {…})` (a `body` struct whose
//! serde field names stay snake_case).
//!
//! # Related
//! - `src/plan_tasks/` — the CLI-side engine and the source of the JSON shapes below.
//! - [`meridian_core::task_create`] — why a personal task is a `provider='local'` row.

use serde::{Deserialize, Serialize};
use std::time::Duration;

use super::cli_exec::run_meridian_json;

/// An LLM call — same budget as the other model-backed commands.
const LLM_TIMEOUT: Duration = Duration::from_secs(150);
/// A create/edit: a tracker round trip plus a force-sync, or a local DB write.
const WRITE_TIMEOUT: Duration = Duration::from_secs(90);

/// A drafted task — mirrors `plan_tasks::draft::TaskDraft`.
///
/// `error` is a SOFT failure: the model was unreachable or unparseable, the fields are
/// empty, and the composer shows them for manual entry. It is never a reason to fail
/// the command — creation must not depend on the AI.
///
/// `provider_down` splits that soft failure in two, because the composer offers opposite
/// affordances for them — the AI picker for a broken engine, a retry for a bad answer.
/// `#[serde(default)]` so an older `meridian` CLI on the machine (the binary is resolved
/// at runtime, not bundled) still deserializes; a missing flag means "not known to be the
/// engine", which is the safe read.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskDraft {
    pub title: String,
    pub description: String,
    pub issue_type: String,
    pub error: Option<String>,
    #[serde(default)]
    pub provider_down: bool,
}

/// POST body for [`create_plan_task`] — `mutate` wraps args in `{body}`.
#[derive(Debug, Deserialize)]
pub struct CreatePlanTaskBody {
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub issue_type: String,
    /// `local` (personal) or a provider id (`jira`, …). Absent = personal.
    #[serde(default)]
    pub target: String,
    /// The day to add it to. Absent = today (the CLI resolves it).
    #[serde(default)]
    pub day: String,
}

/// [`create_plan_task`] response — mirrors `plan_tasks::create::CreatedTask`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreatedTask {
    pub task_key: String,
    pub provider: String,
    pub synced: bool,
    /// A soft caveat to show the user (e.g. the ticket was filed but isn't on their
    /// board yet). Not an error.
    pub note: Option<String>,
}

/// POST body for [`edit_plan_task`].
#[derive(Debug, Deserialize)]
pub struct EditPlanTaskBody {
    pub task_key: String,
    /// `None` leaves the field alone (vs `Some("")`, which the CLI rejects for a title).
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
}

/// POST body for [`set_plan_task_done`].
#[derive(Debug, Deserialize)]
pub struct SetPlanTaskDoneBody {
    pub task_key: String,
    /// `true` = mark done, `false` = reopen.
    pub done: bool,
}

/// [`edit_plan_task`] / [`set_plan_task_done`] response — mirrors
/// `plan_tasks::edit::EditResult`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditResult {
    pub task_key: String,
    pub provider: String,
    /// `applied` | `redirected` (this tracker has no API for it — open it instead).
    pub status: String,
    pub browse_url: Option<String>,
    pub reason: Option<String>,
}

/// [`delete_plan_task`] response — mirrors `plan_tasks::delete::DeleteResult`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeletePlanTaskResult {
    pub task_key: String,
    pub deleted: bool,
}

/// Draft a task from a rough note. Spawns `meridian plan-task-draft --note <n>`.
///
/// Only errors when the CLI itself failed (spawn/timeout). A model failure comes back
/// as `Ok` with `TaskDraft::error` set — see the module header.
#[tauri::command]
#[tracing::instrument(skip(note), fields(note_len = note.len()))]
pub async fn draft_plan_task(note: String) -> Result<TaskDraft, String> {
    if note.trim().is_empty() {
        return Err("a note is required".to_string());
    }
    let draft: TaskDraft = run_meridian_json(
        &["plan-task-draft", "--note", &note],
        LLM_TIMEOUT,
        "plan-task-draft",
    )
    .await?;
    tracing::info!(
        issue_type = %draft.issue_type,
        drafted = draft.error.is_none(),
        "plan-task-draft served"
    );
    Ok(draft)
}

/// Create a task and add it to the day's plan. Spawns
/// `meridian plan-task-create --title … [--target …]`.
#[tauri::command]
#[tracing::instrument(skip(body), fields(target = %body.target, day = %body.day))]
pub async fn create_plan_task(body: CreatePlanTaskBody) -> Result<CreatedTask, String> {
    if body.title.trim().is_empty() {
        return Err("a title is required".to_string());
    }
    let mut args: Vec<&str> = vec!["plan-task-create", "--title", &body.title];
    if !body.description.trim().is_empty() {
        args.extend_from_slice(&["--description", &body.description]);
    }
    if !body.issue_type.trim().is_empty() {
        args.extend_from_slice(&["--issue-type", &body.issue_type]);
    }
    if !body.target.trim().is_empty() {
        args.extend_from_slice(&["--target", &body.target]);
    }
    if !body.day.trim().is_empty() {
        args.extend_from_slice(&["--day", &body.day]);
    }

    let created: CreatedTask = run_meridian_json(&args, WRITE_TIMEOUT, "plan-task-create").await?;
    tracing::info!(
        task_key = %created.task_key,
        provider = %created.provider,
        synced = created.synced,
        "plan-task-create served"
    );
    tauri::async_runtime::spawn(crate::counter_ping::ping_task_update());
    Ok(created)
}

/// Rewrite a task's title and/or description. Spawns
/// `meridian plan-task-edit --key K [--title T] [--description D]`.
#[tauri::command]
#[tracing::instrument(skip(body), fields(task_key = %body.task_key))]
pub async fn edit_plan_task(body: EditPlanTaskBody) -> Result<EditResult, String> {
    if body.task_key.trim().is_empty() {
        return Err("a task key is required".to_string());
    }
    if body.title.is_none() && body.description.is_none() {
        return Err("nothing to change".to_string());
    }
    let mut args: Vec<&str> = vec!["plan-task-edit", "--key", &body.task_key];
    if let Some(t) = &body.title {
        args.extend_from_slice(&["--title", t]);
    }
    if let Some(d) = &body.description {
        args.extend_from_slice(&["--description", d]);
    }

    let res: EditResult = run_meridian_json(&args, WRITE_TIMEOUT, "plan-task-edit").await?;
    tracing::info!(status = %res.status, provider = %res.provider, "plan-task-edit served");
    // Only count a real mutation: a `redirected` result wrote nothing (the
    // tracker has no API for it, so we just hand back a browse URL).
    if res.status == "applied" {
        tauri::async_runtime::spawn(crate::counter_ping::ping_task_update());
    }
    Ok(res)
}

/// Mark a task done / not-done. Spawns `meridian plan-task-done --key K --done B`.
///
/// This exists INSTEAD of the plan's checkbox calling `apply_ticket_fix` directly: that
/// path goes straight to `ticket-update`, which only knows real trackers, so a personal
/// (`provider = 'local'`) task failed with `provider "local" is not configured`. The
/// CLI branches on who owns the task — see `src/plan_tasks/done.rs`.
#[tauri::command]
#[tracing::instrument(skip(body), fields(task_key = %body.task_key, done = body.done))]
pub async fn set_plan_task_done(body: SetPlanTaskDoneBody) -> Result<EditResult, String> {
    if body.task_key.trim().is_empty() {
        return Err("a task key is required".to_string());
    }
    let done = if body.done { "true" } else { "false" };
    let args: Vec<&str> = vec!["plan-task-done", "--key", &body.task_key, "--done", done];

    let res: EditResult = run_meridian_json(&args, WRITE_TIMEOUT, "plan-task-done").await?;
    tracing::info!(status = %res.status, provider = %res.provider, "plan-task-done served");
    // Only count a real mutation: a `redirected` result wrote nothing (the
    // tracker has no API for it, so we just hand back a browse URL).
    if res.status == "applied" {
        tauri::async_runtime::spawn(crate::counter_ping::ping_task_update());
    }
    Ok(res)
}

/// Hide a personal task everywhere it's listed. Spawns
/// `meridian plan-task-delete --key K`.
///
/// The daemon-side CLI refuses anything not `provider = 'local'` — a tracker ticket
/// isn't Meridian's to delete — so a mis-pointed call surfaces as a clear error
/// rather than silently doing nothing.
#[tauri::command]
#[tracing::instrument(fields(task_key = %task_key))]
pub async fn delete_plan_task(task_key: String) -> Result<DeletePlanTaskResult, String> {
    if task_key.trim().is_empty() {
        return Err("a task key is required".to_string());
    }
    let args: Vec<&str> = vec!["plan-task-delete", "--key", &task_key];

    let res: DeletePlanTaskResult =
        run_meridian_json(&args, WRITE_TIMEOUT, "plan-task-delete").await?;
    tracing::info!(deleted = res.deleted, "plan-task-delete served");
    Ok(res)
}
