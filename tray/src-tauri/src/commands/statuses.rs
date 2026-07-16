//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Ticket status list + set — the dashboard's status control, ported to Rust.
//!
//! # What this is
//! Two commands backing the "change a ticket's status" UI:
//! - [`list_task_statuses`] — the statuses a ticket can move to (each normalised
//!   to the canonical `backlog|todo|in_progress|in_review|done|cancelled|unknown`
//!   taxonomy) + the ticket's current status.
//! - [`set_task_status`] — move a ticket to a chosen status. The status may be an
//!   id OR a status NAME (case-insensitive), which is what lets the UI's Undo
//!   pass the previous status name (the only stable handle on Jira).
//!
//! Tracker auth lives in the daemon (`~/.meridian/.env`), so — exactly like
//! [`crate::commands::apply_ticket_fix`] and [`crate::commands::get_ticket_parents`]
//! — these **shell out to the `meridian` CLI** (`ticket-statuses` /
//! `ticket-set-status`) rather than talking to any tracker in-process. They are
//! NOT DB reads, so they live tray-side, not in meridian-core.
//!
//! # Who calls this
//! Registered in `lib.rs`'s `invoke_handler!`; consumed by the dashboard's status
//! control via `ui/lib/bridge.ts` — `load('…','list_task_statuses',{provider,key})`
//! for the read (flat named args) and `mutate('…','set_task_status',{…})` for the
//! write (a `body` payload).
//!
//! # Related
//! - `src/intelligence/ticket_update/statuses.rs` — the CLI-side dispatch these
//!   spawn, and the source of the exact JSON shapes parsed below.
//! - [`crate::commands::apply_ticket_fix`] — sibling hygiene write-back, same
//!   spawn/`current_dir(~/.meridian)`/timeout pattern.
//! - [`crate::install::meridian_bin`] — the "native binary first" resolver.

use serde::{Deserialize, Serialize};
use std::time::Duration;

/// One status option — mirrors the CLI's `{id,name,category}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusOptionDto {
    pub id: String,
    pub name: String,
    /// Canonical `backlog|todo|in_progress|in_review|done|cancelled|unknown`.
    pub category: String,
}

/// `list_task_statuses` response — mirrors `meridian ticket-statuses`' JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusListResponse {
    pub statuses: Vec<StatusOptionDto>,
    pub current_id: Option<String>,
    pub current_name: Option<String>,
}

/// The `result` block of a set — `status` is `applied` or `redirected`;
/// `browse_url`/`reason` are populated only on a redirect.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetStatusOutcome {
    pub status: String,
    pub browse_url: Option<String>,
    pub reason: Option<String>,
}

/// `set_task_status` response — mirrors `meridian ticket-set-status`' JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetStatusResponse {
    pub result: SetStatusOutcome,
    /// The status the ticket now holds; `None` on a redirect (nothing changed).
    pub new_status: Option<StatusOptionDto>,
}

/// POST body for [`set_task_status`] (`{ provider, key, status_id }`). `status_id`
/// may be a status id OR a status name — it's passed through as `--status`.
#[derive(Debug, Deserialize)]
pub struct SetStatusBody {
    pub provider: String,
    pub key: String,
    pub status_id: String,
}

/// Resolve `~/.meridian` (created if missing). The CLI reads tracker creds from
/// `~/.meridian/.env` via dotenvy, which walks UP from the process CWD — so the
/// subprocess must run with its CWD there or auth is never loaded (see
/// [`crate::commands::apply_ticket_fix`] for the full rationale).
fn meridian_home() -> Result<std::path::PathBuf, String> {
    let home = std::env::var("HOME")
        .map_err(|_| "HOME env var not set — cannot locate ~/.meridian".to_string())?;
    let dir = std::path::PathBuf::from(&home).join(".meridian");
    if !dir.exists() {
        std::fs::create_dir_all(&dir).map_err(|e| format!("could not create ~/.meridian: {e}"))?;
    }
    Ok(dir)
}

/// Run `meridian <args…>` in `~/.meridian` with stdin nulled and stdout/stderr
/// piped, under `timeout`. Returns the trimmed stdout on success, or the trimmed
/// stderr (or a status message) as `Err` on non-zero exit / timeout / spawn error.
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
/// result line). Returns a bounded parse-error message on failure.
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

/// List the statuses `key` can move to (+ its current status) on `provider`.
/// Spawns `meridian ticket-statuses --provider <p> --key <k>` (argv, no shell —
/// no injection), 30 s timeout, and parses the last JSON line of stdout.
#[tauri::command]
#[tracing::instrument]
pub async fn list_task_statuses(
    provider: String,
    key: String,
) -> Result<StatusListResponse, String> {
    if provider.is_empty() || key.is_empty() {
        return Err("provider and key are required".to_string());
    }
    let stdout = run_meridian(
        &["ticket-statuses", "--provider", &provider, "--key", &key],
        Duration::from_secs(30),
        "ticket-statuses",
    )
    .await?;
    let resp: StatusListResponse = parse_last_line(&stdout)?;
    tracing::info!(%provider, %key, statuses = resp.statuses.len(), "ticket-statuses served");
    Ok(resp)
}

/// Move `key` to `status_id` (an id OR a status name) on `provider`. Spawns
/// `meridian ticket-set-status --provider <p> --key <k> --status <s>` (argv, no
/// shell), 60 s timeout (an applied move re-syncs the board), and parses the last
/// JSON line of stdout.
#[tauri::command]
#[tracing::instrument(skip(body), fields(provider = %body.provider, key = %body.key, status_id = %body.status_id))]
pub async fn set_task_status(body: SetStatusBody) -> Result<SetStatusResponse, String> {
    if body.provider.is_empty() || body.key.is_empty() || body.status_id.is_empty() {
        return Err("provider, key and status_id are required".to_string());
    }
    let stdout = run_meridian(
        &[
            "ticket-set-status",
            "--provider",
            &body.provider,
            "--key",
            &body.key,
            "--status",
            &body.status_id,
        ],
        Duration::from_secs(60),
        "ticket-set-status",
    )
    .await?;
    let resp: SetStatusResponse = parse_last_line(&stdout)?;
    tracing::info!(status = %resp.result.status, "ticket-set-status applied");
    Ok(resp)
}
