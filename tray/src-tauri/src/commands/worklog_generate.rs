//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! The day-task "Generate worklog" commands, ported to Rust (tray side).
//!
//! # What this is
//! Three commands backing the "Generate worklog" action on a day-task workstream
//! card:
//! - [`generate_day_task_worklog`] — run (or regenerate) the AI draft: match the
//!   day-task to the tickets it advanced (or propose one) + a status update.
//! - [`get_day_task_worklog`] — read an existing draft on panel reopen (or `None`).
//! - [`approve_day_task_worklog`] — approve → create-if-proposed → post the comment
//!   on every target → link the day-task. Retry-safe after a partial post: a ticket
//!   that already took the comment is never posted to twice.
//!
//! The per-ticket edits ([`crate::commands::retarget_day_task_worklog`],
//! [`crate::commands::dismiss_worklog_target`]) are NOT here — they touch neither
//! tracker auth nor a model, so they are direct `meridian-core` calls in
//! [`crate::commands::dashboard`] rather than CLI shell-outs.
//!
//! Tracker auth + the chosen LLM provider live in the daemon (`~/.meridian/.env` /
//! `settings.json`), so — exactly like [`crate::commands::list_task_statuses`] —
//! these **shell out to the `meridian` CLI** (`worklog-generate` /
//! `worklog-generate-get` / `worklog-generate-approve`) rather than talking to any
//! tracker or model in-process.
//!
//! # Who calls this
//! Registered in `lib.rs`'s `invoke_handler!`; consumed by the dashboard's
//! `DayTaskDetailPanel` via `ui/lib/bridge.ts` — `load('…','get_day_task_worklog',
//! {day, taskId})` and `invoke('generate_day_task_worklog',{day, taskId})` (flat
//! Tauri args, which cross the bridge as camelCase → the snake_case params here),
//! and `mutate('…','approve_day_task_worklog',{day, task_id})` (a `body` struct
//! payload, whose serde field names stay snake_case).
//!
//! # Related
//! - `src/pm_worklog/generate.rs` — the CLI-side engine these spawn, and the source
//!   of the exact JSON shapes deserialized below.
//! - [`crate::commands::cli_exec`] — the shared spawn/[`crate::install::cli_cwd`]/
//!   timeout/`parse_last_line`/`run_meridian_json` helpers (this module's former
//!   private copies).
//! - [`crate::commands::statuses`] — sibling CLI-spawning command, same pattern
//!   through the same helpers.

use serde::{Deserialize, Serialize};
use std::time::Duration;

use super::cli_exec::run_meridian_json;

/// One ticket the update posts to. A draft carries 0..N of these — a strand of a
/// day's work often advances more than one planned task — and each tracks its own
/// delivery, because posting to three tickets can succeed on two.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorklogTarget {
    pub task_key: String,
    pub provider: String,
    pub confidence: f64,
    /// The user picked this ticket, overriding the model. `confidence` is then
    /// meaningless — the UI must not render it as a score.
    pub manual: bool,
    /// Hydrated from `pm_tasks` at read time — `None` on an older CLI that
    /// doesn't emit it yet.
    #[serde(default)]
    pub task_title: Option<String>,
    pub posted: bool,
    pub posted_comment_id: Option<String>,
    pub browse_url: Option<String>,
    /// A post was started and its outcome never recorded (a crash mid-request).
    /// The comment may or may not be live; only a human can tell. Never
    /// auto-retried. `false` on an older CLI that doesn't emit it.
    #[serde(default)]
    pub outcome_unknown: bool,
    /// Why this ticket failed, if it did. Its siblings may have succeeded.
    pub error: Option<String>,
}

/// The proposed-new-ticket branch (mutually exclusive with having any
/// [`WorklogTarget`]s).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneratedWorklogPropose {
    pub issue_type: String,
    pub title: String,
    pub description: String,
}

/// One labelled bullet group inside an update; the model names `heading` to fit
/// the work (dev "Decisions"/"Architecture", marketer "Campaigns", editor "Edits").
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorklogSection {
    pub heading: String,
    #[serde(default)]
    pub points: Vec<String>,
}

/// The status update — always present. `summary` + `status` are universal;
/// `sections` is a dynamic set of labelled bullet groups fitting the actual work.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneratedWorklogUpdate {
    pub summary: String,
    #[serde(default)]
    pub sections: Vec<WorklogSection>,
    #[serde(default)]
    pub status: String,
}

/// One generated-worklog draft — mirrors the CLI's JSON exactly.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DayTaskWorklogDraft {
    pub state: String,
    pub provider: String,
    #[serde(default)]
    pub targets: Vec<WorklogTarget>,
    pub propose: Option<GeneratedWorklogPropose>,
    pub update: GeneratedWorklogUpdate,
    pub reasoning: String,
    pub created_task_key: Option<String>,
    pub error: Option<String>,
}

/// POST body for [`approve_day_task_worklog`] — `mutate` wraps args in `{body}`.
/// Snake_case fields matching the UI payload `{day, task_id}` and the sibling
/// [`crate::commands::statuses::SetStatusBody`] convention (no case rewrite).
#[derive(Debug, Deserialize)]
pub struct ApproveBody {
    pub day: String,
    pub task_id: String,
}

/// One ticket's outcome in an [`ApproveResponse`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PostedTarget {
    pub task_key: String,
    pub posted: bool,
    pub browse_url: Option<String>,
    pub error: Option<String>,
}

/// [`approve_day_task_worklog`] response — mirrors the CLI's approve JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApproveResponse {
    /// Every ticket took the update. A partial success is `false` and retryable.
    pub posted: bool,
    #[serde(default)]
    pub targets: Vec<PostedTarget>,
    pub created_task_key: Option<String>,
    pub created: bool,
    pub error: Option<String>,
}

/// Generate (or regenerate) the draft for a day-task. Spawns
/// `meridian worklog-generate --day <d> --task-id <t>` (argv, no shell), 150 s
/// timeout (an LLM call), and parses the last JSON line of stdout.
#[tauri::command]
#[tracing::instrument]
pub async fn generate_day_task_worklog(
    day: String,
    task_id: String,
) -> Result<DayTaskWorklogDraft, String> {
    if day.is_empty() || task_id.is_empty() {
        return Err("day and task_id are required".to_string());
    }
    let draft: DayTaskWorklogDraft = run_meridian_json(
        &["worklog-generate", "--day", &day, "--task-id", &task_id],
        Duration::from_secs(150),
        "worklog-generate",
    )
    .await?;
    tracing::info!(%day, %task_id, state = %draft.state, provider = %draft.provider, "worklog-generate served");
    Ok(draft)
}

/// Read the existing draft for a day-task (or `None`). Spawns
/// `meridian worklog-generate-get --day <d> --task-id <t>`, 30 s timeout; the CLI
/// prints JSON `null` when there is no draft, which parses to `None`.
#[tauri::command]
#[tracing::instrument]
pub async fn get_day_task_worklog(
    day: String,
    task_id: String,
) -> Result<Option<DayTaskWorklogDraft>, String> {
    if day.is_empty() || task_id.is_empty() {
        return Err("day and task_id are required".to_string());
    }
    let draft: Option<DayTaskWorklogDraft> = run_meridian_json(
        &["worklog-generate-get", "--day", &day, "--task-id", &task_id],
        Duration::from_secs(30),
        "worklog-generate-get",
    )
    .await?;
    tracing::info!(%day, %task_id, present = draft.is_some(), "worklog-generate-get served");
    Ok(draft)
}

/// Approve the current draft: create-if-proposed → post the comment → link the
/// day-task. Spawns `meridian worklog-generate-approve --day <d> --task-id <t>`,
/// 150 s timeout (create + post round trips). Idempotent server-side.
#[tauri::command]
#[tracing::instrument(skip(body), fields(day = %body.day, task_id = %body.task_id))]
pub async fn approve_day_task_worklog(body: ApproveBody) -> Result<ApproveResponse, String> {
    if body.day.is_empty() || body.task_id.is_empty() {
        return Err("day and task_id are required".to_string());
    }
    let resp: ApproveResponse = run_meridian_json(
        &[
            "worklog-generate-approve",
            "--day",
            &body.day,
            "--task-id",
            &body.task_id,
        ],
        Duration::from_secs(150),
        "worklog-generate-approve",
    )
    .await?;
    tracing::info!(
        posted = resp.posted,
        created = resp.created,
        "worklog-generate-approve served"
    );
    Ok(resp)
}
