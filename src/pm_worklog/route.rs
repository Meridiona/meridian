//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Route: process ONE (task, hour) — collect the bundle, synthesise (the single
// LLM hop), ground it, and persist a DRAFTED worklog row. This stage never posts
// to Jira: every worklog waits in `drafted` for a human to review/edit/approve
// in the dashboard, after which the `post` sweep is the sole path to real Jira.
// Idempotent: the row UPSERT keyed on (task, day, cycle) replaces a still-DRAFTED
// row on a re-run but leaves an `approved`/`posted` row untouched (see `db.rs`).

use anyhow::Result;
use sqlx::SqlitePool;

use super::config::PmWorklogConfig;
use super::models::UpdateState;
use super::{collect, db, ground, synth};

/// One worklog covers exactly one hour, so time logged is capped here.
const WINDOW_SECONDS: i64 = 3600;

/// What happened for one task in one hour.
#[derive(Debug, Clone)]
pub struct TaskOutcome {
    pub task_key: String,
    pub state: UpdateState,
    pub reason: String,
    pub pm_worklog_id: Option<i64>,
}

/// Collect → synthesise → ground → draft one task's worklog for one hour window.
/// A synth failure leaves nothing persisted — the next driver pass retries this
/// hour/task. The drafted row is what the dashboard shows for approval.
pub async fn process_task(
    pool: &SqlitePool,
    cfg: &PmWorklogConfig,
    task_key: &str,
    hour_start_iso: &str,
    hour_end_iso: &str,
    day_utc: &str,
    cycle_index: i64,
) -> Result<TaskOutcome> {
    // 1. Collect.
    let bundle = collect::fetch_session_bundle(
        pool,
        task_key,
        hour_start_iso,
        hour_end_iso,
        cycle_index,
        day_utc,
    )
    .await?;

    if bundle.sessions.is_empty() {
        return Ok(TaskOutcome {
            task_key: task_key.to_string(),
            state: UpdateState::Skipped,
            reason: "no classified task sessions in window".to_string(),
            pm_worklog_id: None,
        });
    }

    // 2. Synthesise (gated LLM hop).
    let mut update = synth::synthesise(&bundle, cfg).await?;

    // Authoritative scalars from the bundle — never trust the LLM for these.
    update.task_key = bundle.task_key.clone();
    update.window_start = bundle.window_start.clone();
    update.window_end = bundle.window_end.clone();
    update.cycle_index = cycle_index;
    // time_spent = idle-discounted real_seconds, but capped at the window length:
    // overlapping coding-agent + screen sessions can sum past the hour, and you
    // cannot log more than one real hour in a one-hour window.
    update.time_spent_seconds = bundle.real_seconds.min(WINDOW_SECONDS);

    // 3. Ground.
    let grounded = ground::ground(update, &bundle, cfg.min_confidence);

    // 4. Decide state: an empty summary is unactionable, so skip; else draft.
    let state = if grounded.update.summary.trim().is_empty() {
        UpdateState::Skipped
    } else {
        UpdateState::Drafted
    };

    let (id_min, id_max) = bundle.session_id_bounds();
    let pm_worklog_id =
        db::upsert_pm_worklog(pool, &grounded, state, day_utc, id_min, id_max).await?;

    let reason = match state {
        UpdateState::Skipped => "skipped (empty summary after grounding)".to_string(),
        _ => format!(
            "drafted (conf={:.2}, coverage={:.2}, real_s={}) — awaiting UI approval",
            grounded.update.confidence, grounded.coverage, bundle.real_seconds
        ),
    };

    Ok(TaskOutcome {
        task_key: task_key.to_string(),
        state,
        reason,
        pm_worklog_id: Some(pm_worklog_id),
    })
}
