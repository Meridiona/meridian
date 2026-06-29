//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Hour-level worklog driver — the replacement for the per-session classifier +
// per-task pm_worklog synth. Walks hours from local-midnight → now; for each
// READY hour it POSTs `/worklog_hour` to the MLX server ONCE, which runs the full
// agno worklog pipeline (distil → report → rerank → match → worklog/propose →
// persist). Hours are independent; a not-ready hour is left for the next pass.
//
// Reuses pm_worklog's readiness + ledger machinery (ensure_hour / hour_is_done /
// upstream_settled / mark_hour_done) so the settle/aging behaviour is identical.
// It NEVER posts to Jira — drafts land in pm_worklogs for the dashboard to
// approve, and pm_worklog::run_post_loop posts the approved rows (unchanged).

use std::sync::Arc;
use std::time::Duration as StdDuration;

use anyhow::{Context, Result};
use chrono::{Duration, Local, NaiveDate, TimeZone, Utc};
use serde_json::json;
use sqlx::SqlitePool;
use tokio::sync::{watch, Notify};

use crate::pm_worklog::{ledger, PmWorklogConfig};

/// Canonical `+00:00` ISO bound (matches stored `started_at`).
fn iso_bound(dt: chrono::DateTime<Utc>) -> String {
    dt.format("%Y-%m-%dT%H:%M:%S+00:00").to_string()
}

/// The hour label the Python pipeline expects: `YYYY-MM-DDTHH` in local time.
/// Sessions are bucketed by the user's local clock hour, not UTC.
fn hour_label(dt: chrono::DateTime<Local>) -> String {
    dt.format("%Y-%m-%dT%H").to_string()
}

#[derive(Debug, Default, Clone)]
pub struct DriverSummary {
    pub hours_seen: u32,
    pub hours_processed: u32,
    pub hours_not_ready: u32,
    pub hours_errored: u32,
}

/// POST one hour to `/worklog_hour`. Holds the global LLM permit for the whole
/// call so the pipeline's many model phases (embedder / reranker / 2B) never
/// interleave with a summarise call — preserving the one-model-at-a-time rule.
async fn post_worklog_hour(
    cfg: &PmWorklogConfig,
    db_path: &str,
    hour: &str,
    cycle_index: i64,
) -> Result<()> {
    let _llm_permit = crate::llm_gate::acquire().await;

    let url = format!("http://{}:{}/worklog_hour", cfg.mlx_host, cfg.mlx_port);
    let client = reqwest::Client::builder()
        .connect_timeout(StdDuration::from_secs(5))
        // The full pipeline can run for several minutes on a busy hour.
        .timeout(StdDuration::from_secs(900))
        .build()
        .context("building worklog_hour http client")?;

    let traceparent = crate::observability::current_traceparent();
    let resp = client
        .post(&url)
        .json(&json!({
            "hour": hour,
            "db_path": db_path,
            "cycle_index": cycle_index,
            "traceparent": traceparent,
        }))
        .send()
        .await
        .with_context(|| {
            format!("worklog_hour endpoint unreachable at {url} — is the MLX server running?")
        })?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        let preview: String = body.chars().take(200).collect();
        anyhow::bail!("/worklog_hour returned {status}: {preview}");
    }
    Ok(())
}

/// Run one pass over a day's hours, drafting worklogs via `/worklog_hour`.
///
/// Walks the 24 LOCAL clock-hour buckets of `day`. Labels are in local time
/// (`YYYY-MM-DDTHH` local), matching the user's clock. The Python distiller
/// converts local-hour labels to UTC bounds at query time.
pub async fn run_driver(
    pool: &SqlitePool,
    cfg: &PmWorklogConfig,
    db_path: &str,
    min_duration_s: i64,
    day: Option<NaiveDate>,
) -> Result<DriverSummary> {
    let now = Utc::now();
    // Walk local day: midnight in the machine's local timezone.
    let local_day = day.unwrap_or_else(|| Local::now().date_naive());
    let aging = Duration::minutes(cfg.readiness_aging_minutes);
    let day_local = local_day.format("%Y-%m-%d").to_string();

    let midnight_local = Local
        .from_local_datetime(
            &local_day
                .and_hms_opt(0, 0, 0)
                .context("constructing local midnight")?,
        )
        .single()
        .context("ambiguous local midnight (DST transition)")?;
    let mut summary = DriverSummary::default();

    for i in 0..24i64 {
        let hs_local = midnight_local + Duration::hours(i);
        let he_local = hs_local + Duration::hours(1);
        let hs_utc = hs_local.with_timezone(&Utc);
        let he_utc = he_local.with_timezone(&Utc);
        if hs_utc >= now {
            break; // hour hasn't started yet
        }

        let hs = iso_bound(hs_utc);
        let he = iso_bound(he_utc);
        let label = hour_label(hs_local);
        let cycle_index = i;

        summary.hours_seen += 1;
        ledger::ensure_hour(pool, &day_local, &hs, &he)
            .await
            .with_context(|| format!("ensure_hour {hs}"))?;

        if ledger::hour_is_done(pool, &hs)
            .await
            .with_context(|| format!("hour_is_done {hs}"))?
        {
            continue;
        }
        if now < he_utc {
            continue; // hour not over yet
        }
        let aged_out = now >= he_utc + aging;
        let settled = ledger::upstream_settled(pool, &hs, &he, min_duration_s)
            .await
            .with_context(|| format!("upstream_settled {hs}"))?;
        if !settled && !aged_out {
            summary.hours_not_ready += 1;
            tracing::debug!(hour = %hs, "hour not ready — upstream still settling");
            continue;
        }

        tracing::info!(hour = %label, aged_out, "running worklog pipeline for ready hour");
        match post_worklog_hour(cfg, db_path, &label, cycle_index).await {
            Ok(()) => {
                ledger::mark_hour_done(pool, &hs, 0)
                    .await
                    .with_context(|| format!("mark_hour_done {hs}"))?;
                summary.hours_processed += 1;
            }
            Err(e) => {
                // Leave the hour pending so it retries on the next pass.
                summary.hours_errored += 1;
                tracing::warn!(hour = %label, error = %e, "worklog pipeline errored — hour left pending");
            }
        }
    }

    Ok(summary)
}

/// One-shot CLI: run a single driver pass for `day` (default today). Mirrors
/// `meridian pm-worklog` but drives the new hour-level pipeline. Useful for
/// manual runs and integration testing without the full daemon.
pub async fn cli_run(pool: &SqlitePool, db_path: &str, day: Option<&str>) {
    let cfg = PmWorklogConfig::from_env();
    let day_parsed = match day {
        None => None,
        Some(d) => match NaiveDate::parse_from_str(d, "%Y-%m-%d") {
            Ok(date) => Some(date),
            Err(e) => {
                eprintln!("worklog-pipeline: invalid --day value {d:?}: {e}");
                return;
            }
        },
    };
    match run_driver(pool, &cfg, db_path, 0, day_parsed).await {
        Ok(s) => println!(
            "worklog-pipeline: hours_seen={} processed={} not_ready={} errored={}",
            s.hours_seen, s.hours_processed, s.hours_not_ready, s.hours_errored
        ),
        Err(e) => eprintln!("worklog-pipeline: {e}"),
    }
}

/// Daemon task: run the hour driver until shutdown. Drafts only — posting stays
/// with pm_worklog::run_post_loop. Woken by the worklog notify (ETL settle) or a
/// periodic fallback that covers aging + midnight rollover.
pub async fn run_loop(
    pool: SqlitePool,
    db_path: String,
    mut shutdown_rx: watch::Receiver<bool>,
    wake: Arc<Notify>,
) {
    let cfg = PmWorklogConfig::from_env();
    let interval = StdDuration::from_secs((cfg.interval_hours * 3600.0).max(60.0) as u64);
    tracing::info!(
        interval_s = interval.as_secs(),
        "worklog-pipeline driver starting (hour-level /worklog_hour; drafts only)"
    );

    let run_pass = |_scope_today: bool| {
        let pool = pool.clone();
        let cfg = cfg.clone();
        let db_path = db_path.clone();
        async move {
            let min_duration_s = 0;
            // Today only (local time) — no yesterday backfill.
            let days: Vec<Option<NaiveDate>> = vec![None];
            for day in days {
                if let Err(e) = run_driver(&pool, &cfg, &db_path, min_duration_s, day).await {
                    tracing::warn!(error = %e, "worklog-pipeline pass errored");
                }
            }
        }
    };

    run_pass(false).await; // startup catch-up

    let mut backfill_timer = tokio::time::interval(interval);
    backfill_timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    backfill_timer.tick().await; // consume immediate first tick

    loop {
        let scope_today = tokio::select! {
            _ = shutdown_rx.changed() => break,
            _ = wake.notified() => true,
            _ = backfill_timer.tick() => false,
        };
        run_pass(scope_today).await;
    }

    tracing::info!("worklog-pipeline driver stopped");
}
