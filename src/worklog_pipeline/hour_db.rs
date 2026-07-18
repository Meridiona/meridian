//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! DB reads + writes for the Rust-owned hour report.
//!
//! Ports the queries the Python `worklog_pipeline/db.py` used for the report stage
//! (`fetch_hour_timeline`, `fetch_coding_summaries`, `persist_hour_text`,
//! `persist_hour_report`), against the same `app_sessions` / `pm_worklog_hours` tables.
//! Everything else `db.py` did (candidates, classification bindings, `pm_worklogs`
//! drafts) belonged to the classify/generate/propose stages, which are removed — so only
//! these four cross over.
//!
//! The `hour_start` key is the UTC hour start with a `+00:00` suffix, exactly the value
//! [`crate::pm_worklog::ledger`] already stamps on the `pm_worklog_hours` row, so a report
//! UPDATE always targets the ledger row the driver created.

use anyhow::{Context, Result};
use sqlx::{Row, SqlitePool};
use tracing::Instrument;

use super::hour_input::{CodingRow, TimelineRow};

/// Sessions under this many seconds are alt-tab flicker (median raw session ~6 s); 30 s
/// keeps the timeline legible without dropping real work. Matches the Python default.
const MIN_TIMELINE_DURATION_S: i64 = 30;

/// The hour's MEASURED session timeline (screen *and* coding-agent), in time order, with
/// real timestamps and the window title that names each. Load-bearing model input — the
/// only source of clock times — so it is uncapped. Missing table/column degrades to empty.
pub async fn fetch_hour_timeline(pool: &SqlitePool, hs: &str, he: &str) -> Vec<TimelineRow> {
    let res = sqlx::query(
        "SELECT app_name, started_at, ended_at, \
                CAST(COALESCE(duration_s, 0) AS INTEGER) AS duration_s, \
                COALESCE(window_titles, '') AS window_titles, \
                (coding_agent_session_uuid IS NOT NULL) AS is_coding, \
                traceparent \
         FROM app_sessions \
         WHERE started_at >= ? AND started_at < ? AND COALESCE(duration_s, 0) >= ? \
         ORDER BY started_at",
    )
    .bind(hs)
    .bind(he)
    .bind(MIN_TIMELINE_DURATION_S)
    .fetch_all(pool)
    .instrument(tracing::debug_span!("worklog.hour.read.timeline"))
    .await;

    match res {
        Ok(rows) => {
            let out: Vec<TimelineRow> = rows
                .into_iter()
                .map(|r| TimelineRow {
                    app_name: r.try_get::<String, _>("app_name").unwrap_or_default(),
                    started_at: r.try_get::<String, _>("started_at").unwrap_or_default(),
                    ended_at: r.try_get::<String, _>("ended_at").unwrap_or_default(),
                    duration_s: r.try_get::<i64, _>("duration_s").unwrap_or(0),
                    window_titles: r.try_get::<String, _>("window_titles").unwrap_or_default(),
                    is_coding: r.try_get::<i64, _>("is_coding").unwrap_or(0) != 0,
                    traceparent: r
                        .try_get::<Option<String>, _>("traceparent")
                        .unwrap_or(None),
                })
                .collect();
            tracing::debug!(rows = out.len(), "worklog: fetched hour timeline");
            out
        }
        Err(e) => {
            tracing::warn!(error = %e, "worklog: hour timeline query failed — treating as empty");
            Vec::new()
        }
    }
}

/// Coding-agent sessions for the hour that already carry a summary, in time order. These
/// are folded into the activity summary VERBATIM (never re-compressed). A still-live row
/// without a summary is simply absent — the driver's readiness gate waits for them first.
pub async fn fetch_coding_summaries(pool: &SqlitePool, hs: &str, he: &str) -> Vec<CodingRow> {
    let res = sqlx::query(
        "SELECT app_name, started_at, ended_at, \
                CAST(COALESCE(duration_s, 0) AS INTEGER) AS duration_s, \
                session_summary \
         FROM app_sessions \
         WHERE started_at >= ? AND started_at < ? \
           AND coding_agent_session_uuid IS NOT NULL \
           AND session_summary IS NOT NULL AND LENGTH(TRIM(session_summary)) > 0 \
         ORDER BY started_at",
    )
    .bind(hs)
    .bind(he)
    .fetch_all(pool)
    .instrument(tracing::debug_span!("worklog.hour.read.coding"))
    .await;

    match res {
        Ok(rows) => {
            let out: Vec<CodingRow> = rows
                .into_iter()
                .map(|r| CodingRow {
                    app_name: r.try_get::<String, _>("app_name").unwrap_or_default(),
                    started_at: r.try_get::<String, _>("started_at").unwrap_or_default(),
                    ended_at: r.try_get::<String, _>("ended_at").unwrap_or_default(),
                    duration_s: r.try_get::<i64, _>("duration_s").unwrap_or(0),
                    session_summary: r
                        .try_get::<String, _>("session_summary")
                        .unwrap_or_default(),
                })
                .collect();
            tracing::debug!(rows = out.len(), "worklog: fetched coding summaries");
            out
        }
        Err(e) => {
            tracing::warn!(error = %e, "worklog: coding-summary query failed — treating as empty");
            Vec::new()
        }
    }
}

/// Persist the distilled INPUT body onto the hour ledger row — shown by the dashboard's
/// hour-detail panel even for hours that don't yield a report. UPDATE-only: a missing
/// ledger row or a pre-053 DB (no `hour_text` column) is logged and skipped, never an
/// error that fails the hour.
pub async fn persist_hour_text(
    pool: &SqlitePool,
    hour_start: &str,
    body: &str,
    out_chars: i64,
    reduction_pct: f64,
) -> Result<()> {
    let res = sqlx::query(
        "UPDATE pm_worklog_hours \
         SET hour_text = ?, hour_text_chars = ?, hour_text_reduction_pct = ? \
         WHERE hour_start = ?",
    )
    .bind(body)
    .bind(out_chars)
    .bind(reduction_pct)
    .bind(hour_start)
    .execute(pool)
    .instrument(tracing::debug_span!("worklog.hour.write.text"))
    .await;

    match res {
        Ok(r) if r.rows_affected() == 0 => {
            tracing::warn!(
                hour_start,
                "worklog: no pm_worklog_hours row for hour_text — dropped"
            );
            Ok(())
        }
        Ok(_) => Ok(()),
        // A pre-053 DB lacks the column: skip, don't fail the hour.
        Err(sqlx::Error::Database(e)) if e.message().contains("no such column") => {
            tracing::warn!("worklog: pm_worklog_hours.hour_text absent (pre-053 DB) — skipped");
            Ok(())
        }
        Err(e) => Err(e).context("persist hour_text"),
    }
}

/// Persist the REPORT (the model's human-readable output) onto the hour ledger row — the
/// dashboard's "ACTIVITY SUMMARY" box. Same UPDATE-only, pre-054-tolerant contract as
/// [`persist_hour_text`].
pub async fn persist_hour_report(pool: &SqlitePool, hour_start: &str, report: &str) -> Result<()> {
    let res = sqlx::query(
        "UPDATE pm_worklog_hours SET hour_report = ?, hour_report_chars = ? WHERE hour_start = ?",
    )
    .bind(report)
    .bind(report.chars().count() as i64)
    .bind(hour_start)
    .execute(pool)
    .instrument(tracing::debug_span!("worklog.hour.write.report"))
    .await;

    match res {
        Ok(r) if r.rows_affected() == 0 => {
            tracing::warn!(
                hour_start,
                "worklog: no pm_worklog_hours row for hour_report — dropped"
            );
            Ok(())
        }
        Ok(_) => Ok(()),
        Err(sqlx::Error::Database(e)) if e.message().contains("no such column") => {
            tracing::warn!("worklog: pm_worklog_hours.hour_report absent (pre-054 DB) — skipped");
            Ok(())
        }
        Err(e) => Err(e).context("persist hour_report"),
    }
}
