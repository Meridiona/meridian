// meridian — normalises screenpipe activity into structured app sessions
//
// The approved-worklog poster. This is the ONLY path that writes to real Jira.
// The hour-driven driver (`scheduler.rs`) only ever DRAFTS; a human reviews,
// optionally edits, and approves each worklog in the dashboard (which flips the
// row to `approved`). This sweep then picks approved rows up and posts them via
// `jira.rs`, on a fast cadence independent of the hourly driver so "approve in
// the UI" feels close to immediate.
//
// Idempotent: a window already POSTED short-circuits (the `find_existing_worklog`
// backstop), so a restart mid-sweep can never double-post. A post failure leaves
// the row `approved` with `last_post_error` recorded, and the next sweep retries.

use anyhow::Result;
use sqlx::SqlitePool;
use tokio::sync::watch;

use crate::config::{Config, JiraConfig, PmProviderConfig};

use super::config::PmWorklogConfig;
use super::{db, jira};

/// How often the approved-sweep runs. Short by design — this is the latency a
/// user feels between clicking "Approve" in the dashboard and the worklog
/// landing in Jira.
const POST_SWEEP_INTERVAL_SECS: u64 = 60;

/// Outcome of one approved-sweep pass.
#[derive(Debug, Default, Clone)]
pub struct PostSweepSummary {
    pub approved_seen: u32,
    pub posted: u32,
    pub failed: u32,
}

/// Post every approved worklog. `jira` is `None` when no Jira provider is
/// configured — then approved rows are left in place (nothing to post to) and a
/// warning is logged once per pass.
pub async fn post_approved(
    pool: &SqlitePool,
    jira: Option<&JiraConfig>,
    cfg: &PmWorklogConfig,
) -> Result<PostSweepSummary> {
    let approved = db::fetch_approved_worklogs(pool).await?;
    let mut summary = PostSweepSummary {
        approved_seen: approved.len() as u32,
        ..Default::default()
    };
    if approved.is_empty() {
        return Ok(summary);
    }

    let Some(jira) = jira else {
        tracing::warn!(
            approved = approved.len(),
            "approved worklogs waiting but no Jira provider configured — not posting"
        );
        return Ok(summary);
    };

    for w in approved {
        match post_one(pool, jira, cfg, &w).await {
            Ok(true) => summary.posted += 1,
            Ok(false) => {} // ineligible/skipped — already recorded
            Err(e) => {
                summary.failed += 1;
                tracing::warn!(
                    pm_worklog_id = w.id, task = %w.task_key, error = %e,
                    "approved worklog post failed — left approved for retry"
                );
                db::mark_post_failed(pool, w.id, &format!("{e:#}")).await?;
            }
        }
    }
    Ok(summary)
}

/// Post one approved worklog. Returns `Ok(true)` if it was posted, `Ok(false)` if
/// it was skipped as ineligible (too short — recorded as a non-retryable error),
/// or `Err` on a transient failure the caller should record for retry.
async fn post_one(
    pool: &SqlitePool,
    jira: &JiraConfig,
    cfg: &PmWorklogConfig,
    w: &db::ApprovedWorklog,
) -> Result<bool> {
    if w.comment.trim().is_empty() {
        // Nothing to post — the user approved an empty draft. Terminal: editing
        // the worklog re-drafts it, so don't keep retrying.
        db::fail_worklog(pool, w.id, "approved worklog has an empty comment").await?;
        return Ok(false);
    }
    if w.time_spent_seconds < cfg.min_post_seconds {
        // Below Jira's hard floor — terminal; nothing the sweep can do.
        db::fail_worklog(
            pool,
            w.id,
            &format!(
                "time_spent={}s below Jira's {}s minimum",
                w.time_spent_seconds, cfg.min_post_seconds
            ),
        )
        .await?;
        return Ok(false);
    }

    // Idempotency backstop: if this exact (task, window) was already posted,
    // adopt that worklog id instead of posting again.
    if let Some((_, worklog_id)) =
        db::find_existing_worklog(pool, &w.task_key, &w.window_start, &w.window_end).await?
    {
        tracing::info!(
            pm_worklog_id = w.id, task = %w.task_key, %worklog_id,
            "window already posted — adopting existing worklog id"
        );
        db::mark_worklog_posted(pool, w.id, &worklog_id).await?;
        return Ok(true);
    }

    let result = jira::post_worklog(
        jira,
        &w.task_key,
        w.time_spent_seconds,
        &w.window_start,
        w.comment.trim(),
    )
    .await?;
    db::mark_worklog_posted(pool, w.id, &result.worklog_id).await?;
    tracing::info!(
        pm_worklog_id = w.id, task = %w.task_key, worklog_id = %result.worklog_id,
        time_spent = %result.time_spent_jira, "approved worklog posted to Jira"
    );
    Ok(true)
}

/// First Jira provider in the daemon config, if any.
fn jira_from_config(config: &Config) -> Option<JiraConfig> {
    config.pm_providers.iter().find_map(|p| match p {
        PmProviderConfig::Jira(j) => Some(j.clone()),
        _ => None,
    })
}

/// Daemon task: post approved worklogs on a short cadence until shutdown.
pub async fn run_post_loop(pool: SqlitePool, mut shutdown_rx: watch::Receiver<bool>) {
    let cfg = PmWorklogConfig::from_env();
    let interval = std::time::Duration::from_secs(POST_SWEEP_INTERVAL_SECS);
    tracing::info!(
        interval_s = interval.as_secs(),
        "pm-worklog approved-poster starting (approval is the only post gate)"
    );

    loop {
        let config = Config::from_env();
        let jira = jira_from_config(&config);
        match post_approved(&pool, jira.as_ref(), &cfg).await {
            Ok(s) if s.posted > 0 || s.failed > 0 => tracing::info!(
                approved = s.approved_seen,
                posted = s.posted,
                failed = s.failed,
                "approved-poster pass complete"
            ),
            Ok(_) => {}
            Err(e) => tracing::error!(error = %e, "approved-poster pass failed"),
        }

        tokio::select! {
            _ = shutdown_rx.changed() => break,
            _ = tokio::time::sleep(interval) => {}
        }
    }
    tracing::info!("pm-worklog approved-poster stopped");
}

/// One-shot CLI: `meridian worklog-post-approved` — post every approved worklog
/// now (the same sweep the daemon runs, for manual/testing use).
pub async fn cli_post_approved(pool: &SqlitePool) {
    let cfg = PmWorklogConfig::from_env();
    let config = Config::from_env();
    let jira = jira_from_config(&config);
    match post_approved(pool, jira.as_ref(), &cfg).await {
        Ok(s) => println!(
            "worklog-post-approved: approved={} posted={} failed={} (jira={})",
            s.approved_seen,
            s.posted,
            s.failed,
            jira.is_some(),
        ),
        Err(e) => eprintln!("worklog-post-approved: sweep failed: {e:#}"),
    }
}
