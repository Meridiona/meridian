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
//! — a TRACKER-owned ticket **shells out to the `meridian` CLI** (`ticket-statuses` /
//! `ticket-set-status`) rather than talking to any tracker in-process.
//!
//! A **personal** (`provider = 'local'`) task is different: there is no tracker to
//! authenticate against, just our own `pm_tasks` row, so both commands short-circuit
//! straight to [`meridian_core::task_create`]'s local-status helpers instead of
//! shelling out — a personal task's fixed To Do/In Progress/Done lifecycle
//! ([`meridian_core::task_create::LOCAL_STATUSES`]) is a plain DB read/write, not a
//! process worth spawning for.
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
use tauri::State;

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

/// One [`meridian_core::task_create::LocalStatusOption`] shaped as a [`StatusOptionDto`].
fn local_status_dto(o: &meridian_core::task_create::LocalStatusOption) -> StatusOptionDto {
    StatusOptionDto {
        id: o.id.to_string(),
        name: o.name.to_string(),
        category: o.category.to_string(),
    }
}

/// List the statuses `key` can move to (+ its current status) on `provider`. A
/// personal task reads its fixed local lifecycle straight from `pm_tasks`
/// (see the module header); a tracker-owned ticket spawns
/// `meridian ticket-statuses --provider <p> --key <k>` (argv, no shell — no
/// injection), 30 s timeout, and parses the last JSON line of stdout.
#[tauri::command]
#[tracing::instrument(skip(pool))]
pub async fn list_task_statuses(
    pool: State<'_, crate::db_pool::DbPool>,
    provider: String,
    key: String,
) -> Result<StatusListResponse, String> {
    if provider.is_empty() || key.is_empty() {
        return Err("provider and key are required".to_string());
    }
    if provider == meridian_core::task_create::LOCAL_PROVIDER {
        let Some(pool) = pool.get() else {
            return Err("meridian.db is not open yet".to_string());
        };
        let current = meridian_core::task_create::local_task_current(&pool, &key)
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("no personal task {key}"))?;
        let (current_name, _) = current;
        let current_id = meridian_core::task_create::resolve_local_status(&current_name)
            .map(|o| o.id.to_string());
        let resp = StatusListResponse {
            statuses: meridian_core::task_create::LOCAL_STATUSES
                .iter()
                .map(local_status_dto)
                .collect(),
            current_id,
            current_name: Some(current_name),
        };
        tracing::info!(%key, "list_task_statuses served (personal task)");
        return Ok(resp);
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

/// Move `key` to `status_id` (an id OR a status name) on `provider`. A personal task
/// writes straight to `pm_tasks` (see the module header); a tracker-owned ticket
/// spawns `meridian ticket-set-status --provider <p> --key <k> --status <s>` (argv, no
/// shell), 60 s timeout (an applied move re-syncs the board), and parses the last
/// JSON line of stdout.
#[tauri::command]
#[tracing::instrument(skip(body, pool), fields(provider = %body.provider, key = %body.key, status_id = %body.status_id))]
pub async fn set_task_status(
    pool: State<'_, crate::db_pool::DbPool>,
    body: SetStatusBody,
) -> Result<SetStatusResponse, String> {
    if body.provider.is_empty() || body.key.is_empty() || body.status_id.is_empty() {
        return Err("provider, key and status_id are required".to_string());
    }
    if body.provider == meridian_core::task_create::LOCAL_PROVIDER {
        let Some(pool) = pool.get() else {
            return Err("meridian.db is not open yet".to_string());
        };
        let Some(opt) = meridian_core::task_create::resolve_local_status(&body.status_id) else {
            return Err(format!("unknown status \"{}\"", body.status_id));
        };
        let now = chrono::Utc::now().to_rfc3339();
        let is_terminal = opt.category == "done";
        let changed = meridian_core::task_create::set_local_status(
            &pool,
            &body.key,
            opt.name,
            is_terminal,
            &now,
        )
        .await
        .map_err(|e| e.to_string())?;
        if !changed {
            return Err(format!("could not update {}", body.key));
        }
        tracing::info!(status = opt.name, "set_task_status applied (personal task)");
        return Ok(SetStatusResponse {
            result: SetStatusOutcome {
                status: "applied".to_string(),
                browse_url: None,
                reason: None,
            },
            new_status: Some(local_status_dto(opt)),
        });
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
