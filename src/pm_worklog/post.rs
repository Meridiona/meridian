// meridian — normalises screenpipe activity into structured app sessions
//
// The approved-worklog poster. This is the ONLY path that writes to a real
// tracker. The hour-driven driver (`scheduler.rs`) only ever DRAFTS; a human
// reviews, optionally edits, and approves each worklog in the dashboard (which
// flips the row to `approved`). This sweep then picks approved rows up and posts
// them to whichever tracker the row belongs to (`pm_worklogs.provider`):
//
//   jira   → native worklog endpoint        (`jira.rs`)
//   linear → structured `commentCreate`      (`linear.rs`)  — no native worklog API
//   github → structured issue comment (REST) (`github.rs`)  — no native time tracking
//
// The sweep runs on a fast cadence independent of the hourly driver so "approve
// in the UI" feels close to immediate.
//
// Idempotent: a window already POSTED short-circuits (the `find_existing_worklog`
// backstop), so a restart mid-sweep can never double-post. A post failure leaves
// the row `approved` with `last_post_error` recorded, and the next sweep retries.

use anyhow::Result;
use sqlx::SqlitePool;
use tokio::sync::watch;

use crate::config::{Config, GitHubConfig, JiraConfig, LinearConfig, PmProviderConfig};

use super::config::PmWorklogConfig;
use super::{db, github, jira, linear};

/// How often the approved-sweep runs. Short by design — this is the latency a
/// user feels between clicking "Approve" in the dashboard and the worklog
/// landing in their tracker.
const POST_SWEEP_INTERVAL_SECS: u64 = 60;

/// Outcome of one approved-sweep pass.
#[derive(Debug, Default, Clone)]
pub struct PostSweepSummary {
    pub approved_seen: u32,
    pub posted: u32,
    pub failed: u32,
}

/// Post every approved worklog, routing each to its provider. Rows whose
/// provider is not configured on this daemon are left in place (nothing to post
/// to) with a warning — the same forgiving behaviour the Jira-only poster had.
pub async fn post_approved(
    pool: &SqlitePool,
    config: &Config,
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

    for w in approved {
        match post_one(pool, config, cfg, &w).await {
            Ok(true) => summary.posted += 1,
            Ok(false) => {} // ineligible/skipped — already recorded
            Err(e) => {
                summary.failed += 1;
                tracing::warn!(
                    pm_worklog_id = w.id, task = %w.task_key, provider = %w.provider, error = %e,
                    "approved worklog post failed — left approved for retry"
                );
                db::mark_post_failed(pool, w.id, &format!("{e:#}")).await?;
            }
        }
    }
    Ok(summary)
}

/// Post one approved worklog. Returns `Ok(true)` if it was posted, `Ok(false)` if
/// it was skipped as ineligible (too short / empty / no matching provider — each
/// recorded), or `Err` on a transient failure the caller should record for retry.
async fn post_one(
    pool: &SqlitePool,
    config: &Config,
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
        // Below the worklog floor — terminal; nothing the sweep can do.
        db::fail_worklog(
            pool,
            w.id,
            &format!(
                "time_spent={}s below the {}s minimum",
                w.time_spent_seconds, cfg.min_post_seconds
            ),
        )
        .await?;
        return Ok(false);
    }

    // Idempotency backstop: if this exact (task, window) was already posted,
    // adopt that worklog/comment id instead of posting again.
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

    // Route to the provider this worklog was drafted against. A missing provider
    // config (e.g. the row is for Linear but only Jira is configured here) is not
    // a failure — leave it approved and warn, so it posts once configured.
    let comment = w.comment.trim();
    let (worklog_id, label) = match w.provider.as_str() {
        "jira" => {
            let Some(c) = jira_cfg(config) else {
                return missing_provider(&w.provider, w);
            };
            let r = jira::post_worklog(
                c,
                &w.task_key,
                w.time_spent_seconds,
                &w.window_start,
                comment,
            )
            .await?;
            (r.worklog_id, r.time_spent_jira)
        }
        "linear" => {
            let Some(c) = linear_cfg(config) else {
                return missing_provider(&w.provider, w);
            };
            let r = linear::post_worklog(
                c,
                &w.task_key,
                w.time_spent_seconds,
                &w.window_start,
                &w.window_end,
                comment,
            )
            .await?;
            (r.id, r.label)
        }
        "github" => {
            let Some(c) = github_cfg(config) else {
                return missing_provider(&w.provider, w);
            };
            let r = github::post_worklog(
                c,
                &w.task_key,
                w.time_spent_seconds,
                &w.window_start,
                &w.window_end,
                comment,
            )
            .await?;
            (r.id, r.label)
        }
        other => {
            db::fail_worklog(pool, w.id, &format!("unknown provider '{other}'")).await?;
            return Ok(false);
        }
    };

    db::mark_worklog_posted(pool, w.id, &worklog_id).await?;
    tracing::info!(
        pm_worklog_id = w.id, task = %w.task_key, provider = %w.provider,
        worklog_id = %worklog_id, time_spent = %label, "approved worklog posted"
    );
    Ok(true)
}

/// The worklog's provider is not configured on this daemon: leave it approved
/// (nothing to post to) and warn once. Returns `Ok(false)` (not posted, not a
/// hard error — it will post when the provider is configured).
fn missing_provider(provider: &str, w: &db::ApprovedWorklog) -> Result<bool> {
    tracing::warn!(
        pm_worklog_id = w.id, task = %w.task_key, %provider,
        "approved worklog waiting but its provider is not configured — not posting"
    );
    Ok(false)
}

fn jira_cfg(config: &Config) -> Option<&JiraConfig> {
    config.pm_providers.iter().find_map(|p| match p {
        PmProviderConfig::Jira(j) => Some(j),
        _ => None,
    })
}

fn github_cfg(config: &Config) -> Option<&GitHubConfig> {
    config.pm_providers.iter().find_map(|p| match p {
        PmProviderConfig::GitHub(g) => Some(g),
        _ => None,
    })
}

fn linear_cfg(config: &Config) -> Option<&LinearConfig> {
    config.pm_providers.iter().find_map(|p| match p {
        PmProviderConfig::Linear(l) => Some(l),
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
        match post_approved(&pool, &config, &cfg).await {
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
    let providers: Vec<&str> = config
        .pm_providers
        .iter()
        .map(|p| p.provider_name())
        .collect();
    match post_approved(pool, &config, &cfg).await {
        Ok(s) => println!(
            "worklog-post-approved: approved={} posted={} failed={} (providers={:?})",
            s.approved_seen, s.posted, s.failed, providers,
        ),
        Err(e) => eprintln!("worklog-post-approved: sweep failed: {e:#}"),
    }
}
