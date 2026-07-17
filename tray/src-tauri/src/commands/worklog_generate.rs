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
//! - [`crate::commands::statuses`] — sibling CLI-spawning command, same
//!   spawn/`current_dir(~/.meridian)`/timeout/`run_meridian_json` pattern.

use serde::{Deserialize, Serialize};
use std::time::Duration;

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
///
/// Shared with [`crate::commands::plan_tasks`] — the CLI-spawning contract (argv, no
/// shell; `current_dir(~/.meridian)` so dotenvy finds the daemon's `.env`; a bounded
/// error) is identical for every command that shells out, so it lives here once.
pub(crate) async fn run_meridian(
    args: &[&str],
    timeout: Duration,
    label: &str,
) -> Result<String, String> {
    let home = meridian_home()?;
    let bin = crate::install::meridian_bin();
    // WHICH binary ran is the single most useful fact when one of these misbehaves:
    // the tray calls the INSTALLED meridian, which can be older than the tray asking
    // it for a subcommand. Log it before we spawn, so a failure downstream is one
    // trace lookup rather than a guess.
    tracing::debug!(bin = %bin, cwd = %home.display(), args = ?args, "{label}: spawning");
    let child = tokio::process::Command::new(&bin)
        .args(args)
        .current_dir(&home)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output();

    let output = match tokio::time::timeout(timeout, child).await {
        Err(_) => {
            tracing::warn!(bin = %bin, timeout_s = timeout.as_secs(), "{label}: timed out");
            return Err(format!("{label} timed out"));
        }
        Ok(Err(e)) => {
            tracing::warn!(bin = %bin, error = %e, "{label} spawn failed");
            return Err(format!("spawn error: {e}"));
        }
        Ok(Ok(o)) => o,
    };

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    tracing::debug!(
        bin = %bin,
        code = ?output.status.code(),
        stdout_len = stdout.len(),
        stderr_tail = %tail(&stderr, 400),
        "{label}: finished"
    );

    if !output.status.success() {
        let msg = if stderr.is_empty() {
            format!("{label} exited {:?}", output.status.code())
        } else {
            stderr
        };
        tracing::warn!(bin = %bin, code = ?output.status.code(), "{label} non-zero: {msg}");
        return Err(msg);
    }
    Ok(stdout)
}

/// Last `n` chars of `s` — bounded so a runaway log line can't fill a span.
fn tail(s: &str, n: usize) -> String {
    let s = s.trim();
    s.chars()
        .skip(s.chars().count().saturating_sub(n))
        .collect()
}

/// Run `meridian <args…>` and parse its last stdout line as JSON `T`.
///
/// The ONE way a command should shell out for a JSON result — [`run_meridian`] +
/// [`parse_last_line`] were being paired by hand at every call site, and the pairing is
/// where the diagnostics went missing.
///
/// # Why the error names the binary
/// The tray spawns the INSTALLED `meridian`, which is versioned independently of the
/// tray. Ask a stale one for a subcommand it doesn't have and (before the guard in
/// `main.rs`) it ignored the argv and booted a daemon, which the single-instance guard
/// killed with a warning on stdout — surfacing to the user as "could not parse result:
/// …daemon.sock", a message about a socket, naming neither the binary nor the
/// subcommand. So a parse failure here reports WHICH binary produced the unparseable
/// output, and logs the whole of it at `error` for the trace.
pub(crate) async fn run_meridian_json<T: for<'de> Deserialize<'de>>(
    args: &[&str],
    timeout: Duration,
    label: &str,
) -> Result<T, String> {
    let stdout = run_meridian(args, timeout, label).await?;
    match parse_last_line::<T>(&stdout) {
        Ok(v) => Ok(v),
        Err(_) => {
            let bin = crate::install::meridian_bin();
            tracing::error!(
                bin = %bin,
                args = ?args,
                stdout = %tail(&stdout, 2000),
                "{label}: output was not JSON - is this binary older than the tray?"
            );
            Err(format!(
                "{label}: {bin} returned no result. It may be older than this app - reinstall or rebuild it."
            ))
        }
    }
}

/// Parse the LAST non-empty stdout line as JSON `T` (the CLI logs before the
/// result line). Shared with [`crate::commands::plan_tasks`] — same contract.
pub(crate) fn parse_last_line<T: for<'de> Deserialize<'de>>(stdout: &str) -> Result<T, String> {
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
