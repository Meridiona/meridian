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
//! - [`crate::commands::cli_exec`] — the shared spawn/`current_dir(~/.meridian)`/
//!   timeout/`parse_last_line` helpers (this module's former private copies).
//! - [`crate::install::meridian_bin`] — the "native binary first" resolver.

use serde::{Deserialize, Serialize};
use std::time::Duration;

// The CLI-spawning contract (argv/no shell, `current_dir(~/.meridian)` so dotenvy
// finds the daemon's `.env`, a timeout, last-line JSON, and a diagnostic that names
// the binary) lives in ONE place. This module used to carry its own copy of
// meridian_home/run_meridian/parse_last_line; they drifted, and the drift is what
// made a stale-binary failure read as an unparseable socket warning.
use super::cli_exec::run_meridian_json;

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
    let resp: StatusListResponse = run_meridian_json(
        &["ticket-statuses", "--provider", &provider, "--key", &key],
        Duration::from_secs(30),
        "ticket-statuses",
    )
    .await?;
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
    let resp: SetStatusResponse = run_meridian_json(
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
    tracing::info!(status = %resp.result.status, "ticket-set-status applied");
    Ok(resp)
}
