//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! The day-task "Generate worklog" commands, ported to Rust (tray side).
//!
//! # What this is
//! Three commands backing the "Generate worklog" action on a day-task workstream
//! card:
//! - [`generate_day_task_worklog`] — run (or regenerate) the AI draft: match the
//!   day-task to a ticket (or propose one) + a status update.
//! - [`get_day_task_worklog`] — read an existing draft on panel reopen (or `None`).
//! - [`approve_day_task_worklog`] — approve → create-if-proposed → post the comment
//!   → link the day-task.
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
//! {day, task_id})` (flat named args), `invoke('generate_day_task_worklog',{day,
//! task_id})`, and `mutate('…','approve_day_task_worklog',{day,task_id})` (a
//! `body` payload).
//!
//! # Related
//! - `src/pm_worklog/generate.rs` — the CLI-side engine these spawn, and the source
//!   of the exact JSON shapes deserialized below.
//! - [`crate::commands::statuses`] — sibling CLI-spawning command, same
//!   spawn/`current_dir(~/.meridian)`/timeout/`parse_last_line` pattern.

use serde::{Deserialize, Serialize};
use std::time::Duration;

/// The matched-ticket branch (mutually exclusive with [`GeneratedWorklogPropose`]).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneratedWorklogMatch {
    pub task_key: String,
    pub confidence: f64,
}

/// The proposed-new-ticket branch (mutually exclusive with [`GeneratedWorklogMatch`]).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneratedWorklogPropose {
    pub issue_type: String,
    pub title: String,
    pub description: String,
}

/// The status update — always present.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneratedWorklogUpdate {
    pub summary: String,
    #[serde(default)]
    pub decisions: Vec<String>,
    #[serde(default)]
    pub architecture: Vec<String>,
    #[serde(default)]
    pub status: String,
}

/// One generated-worklog draft — mirrors the CLI's JSON exactly.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DayTaskWorklogDraft {
    pub state: String,
    pub provider: String,
    #[serde(rename = "match")]
    pub match_: Option<GeneratedWorklogMatch>,
    pub propose: Option<GeneratedWorklogPropose>,
    pub update: GeneratedWorklogUpdate,
    pub reasoning: String,
    pub target_key: Option<String>,
    pub created_task_key: Option<String>,
    pub posted_comment_id: Option<String>,
    pub browse_url: Option<String>,
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

/// [`approve_day_task_worklog`] response — mirrors the CLI's approve JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApproveResponse {
    pub posted: bool,
    pub target_key: Option<String>,
    pub created_task_key: Option<String>,
    pub created: bool,
    pub browse_url: Option<String>,
    pub error: Option<String>,
}

/// Resolve `~/.meridian` (created if missing) — the CWD the CLI must run in so
/// dotenvy loads `~/.meridian/.env` (see [`crate::commands::statuses`]).
fn meridian_home() -> Result<std::path::PathBuf, String> {
    let home = std::env::var("HOME")
        .map_err(|_| "HOME env var not set — cannot locate ~/.meridian".to_string())?;
    let dir = std::path::PathBuf::from(&home).join(".meridian");
    if !dir.exists() {
        std::fs::create_dir_all(&dir).map_err(|e| format!("could not create ~/.meridian: {e}"))?;
    }
    Ok(dir)
}

/// Run `meridian <args…>` in `~/.meridian` under `timeout`, returning trimmed
/// stdout on success or a bounded error message. Same pattern as
/// [`crate::commands::statuses`].
async fn run_meridian(args: &[&str], timeout: Duration, label: &str) -> Result<String, String> {
    let home = meridian_home()?;
    let bin = crate::install::meridian_bin();
    let child = tokio::process::Command::new(&bin)
        .args(args)
        .current_dir(&home)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output();

    let output = match tokio::time::timeout(timeout, child).await {
        Err(_) => return Err(format!("{label} timed out")),
        Ok(Err(e)) => {
            tracing::warn!(bin = %bin, error = %e, "{label} spawn failed");
            return Err(format!("spawn error: {e}"));
        }
        Ok(Ok(o)) => o,
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let msg = if stderr.is_empty() {
            format!("{label} exited {:?}", output.status.code())
        } else {
            stderr
        };
        tracing::warn!("{label} non-zero: {msg}");
        return Err(msg);
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Parse the LAST non-empty stdout line as JSON `T` (the CLI logs before the
/// result line).
fn parse_last_line<T: for<'de> Deserialize<'de>>(stdout: &str) -> Result<T, String> {
    let last = stdout.lines().rfind(|l| !l.trim().is_empty());
    match last.and_then(|l| serde_json::from_str::<T>(l).ok()) {
        Some(v) => Ok(v),
        None => {
            let s = stdout.trim();
            let skip = s.chars().count().saturating_sub(200);
            let tail: String = s.chars().skip(skip).collect();
            Err(format!("could not parse result: {tail}"))
        }
    }
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
    let stdout = run_meridian(
        &["worklog-generate", "--day", &day, "--task-id", &task_id],
        Duration::from_secs(150),
        "worklog-generate",
    )
    .await?;
    let draft: DayTaskWorklogDraft = parse_last_line(&stdout)?;
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
    let stdout = run_meridian(
        &["worklog-generate-get", "--day", &day, "--task-id", &task_id],
        Duration::from_secs(30),
        "worklog-generate-get",
    )
    .await?;
    let draft: Option<DayTaskWorklogDraft> = parse_last_line(&stdout)?;
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
    let stdout = run_meridian(
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
    let resp: ApproveResponse = parse_last_line(&stdout)?;
    tracing::info!(
        posted = resp.posted,
        created = resp.created,
        "worklog-generate-approve served"
    );
    Ok(resp)
}
