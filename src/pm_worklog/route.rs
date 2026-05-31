// meridian — normalises screenpipe activity into structured app sessions
//
// Route: process ONE (task, hour) — collect the bundle, synthesise (the single
// LLM hop), ground it, persist the worklog row, and (unless dry-run) post it to
// Jira. Idempotent: a window already posted short-circuits, and the row UPSERT
// keyed on (task, day, cycle) means a re-run replaces a DRAFTED row but never a
// POSTED one. Port of the Collect→Synth→Ground→Route spine in
// `pm_worklog_update/workflow.py`, minus the LLM (which lives in the endpoint).

use anyhow::Result;
use sqlx::SqlitePool;

use crate::config::JiraConfig;

use super::config::PmWorklogConfig;
use super::models::UpdateState;
use super::{collect, db, ground, jira, synth};

/// One worklog covers exactly one hour, so time logged is capped here.
const WINDOW_SECONDS: i64 = 3600;

/// What happened for one task in one hour.
#[derive(Debug, Clone)]
pub struct TaskOutcome {
    pub task_key: String,
    pub state: UpdateState,
    pub reason: String,
    pub pm_worklog_id: Option<i64>,
    pub posted_worklog_id: Option<String>,
}

/// Process one task's worklog for one hour window. `jira` is `None` when no Jira
/// provider is configured (then we only ever DRAFT). `dry_run` persists the
/// drafted row but never posts.
#[allow(clippy::too_many_arguments)]
pub async fn process_task(
    pool: &SqlitePool,
    jira: Option<&JiraConfig>,
    cfg: &PmWorklogConfig,
    task_key: &str,
    hour_start_iso: &str,
    hour_end_iso: &str,
    day_utc: &str,
    cycle_index: i64,
    dry_run: bool,
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
            posted_worklog_id: None,
        });
    }

    // 2. Synthesise (gated LLM hop). A synth failure leaves nothing persisted —
    // the next driver pass retries this hour/task.
    let mut update = synth::synthesise(&bundle, cfg).await?;

    // Authoritative scalars from the bundle — never trust the LLM for these.
    update.task_key = bundle.task_key.clone();
    update.window_start = bundle.window_start.clone();
    update.window_end = bundle.window_end.clone();
    update.cycle_index = cycle_index;
    // time_spent = idle-discounted real_seconds, but capped at the window length:
    // overlapping coding-agent + screen sessions can sum past the hour, and you
    // cannot log more than one real hour in a one-hour window (the Python
    // time_spent_sanity_check, applied authoritatively here).
    update.time_spent_seconds = bundle.real_seconds.min(WINDOW_SECONDS);

    // 3. Ground.
    let grounded = ground::ground(update, &bundle, cfg.min_confidence);

    // 4. Decide initial state.
    let mut state = if grounded.update.summary.trim().is_empty() {
        UpdateState::Skipped
    } else {
        UpdateState::Drafted
    };

    let (id_min, id_max) = bundle.session_id_bounds();
    let pm_worklog_id =
        db::upsert_pm_worklog(pool, &grounded, state, day_utc, id_min, id_max).await?;

    let mut reason = format!(
        "drafted (conf={:.2}, coverage={:.2}, real_s={})",
        grounded.update.confidence, grounded.coverage, bundle.real_seconds
    );
    let mut posted_worklog_id: Option<String> = None;

    // 5. Post (unless dry-run / ineligible).
    if dry_run {
        reason = format!("{reason}; dry_run — not posted");
    } else if let Some(skip) = post_ineligibility(state, &bundle, cfg) {
        reason = format!("{reason}; post skipped: {skip}");
    } else if let Some(jira_cfg) = jira {
        match post_or_recover(pool, jira_cfg, &bundle, &grounded.update.summary).await {
            Ok(wid) => {
                db::mark_worklog_posted(pool, pm_worklog_id, &wid).await?;
                state = UpdateState::Posted;
                posted_worklog_id = Some(wid.clone());
                reason = format!("worklog {wid} posted");
            }
            Err(e) => {
                reason = format!("{reason}; jira post failed: {e}");
                tracing::warn!(task_key, error = %e, "jira worklog post failed — left DRAFTED");
            }
        }
    } else {
        reason = format!("{reason}; no Jira provider configured");
    }

    Ok(TaskOutcome {
        task_key: task_key.to_string(),
        state,
        reason,
        pm_worklog_id: Some(pm_worklog_id),
        posted_worklog_id,
    })
}

/// Return `Some(reason)` if this row must not post; `None` if eligible.
/// Ticket-closed is intentionally NOT a gate (we keep logging time after done).
fn post_ineligibility(
    state: UpdateState,
    bundle: &super::models::SessionBundle,
    cfg: &PmWorklogConfig,
) -> Option<String> {
    if state == UpdateState::Skipped {
        return Some("row marked SKIPPED".to_string());
    }
    if bundle.real_seconds < cfg.min_post_seconds {
        return Some(format!(
            "real_seconds={} below Jira's {}s minimum",
            bundle.real_seconds, cfg.min_post_seconds
        ));
    }
    None
}

/// Post a worklog, or recover the id if this window was already posted (the
/// idempotency short-circuit that makes restarts/backfill safe).
async fn post_or_recover(
    pool: &SqlitePool,
    jira: &JiraConfig,
    bundle: &super::models::SessionBundle,
    summary: &str,
) -> Result<String> {
    if let Some((prior_id, worklog_id)) = db::find_existing_worklog(
        pool,
        &bundle.task_key,
        &bundle.window_start,
        &bundle.window_end,
    )
    .await?
    {
        tracing::info!(
            task_key = %bundle.task_key,
            worklog_id = %worklog_id,
            row = prior_id,
            "worklog already posted for this window — skipping re-post"
        );
        return Ok(worklog_id);
    }

    let comment = summary.trim();
    let result = jira::post_worklog(
        jira,
        &bundle.task_key,
        bundle.real_seconds,
        &bundle.window_start,
        comment,
    )
    .await?;
    Ok(result.worklog_id)
}
