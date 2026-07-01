//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Per-hour activity text — the read side of the `pm_worklog_hours` `hour_text`
//! (migration 053, raw distilled INPUT) and `hour_report` (migration 054, the
//! human-readable /activity_report OUTPUT) columns.
//!
//! # What this is
//! The worklog pipeline's distill stage (`services/agents/worklog_pipeline`)
//! persists a compact activity body per hour it processes, and its report stage
//! persists the LLM-authored human-readable summary of that body — both
//! independent of whether the hour yields a worklog draft. This reader surfaces
//! the REPORT (not the raw distilled body) for the dashboard's hour-detail panel:
//! the "here's what happened this hour" text is the report — the body is kept
//! only as diagnostic/fallback data, never shown as the primary summary. There is
//! no matching Next.js route — this is new backend work, not a route port.
//!
//! Like the worklog pipeline itself, this is **today-only**: a non-today `day`
//! short-circuits to an empty response rather than querying (the pipeline never
//! back-fills, so past hours carry no going-forward guarantee).
//!
//! # Who calls this
//! The tray `get_hour_text` command → the dashboard hour-detail panel
//! (`HourDetailPanel`), fetched on the selected-hour change.
//!
//! # Related
//! - [`crate::worklogs`] — the day's worklog cards the same panel filters by hour.
//! - [`crate::date`] — the shared local-day helper this reuses for the today gate.

use crate::SqlitePool;
use chrono::{Duration, Local, NaiveDate, TimeZone, Utc};
use serde::Serialize;
use sqlx::FromRow;
use tracing::Instrument;

/// The activity report for one hour — the human-readable /activity_report
/// OUTPUT (`report`), not the raw distilled input. `None` when the hour hasn't
/// reached the report stage yet (or `day` isn't today).
#[derive(Debug, Clone, Serialize)]
pub struct HourTextResponse {
    /// The requested local hour, echoed back (`"HH"` / `"0".."23"`).
    pub hour: String,
    /// The human-readable activity report, or `None` if not yet persisted.
    pub report: Option<String>,
    /// Character count of `report` as recorded by the pipeline.
    pub report_chars: Option<i64>,
}

impl HourTextResponse {
    /// An empty response (nothing persisted / not today), echoing `hour`.
    fn empty(hour: &str) -> Self {
        Self {
            hour: hour.to_string(),
            report: None,
            report_chars: None,
        }
    }
}

#[derive(FromRow)]
struct RawHourText {
    hour_report: Option<String>,
    hour_report_chars: Option<i64>,
}

/// Build the `pm_worklog_hours.hour_start` key for a LOCAL `day` + `hour`.
///
/// The ledger keys each row on the UTC `+00:00` instant that starts the local
/// clock-hour — exactly `src/worklog_pipeline.rs`'s `iso_bound(local_midnight +
/// hours(h))`. We reproduce that here so the lookup lands on the row the driver
/// (and the Python pipeline's `local_hour_utc_bounds` write) created. Returns
/// `None` for an unparseable date, an out-of-range hour, or a nonexistent local
/// wall-clock (a DST spring-forward gap).
fn hour_start_key(day: &str, hour: &str) -> Option<String> {
    let date = NaiveDate::parse_from_str(day, "%Y-%m-%d").ok()?;
    let h: i64 = hour.trim().parse().ok()?;
    if !(0..24).contains(&h) {
        return None;
    }
    let midnight_naive = date.and_hms_opt(0, 0, 0)?;
    let midnight_local = Local.from_local_datetime(&midnight_naive).single()?;
    let hs_utc = (midnight_local + Duration::hours(h)).with_timezone(&Utc);
    Some(hs_utc.format("%Y-%m-%dT%H:%M:%S+00:00").to_string())
}

/// Read the distilled activity text for the local `day` + `hour`.
///
/// Today-only: a `day` other than the local today short-circuits to an empty
/// response (no query), matching the worklog pipeline's no-backfill convention.
/// A missing table / pre-053 columns (older DB) also degrade to an empty response
/// rather than erroring.
#[tracing::instrument(skip(pool))]
pub async fn get_hour_text(
    pool: &SqlitePool,
    day: &str,
    hour: &str,
) -> anyhow::Result<HourTextResponse> {
    // Today-only gate — past hours carry no going-forward persistence guarantee.
    if day != crate::date::today_string() {
        tracing::debug!(day, "hour_text: non-today day — empty response");
        return Ok(HourTextResponse::empty(hour));
    }

    let Some(hour_start) = hour_start_key(day, hour) else {
        tracing::warn!(day, hour, "hour_text: could not build hour_start key");
        return Ok(HourTextResponse::empty(hour));
    };

    let row = sqlx::query_as::<_, RawHourText>(
        "SELECT hour_report, hour_report_chars \
         FROM pm_worklog_hours WHERE hour_start = ?",
    )
    .bind(&hour_start)
    .fetch_optional(pool)
    .instrument(tracing::debug_span!("hour_text.read.pm_worklog_hours"))
    .await;

    let row = match row {
        Ok(row) => row,
        Err(e) => {
            // Missing table/columns on an un-migrated DB is not fatal — the hour
            // simply has no persisted text yet.
            tracing::warn!(error = %e, "hour_text: pm_worklog_hours read skipped");
            return Ok(HourTextResponse::empty(hour));
        }
    };
    tracing::debug!(
        rows = row.is_some() as i64,
        "hour_text.read.pm_worklog_hours"
    );

    let resp = match row {
        Some(r) => HourTextResponse {
            hour: hour.to_string(),
            report: r.hour_report,
            report_chars: r.hour_report_chars,
        },
        None => HourTextResponse::empty(hour),
    };
    tracing::info!(
        day,
        hour,
        has_report = resp.report.is_some(),
        "hour_text computed"
    );
    Ok(resp)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;

    async fn pool_with_hours() -> SqlitePool {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE pm_worklog_hours (hour_start TEXT PRIMARY KEY, day_utc TEXT, \
                hour_end TEXT, status TEXT, task_count INTEGER, processed_at TEXT, \
                hour_report TEXT, hour_report_chars INTEGER)",
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    #[tokio::test]
    async fn non_today_short_circuits_without_querying() {
        let pool = pool_with_hours().await;
        // A date that is definitely not today → empty, no error even though we
        // never seeded a row.
        let resp = get_hour_text(&pool, "2000-01-01", "9").await.unwrap();
        assert!(resp.report.is_none());
        assert_eq!(resp.hour, "9");
    }

    #[tokio::test]
    async fn reads_persisted_report_for_today() {
        let pool = pool_with_hours().await;
        let today = crate::date::today_string();
        let hour_start = hour_start_key(&today, "9").unwrap();
        sqlx::query(
            "INSERT INTO pm_worklog_hours (hour_start, day_utc, hour_end, status, task_count, \
                hour_report, hour_report_chars) \
             VALUES (?, ?, '', 'done', 0, 'Spent the hour debugging the worklog pipeline.', 47)",
        )
        .bind(&hour_start)
        .bind(&today)
        .execute(&pool)
        .await
        .unwrap();

        let resp = get_hour_text(&pool, &today, "9").await.unwrap();
        assert_eq!(
            resp.report.as_deref(),
            Some("Spent the hour debugging the worklog pipeline.")
        );
        assert_eq!(resp.report_chars, Some(47));
    }

    #[tokio::test]
    async fn today_hour_with_no_row_is_empty() {
        let pool = pool_with_hours().await;
        let today = crate::date::today_string();
        let resp = get_hour_text(&pool, &today, "23").await.unwrap();
        assert!(resp.report.is_none());
        assert!(resp.report_chars.is_none());
    }

    #[tokio::test]
    async fn out_of_range_hour_is_empty() {
        let pool = pool_with_hours().await;
        let today = crate::date::today_string();
        let resp = get_hour_text(&pool, &today, "99").await.unwrap();
        assert!(resp.report.is_none());
    }
}
