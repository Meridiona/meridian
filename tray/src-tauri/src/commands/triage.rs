//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Triage commands — the ported `/api/triage` GET + `decision`/`ignore` writes.
//!
//! These drive the board-cleanup page: read the cleanup working set, and record
//! the user's cleanup decisions back into `pm_task_curation`. Like every UI
//! action, nothing is pushed to the real tracker here — the writes record intent
//! in `meridian.db`; the daemon's apply-sweep is what later pushes a close out.
//! The third sub-route, `triage/apply` ([`apply_ticket_fix`]), applies a hygiene
//! fix to the REAL tracker — it shells out to `meridian ticket-update` (tracker
//! auth lives in the daemon), so it's a process spawn, not a DB write, but it
//! lives here with its triage siblings.
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
use std::time::Duration;
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

/// POST body for [`apply_ticket_fix`] (`{ provider, key, field, value? }`).
#[derive(Debug, Deserialize)]
pub struct ApplyBody {
    pub provider: String,
    pub key: String,
    pub field: String,
    /// The new field value (`@me` for assign-self); empty for value-less fixes.
    #[serde(default)]
    pub value: String,
}

/// The JSON `meridian ticket-update` prints (its last stdout line). `applied` =
/// the write landed + the local mirror re-synced; `redirected` = no provider API
/// for that field, so the dialog opens `browse_url` instead.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApplyOutput {
    pub status: String,
    pub provider: String,
    pub key: String,
    pub field: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub browse_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Ack for [`apply_ticket_fix`] — mirrors the route's `{ ok, result }`.
#[derive(Debug, Serialize)]
pub struct ApplyResponse {
    pub ok: bool,
    pub result: ApplyOutput,
}

/// Apply one board-hygiene fix to the real tracker (the ported /api/triage/apply
/// POST). Spawns `meridian ticket-update --provider <p> --key <k> --field <f>
/// --value <v>` (args via argv, not a shell string — no injection) with a 60 s
/// timeout (a write can re-sync the whole board), and relays the CLI's JSON
/// result. Errors (the route's 400/500) surface to the dialog as the message.
#[tauri::command]
#[tracing::instrument(skip(body), fields(provider = %body.provider, key = %body.key, field = %body.field))]
pub async fn apply_ticket_fix(body: ApplyBody) -> Result<ApplyResponse, String> {
    if body.provider.is_empty() || body.key.is_empty() || body.field.is_empty() {
        return Err("provider, key and field are required".to_string());
    }

    // meridian ticket-update reads provider credentials from ~/.meridian/.env via
    // dotenvy, which walks UP from the process CWD. In a packaged .app the tray's
    // CWD is inside the bundle — dotenvy never reaches ~/.meridian/.env from there,
    // so LINEAR_API_KEY / GITHUB_TOKEN / AZURE_DEVOPS_PAT are never loaded and the
    // subprocess bails "provider not configured". Anchoring to ~/.meridian fixes it.
    let home = std::env::var("HOME")
        .map_err(|_| "HOME env var not set — cannot locate ~/.meridian".to_string())?;
    let meridian_home = std::path::PathBuf::from(&home).join(".meridian");
    if !meridian_home.exists() {
        std::fs::create_dir_all(&meridian_home)
            .map_err(|e| format!("could not create ~/.meridian: {e}"))?;
    }

    let bin = crate::install::meridian_bin();
    let child = tokio::process::Command::new(&bin)
        .args([
            "ticket-update",
            "--provider",
            &body.provider,
            "--key",
            &body.key,
            "--field",
            &body.field,
            "--value",
            &body.value,
        ])
        .current_dir(&meridian_home)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output();

    let output = match tokio::time::timeout(Duration::from_secs(60), child).await {
        Err(_) => return Err("ticket-update timed out after 60s".to_string()),
        Ok(Err(e)) => {
            tracing::warn!(bin = %bin, error = %e, "ticket-update spawn failed");
            return Err(format!("spawn error: {e}"));
        }
        Ok(Ok(o)) => o,
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let msg = if stderr.is_empty() {
            format!("ticket-update exited {:?}", output.status.code())
        } else {
            stderr
        };
        tracing::warn!("ticket-update non-zero: {msg}");
        return Err(msg);
    }

    // The CLI logs the re-sync before the result JSON — take the last non-empty line.
    let stdout = String::from_utf8_lossy(&output.stdout);
    let last = stdout.lines().rfind(|l| !l.trim().is_empty());
    match last.and_then(|l| serde_json::from_str::<ApplyOutput>(l).ok()) {
        Some(result) => {
            tracing::info!(status = %result.status, "ticket-update applied");
            Ok(ApplyResponse { ok: true, result })
        }
        None => {
            let s = stdout.trim();
            let skip = s.chars().count().saturating_sub(200);
            let tail: String = s.chars().skip(skip).collect();
            Err(format!("could not parse result: {tail}"))
        }
    }
}
