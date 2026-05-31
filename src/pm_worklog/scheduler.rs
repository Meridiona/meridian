// meridian — normalises screenpipe activity into structured app sessions
//
// The hour-driven driver. Walk hours from local-midnight → now; for each hour
// that is READY, write one worklog per task that had classified work in it, then
// mark the hour done. Hours are independent — a not-ready hour is left for the
// next pass and does NOT block later hours, so a single stuck classification can
// never freeze the day. The aging escape (config) bounds how long we wait for an
// hour to settle before processing it best-effort.

use anyhow::Result;
use chrono::{Duration, Local, NaiveDate, TimeZone, Utc};
use sqlx::SqlitePool;
use tokio::sync::watch;

use crate::config::{Config, PmProviderConfig};

use super::config::PmWorklogConfig;
use super::models::UpdateState;
use super::{ledger, route};

/// Canonical `+00:00` ISO bound (matches the stored `started_at` format).
fn iso_bound(dt: chrono::DateTime<Utc>) -> String {
    dt.format("%Y-%m-%dT%H:%M:%S+00:00").to_string()
}

/// Outcome of one driver pass.
#[derive(Debug, Default, Clone)]
pub struct DriverSummary {
    pub hours_seen: u32,
    pub hours_processed: u32,
    pub hours_not_ready: u32,
    pub worklogs_drafted: u32,
    pub worklogs_posted: u32,
    pub worklogs_skipped: u32,
}

/// Run one full pass over a day's hours. `day` defaults to today (local).
/// `dry_run` drafts rows but never POSTs.
pub async fn run_driver(
    pool: &SqlitePool,
    jira: Option<&crate::config::JiraConfig>,
    cfg: &PmWorklogConfig,
    min_duration_s: i64,
    day: Option<NaiveDate>,
    dry_run: bool,
) -> Result<DriverSummary> {
    let now = Local::now();
    let day = day.unwrap_or_else(|| now.date_naive());
    let aging = Duration::minutes(cfg.readiness_aging_minutes);

    let midnight = match Local
        .from_local_datetime(&day.and_hms_opt(0, 0, 0).unwrap())
        .single()
    {
        Some(m) => m,
        None => {
            tracing::warn!(?day, "could not resolve local midnight — skipping");
            return Ok(DriverSummary::default());
        }
    };

    let mut summary = DriverSummary::default();

    for i in 0..24i64 {
        let hs_local = midnight + Duration::hours(i);
        let he_local = hs_local + Duration::hours(1);
        if hs_local >= now {
            break; // hour hasn't started yet
        }

        let hs = iso_bound(hs_local.with_timezone(&Utc));
        let he = iso_bound(he_local.with_timezone(&Utc));
        let hs_utc = hs_local.with_timezone(&Utc);
        let day_utc = hs_utc.format("%Y-%m-%d").to_string();
        let cycle_index = i;

        summary.hours_seen += 1;
        ledger::ensure_hour(pool, &day_utc, &hs, &he).await?;

        if ledger::hour_is_done(pool, &hs).await? {
            continue;
        }

        let hour_over = now >= he_local;
        if !hour_over {
            continue;
        }
        let aged_out = now >= he_local + aging;
        let settled = ledger::upstream_settled(pool, &hs, &he, min_duration_s).await?;
        if !settled && !aged_out {
            summary.hours_not_ready += 1;
            tracing::debug!(hour = %hs, "hour not ready — upstream still settling");
            continue;
        }

        // READY — write one worklog per task with classified work this hour.
        let tasks = ledger::tasks_in_hour(pool, &hs, &he).await?;
        tracing::info!(
            hour = %hs, tasks = tasks.len(), aged_out, dry_run,
            "processing ready hour"
        );

        let mut hour_had_error = false;
        for task_key in &tasks {
            match route::process_task(
                pool,
                jira,
                cfg,
                task_key,
                &hs,
                &he,
                &day_utc,
                cycle_index,
                dry_run,
            )
            .await
            {
                Ok(outcome) => {
                    match outcome.state {
                        UpdateState::Posted => summary.worklogs_posted += 1,
                        UpdateState::Drafted => summary.worklogs_drafted += 1,
                        UpdateState::Skipped | UpdateState::Failed => summary.worklogs_skipped += 1,
                    }
                    tracing::info!(
                        task = %task_key, state = outcome.state.as_str(),
                        reason = %outcome.reason, "worklog cycle done"
                    );
                }
                Err(e) => {
                    // A hard error (synth/network/db) — leave the hour pending so
                    // it retries, rather than silently losing this task's worklog.
                    hour_had_error = true;
                    summary.worklogs_skipped += 1;
                    tracing::warn!(task = %task_key, hour = %hs, error = %e, "worklog cycle errored");
                }
            }
        }

        if hour_had_error {
            tracing::warn!(
                hour = %hs,
                "hour left pending — a task errored; will retry on the next pass"
            );
        } else {
            ledger::mark_hour_done(pool, &hs, tasks.len() as i64).await?;
            summary.hours_processed += 1;
        }
    }

    Ok(summary)
}

/// First Jira provider in the daemon config, if any (worklogs only post to Jira).
fn jira_from_config(config: &Config) -> Option<crate::config::JiraConfig> {
    config.pm_providers.iter().find_map(|p| match p {
        PmProviderConfig::Jira(j) => Some(j.clone()),
        _ => None,
    })
}

/// Daemon task: run the driver on the configured interval until shutdown.
/// Posts only when `PM_WORKLOG_POST_ENABLED` is set — otherwise dry-run.
pub async fn run_loop(pool: SqlitePool, mut shutdown_rx: watch::Receiver<bool>) {
    let cfg = PmWorklogConfig::from_env();
    let interval = std::time::Duration::from_secs((cfg.interval_hours * 3600.0).max(60.0) as u64);
    tracing::info!(
        interval_s = interval.as_secs(),
        post_enabled = cfg.post_enabled,
        "pm-worklog driver starting"
    );

    loop {
        let config = Config::from_env();
        let jira = jira_from_config(&config);
        let dry_run = !cfg.post_enabled || jira.is_none();

        // Process yesterday AND today every pass. Yesterday's done hours skip
        // instantly via the ledger; this closes the day-rollover gap where the
        // previous day's last hours would otherwise strand at midnight.
        let today = Local::now().date_naive();
        let days: Vec<Option<NaiveDate>> = vec![today.pred_opt(), Some(today)];
        let min_duration_s = config.min_classification_duration_s;
        for day in days.into_iter().flatten() {
            match run_driver(
                &pool,
                jira.as_ref(),
                &cfg,
                min_duration_s,
                Some(day),
                dry_run,
            )
            .await
            {
                Ok(s) => tracing::info!(
                    day = %day,
                    hours_processed = s.hours_processed,
                    drafted = s.worklogs_drafted,
                    posted = s.worklogs_posted,
                    not_ready = s.hours_not_ready,
                    "pm-worklog driver pass complete"
                ),
                Err(e) => tracing::error!(day = %day, error = %e, "pm-worklog driver pass failed"),
            }
        }

        tokio::select! {
            _ = shutdown_rx.changed() => break,
            _ = tokio::time::sleep(interval) => {}
        }
    }
    tracing::info!("pm-worklog driver stopped");
}

/// One-shot CLI entry: `meridian pm-worklog [--day YYYY-MM-DD] [--dry-run]`.
pub async fn cli_run(pool: &SqlitePool, day: Option<&str>, dry_run: bool) {
    let cfg = PmWorklogConfig::from_env();
    let config = Config::from_env();
    let jira = jira_from_config(&config);
    let effective_dry = dry_run || !cfg.post_enabled || jira.is_none();

    let parsed_day = match day {
        Some(d) => match NaiveDate::parse_from_str(d, "%Y-%m-%d") {
            Ok(nd) => Some(nd),
            Err(e) => {
                eprintln!("pm-worklog: bad --day {d:?}: {e}");
                return;
            }
        },
        None => None,
    };

    println!(
        "pm-worklog: day={} dry_run={} (post_enabled={}, jira={})",
        parsed_day
            .map(|d| d.to_string())
            .unwrap_or_else(|| "today".into()),
        effective_dry,
        cfg.post_enabled,
        jira.is_some(),
    );

    match run_driver(
        pool,
        jira.as_ref(),
        &cfg,
        config.min_classification_duration_s,
        parsed_day,
        effective_dry,
    )
    .await
    {
        Ok(s) => println!(
            "pm-worklog: hours seen={} processed={} not_ready={} | worklogs drafted={} posted={} skipped={}",
            s.hours_seen, s.hours_processed, s.hours_not_ready,
            s.worklogs_drafted, s.worklogs_posted, s.worklogs_skipped,
        ),
        Err(e) => eprintln!("pm-worklog: driver failed: {e}"),
    }
}
