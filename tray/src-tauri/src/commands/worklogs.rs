//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Worklog commands — the ported `/api/worklogs` read + `[id]` write routes.
//!
//! The worklog review surface: read a day's draft worklogs, and record the
//! human-in-the-loop decisions (edit the comment, approve / reject / unapprove).
//! Nothing posts to Jira here — the writes record intent in `meridian.db` and the
//! daemon's ~60s approved-sweep is what posts. A `posted` worklog is immutable.
//!
//! Each write takes ONE `body` payload object (carrying the row `id`) so the Tauri
//! and browser paths send one identical shape; request-scoped `now` is resolved
//! here so the [`meridian_core::worklogs`] write fns stay deterministic.
//!
//! # Who calls this
//! Registered in `lib.rs`'s `invoke_handler!`; consumed by
//! `ui/components/views/WorklogsView.tsx` (read via `load`, writes via `mutate`).
//!
//! # Related
//! - [`meridian_core::worklogs`] — the byte-for-byte route ports these delegate to.

use serde::{Deserialize, Serialize};
use tauri::State;

/// Seconds-precision UTC RFC3339 — matches the route's `nowIso()`.
fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// A day's worklogs for review, computed in Rust (the ported /api/worklogs GET).
/// `day` defaults to today (local) when omitted, matching the route.
#[tauri::command]
#[tracing::instrument(skip(pool))]
pub async fn get_worklogs(
    pool: State<'_, Option<meridian_core::SqlitePool>>,
    day: Option<String>,
) -> Result<meridian_core::worklogs::WorklogsResponse, String> {
    let Some(pool) = pool.inner() else {
        return Err("meridian.db is not open yet".to_string());
    };
    let day = day.unwrap_or_else(meridian_core::date::today_string);
    meridian_core::worklogs::get_worklogs(pool, &day)
        .await
        .map_err(|e| {
            tracing::warn!(error = %e, "get_worklogs failed");
            e.to_string()
        })
}

/// Ack for the worklog writes — mirrors the routes' `{ ok, id, state }`.
#[derive(Debug, Serialize)]
pub struct WorklogWriteAck {
    pub ok: bool,
    pub id: i64,
    pub state: String,
}

/// PATCH body for [`edit_worklog`] (`{ id, summary }`).
#[derive(Debug, Deserialize)]
pub struct WorklogEditBody {
    pub id: i64,
    pub summary: String,
}

/// Edit a worklog's Jira comment (the ported /api/worklogs/[id] PATCH).
#[tauri::command]
#[tracing::instrument(skip(pool, body), fields(id = body.id))]
pub async fn edit_worklog(
    pool: State<'_, Option<meridian_core::SqlitePool>>,
    body: WorklogEditBody,
) -> Result<WorklogWriteAck, String> {
    let Some(pool) = pool.inner() else {
        return Err("meridian.db is not open yet".to_string());
    };
    let state = meridian_core::worklogs::edit_worklog(pool, body.id, &body.summary, &now_iso())
        .await
        .map_err(|e| {
            tracing::warn!(error = %e, id = body.id, "edit_worklog failed");
            e.to_string()
        })?;
    Ok(WorklogWriteAck {
        ok: true,
        id: body.id,
        state,
    })
}

/// POST body for [`worklog_action`] (`{ id, action, correctedTaskKey?,
/// correctedToUntracked? }`). camelCase to match the route's JSON body.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorklogActionBody {
    pub id: i64,
    pub action: String,
    #[serde(default)]
    pub corrected_task_key: Option<String>,
    #[serde(default)]
    pub corrected_to_untracked: Option<bool>,
}

/// Approve / reject / unapprove a worklog (the ported /api/worklogs/[id] POST).
/// The reject-only attribution correction (where the time should have gone) is
/// gated here, mirroring the route (ignored for approve/unapprove).
#[tauri::command]
#[tracing::instrument(skip(pool, body), fields(id = body.id, action = %body.action))]
pub async fn worklog_action(
    pool: State<'_, Option<meridian_core::SqlitePool>>,
    body: WorklogActionBody,
) -> Result<WorklogWriteAck, String> {
    let Some(pool) = pool.inner() else {
        return Err("meridian.db is not open yet".to_string());
    };
    let action = meridian_core::worklogs::WorklogAction::parse(&body.action)
        .ok_or("action must be approve|reject|unapprove")?;

    // Attribution correction applies to `reject` only (matches the route).
    let is_reject = matches!(action, meridian_core::worklogs::WorklogAction::Reject);
    let corrected_task_key = if is_reject {
        body.corrected_task_key
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
    } else {
        None
    };
    let corrected_to_untracked = is_reject && body.corrected_to_untracked.unwrap_or(false);

    let state = meridian_core::worklogs::worklog_action(
        pool,
        body.id,
        action,
        corrected_task_key,
        corrected_to_untracked,
        &now_iso(),
    )
    .await
    .map_err(|e| {
        tracing::warn!(error = %e, id = body.id, "worklog_action failed");
        e.to_string()
    })?;
    Ok(WorklogWriteAck {
        ok: true,
        id: body.id,
        state,
    })
}
