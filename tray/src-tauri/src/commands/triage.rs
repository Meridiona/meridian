//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Triage commands — the ported `/api/triage` GET + `decision`/`ignore` writes.
//!
//! These drive the board-cleanup page: read the cleanup working set, and record
//! the user's cleanup decisions back into `pm_task_curation`. Like every UI
//! action, nothing is pushed to the real tracker here — the writes record intent
//! in `meridian.db`; the daemon's apply-sweep is what later pushes a close out.
//! (The third sub-route, `triage/apply`, shells out to `meridian ticket-update`
//! and so stays a process command, not a DB write — it is not here.)
//!
//! Each write resolves request-scoped time (`now`, the snooze expiry) here so the
//! [`meridian_core::triage`] write fns stay deterministic + unit-testable, and
//! takes ONE `body` payload object so the Tauri (camelCase→snake_case) and browser
//! (`JSON.stringify`) paths send one identical snake_case shape.
//!
//! # Who calls this
//! Registered in `lib.rs`'s `invoke_handler!`; consumed by
//! `ui/components/views/CleanupView.tsx` via `ui/lib/bridge.ts` (`mutate` for the
//! writes; the GET has no consumer yet — the page reads hygiene via `get_tasks`).
//!
//! # Related
//! - [`meridian_core::triage`] — the byte-for-byte route ports these delegate to
//!   ([`meridian_core::triage::record_decision`] / [`meridian_core::triage::set_ignored`]).
//! - [`crate::commands::dashboard`] — sibling DB-read commands.

use serde::{Deserialize, Serialize};
use tauri::State;

/// Seconds-precision UTC RFC3339 (`2026-06-18T10:00:00Z`) — matches the route's
/// `nowIso()` (which strips fractional seconds).
fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// The cleanup working set (the ported /api/triage GET). Resolves `now` here (so
/// the core fn stays deterministic) to hide future-snoozed tickets. No dashboard
/// consumer today — ported for parity with the daemon's cleanup engine; see
/// [`meridian_core::triage`].
#[tauri::command]
#[tracing::instrument(skip(pool))]
pub async fn get_triage(
    pool: State<'_, Option<meridian_core::SqlitePool>>,
) -> Result<meridian_core::triage::TriageResponse, String> {
    let Some(pool) = pool.inner() else {
        return Err("meridian.db is not open yet".to_string());
    };
    // The triage GET route compares snoozed_until against `new Date().toISOString()`
    // (MILLIS precision) — keep millis here, not the writes' seconds-precision now.
    let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    meridian_core::triage::get_triage(pool, &now)
        .await
        .map_err(|e| {
            tracing::warn!(error = %e, "get_triage failed");
            e.to_string()
        })
}

/// POST body for [`triage_decision`] (`{ task_key, decision, snooze_days? }`).
#[derive(Debug, Deserialize)]
pub struct TriageDecisionBody {
    pub task_key: String,
    pub decision: String,
    #[serde(default)]
    pub snooze_days: Option<i64>,
}

/// Ack for [`triage_decision`] — mirrors the route's JSON response.
#[derive(Debug, Serialize)]
pub struct TriageDecisionAck {
    pub ok: bool,
    pub task_key: String,
    pub decision: String,
    pub snoozed_until: Option<String>,
}

/// Record a board-cleanup decision (the ported /api/triage/decision POST). The
/// snooze expiry is resolved here: `now + max(1, snooze_days|7)` days, only for a
/// `snoozed` decision. NOTE: the route uses JS `setDate` (calendar-day add in
/// local time); `Utc::now() + days` differs only across a DST transition inside
/// the snooze window — accepted, as a snooze is inherently approximate.
#[tauri::command]
#[tracing::instrument(skip(pool, body), fields(task_key = %body.task_key, decision = %body.decision))]
pub async fn triage_decision(
    pool: State<'_, Option<meridian_core::SqlitePool>>,
    body: TriageDecisionBody,
) -> Result<TriageDecisionAck, String> {
    let Some(pool) = pool.inner() else {
        return Err("meridian.db is not open yet".to_string());
    };
    if body.task_key.is_empty() {
        return Err("task_key required".to_string());
    }
    let decision = meridian_core::triage::Decision::parse(&body.decision)
        .ok_or("decision must be keep|excluded|snoozed")?;

    // max(1, snooze_days), default 7 (the route's `max(1, floor(days)) : 7`).
    let snooze_days = body.snooze_days.map(|d| d.max(1)).unwrap_or(7);
    let snoozed_until = if matches!(decision, meridian_core::triage::Decision::Snoozed) {
        Some(
            (chrono::Utc::now() + chrono::Duration::days(snooze_days))
                .to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        )
    } else {
        None
    };

    meridian_core::triage::record_decision(
        pool,
        &body.task_key,
        decision,
        snoozed_until.as_deref(),
        &now_iso(),
    )
    .await
    .map_err(|e| {
        tracing::warn!(error = %e, "triage_decision failed");
        e.to_string()
    })?;

    Ok(TriageDecisionAck {
        ok: true,
        task_key: body.task_key,
        decision: decision.as_str().to_string(),
        snoozed_until,
    })
}

/// POST body for [`triage_ignore`] (`{ task_key, code, undo? }`).
#[derive(Debug, Deserialize)]
pub struct TriageIgnoreBody {
    pub task_key: String,
    pub code: String,
    #[serde(default)]
    pub undo: Option<bool>,
}

/// Ack for [`triage_ignore`] — mirrors the route's JSON response.
#[derive(Debug, Serialize)]
pub struct TriageIgnoreAck {
    pub ok: bool,
    pub task_key: String,
    pub ignored: Vec<String>,
}

/// Toggle an optional hygiene defect's ignored flag (the ported /api/triage/ignore
/// POST). Must-fix codes are rejected by the core fn; `undo` removes the code.
#[tauri::command]
#[tracing::instrument(skip(pool, body), fields(task_key = %body.task_key, code = %body.code))]
pub async fn triage_ignore(
    pool: State<'_, Option<meridian_core::SqlitePool>>,
    body: TriageIgnoreBody,
) -> Result<TriageIgnoreAck, String> {
    let Some(pool) = pool.inner() else {
        return Err("meridian.db is not open yet".to_string());
    };
    if body.task_key.is_empty() {
        return Err("task_key required".to_string());
    }
    if body.code.is_empty() {
        return Err("code required".to_string());
    }
    let ignored = meridian_core::triage::set_ignored(
        pool,
        &body.task_key,
        &body.code,
        body.undo.unwrap_or(false),
    )
    .await
    .map_err(|e| {
        tracing::warn!(error = %e, "triage_ignore failed");
        e.to_string()
    })?;

    Ok(TriageIgnoreAck {
        ok: true,
        task_key: body.task_key,
        ignored,
    })
}
