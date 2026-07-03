//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Per-hour generation/pause status for the timeline's hour-row badges.
//!
//! # What this is
//! For every local hour (0..24) of `day`: whether that hour's worklog is
//! actively being generated right now (`pm_worklog_hours.status = 'generating'`,
//! flipped live by `src/worklog_pipeline.rs::process_hour` just before it calls
//! `/worklog_hour`, reverted to `pending` on failure) and whether tracking was
//! paused at any point during that hour (`gaps` rows of kind `tracking_paused` /
//! `schedule_paused`, migration 051 — written by the tray's pause/resume + the
//! schedule-pause poll tick). No route to port — new backend work.
//!
//! # Who calls this
//! The tray `get_hour_status` command → `TimelineColumn`'s per-hour badges
//! (paired with the live `get_daemon_status` for "is tracking paused right now").
//!
//! # Related
//! - [`crate::worklogs`] — the hour's actual worklog content.
//! - [`crate::hour_text`] — the sibling per-hour reader; shares the same
//!   `hour_start` key construction (`local_day_bounds`-adjacent, but per-hour).
//! - `src/pm_worklog/ledger.rs` (daemon) — writes `pm_worklog_hours.status`.

use crate::SqlitePool;
use chrono::{DateTime, Duration, Local, NaiveDate, TimeZone, Timelike, Utc};
use serde::Serialize;
use sqlx::FromRow;
use std::collections::HashSet;
use tracing::Instrument;

/// One local hour's generation/pause state.
#[derive(Debug, Clone, Serialize)]
pub struct HourStatus {
    /// Local hour, `0..24`.
    pub hour: i64,
    /// This hour's worklog is being generated right now.
    pub generating: bool,
    /// Tracking was paused (manually or on schedule) at some point during this
    /// hour — a historical marker, independent of whether tracking is paused
    /// *right now* (the caller combines this with the live daemon status for
    /// the current hour).
    pub paused: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct HourStatusResponse {
    pub day: String,
    pub hours: Vec<HourStatus>,
}

/// Local midnight of `day` as a UTC instant, mirroring
/// `hour_text::hour_start_key`'s local→UTC construction. `None` for an
/// unparseable date or a nonexistent local wall-clock (DST spring-forward).
fn local_midnight_utc(day: &str) -> Option<DateTime<Utc>> {
    let date = NaiveDate::parse_from_str(day, "%Y-%m-%d").ok()?;
    let midnight_naive = date.and_hms_opt(0, 0, 0)?;
    Some(
        Local
            .from_local_datetime(&midnight_naive)
            .single()?
            .with_timezone(&Utc),
    )
}

#[derive(FromRow)]
struct WorklogHourRow {
    hour_start: String,
    status: String,
}

#[derive(FromRow)]
struct GapRow {
    started_at: String,
    ended_at: String,
}

/// Read the generating/paused state of every local hour of `day`.
///
/// Degrades gracefully (all-false rows) rather than erroring on a missing
/// table or an unparseable `day` — this feeds a purely cosmetic badge, never
/// worth failing the timeline load over.
#[tracing::instrument(skip(pool))]
pub async fn get_hour_status(pool: &SqlitePool, day: &str) -> anyhow::Result<HourStatusResponse> {
    let Some(midnight_utc) = local_midnight_utc(day) else {
        tracing::warn!(day, "hour_status: unparseable day — empty response");
        return Ok(HourStatusResponse {
            day: day.to_string(),
            hours: (0..24).map(empty_hour).collect(),
        });
    };

    // hour_start keys for this day's 24 local hours, in the same UTC-instant
    // form the ledger keys rows on.
    let hour_starts: Vec<String> = (0..24)
        .map(|h| {
            (midnight_utc + Duration::hours(h))
                .format("%Y-%m-%dT%H:%M:%S+00:00")
                .to_string()
        })
        .collect();
    let day_end_utc = midnight_utc + Duration::days(1);

    let placeholders = vec!["?"; hour_starts.len()].join(",");
    let rows_result = sqlx::query_as::<_, WorklogHourRow>(&format!(
        "SELECT hour_start, status FROM pm_worklog_hours WHERE hour_start IN ({placeholders})"
    ))
    .bind_all(&hour_starts)
    .fetch_all(pool)
    .instrument(tracing::debug_span!("hour_status.read.pm_worklog_hours"))
    .await;

    let mut generating: HashSet<i64> = HashSet::new();
    match rows_result {
        Ok(rows) => {
            for r in rows {
                if r.status == "generating" {
                    if let Some(h) = hour_starts.iter().position(|hs| hs == &r.hour_start) {
                        generating.insert(h as i64);
                    }
                }
            }
        }
        Err(e) => tracing::warn!(error = %e, "hour_status: pm_worklog_hours read skipped"),
    }

    let gaps_result = sqlx::query_as::<_, GapRow>(
        "SELECT started_at, ended_at FROM gaps \
         WHERE kind IN ('tracking_paused', 'schedule_paused') \
           AND started_at < ? AND ended_at > ?",
    )
    .bind(day_end_utc.to_rfc3339())
    .bind(midnight_utc.to_rfc3339())
    .fetch_all(pool)
    .instrument(tracing::debug_span!("hour_status.read.gaps"))
    .await;

    let mut paused: HashSet<i64> = HashSet::new();
    match gaps_result {
        Ok(gaps) => {
            for g in gaps {
                mark_paused_hours(
                    &g.started_at,
                    &g.ended_at,
                    midnight_utc,
                    day_end_utc,
                    &mut paused,
                );
            }
        }
        Err(e) => tracing::warn!(error = %e, "hour_status: gaps read skipped"),
    }

    let hours = (0..24)
        .map(|h| HourStatus {
            hour: h,
            generating: generating.contains(&h),
            paused: paused.contains(&h),
        })
        .collect();

    tracing::info!(
        day,
        generating = generating.len(),
        paused = paused.len(),
        "hour_status computed"
    );
    Ok(HourStatusResponse {
        day: day.to_string(),
        hours,
    })
}

fn empty_hour(h: i64) -> HourStatus {
    HourStatus {
        hour: h,
        generating: false,
        paused: false,
    }
}

/// Mark every local hour of `[midnight_utc, day_end_utc)` that a `[started_at,
/// ended_at)` gap overlaps. Both gap bounds are clamped to the day's window
/// before bucketing, so a pause spanning midnight only marks the hours that
/// actually fall on this local day — which also guarantees the clamped
/// interval never crosses a local calendar day, so `start_hour..=end_hour` is
/// always ascending and cannot wrap.
fn mark_paused_hours(
    started_at: &str,
    ended_at: &str,
    midnight_utc: DateTime<Utc>,
    day_end_utc: DateTime<Utc>,
    out: &mut HashSet<i64>,
) {
    let (Ok(s), Ok(e)) = (
        DateTime::parse_from_rfc3339(started_at),
        DateTime::parse_from_rfc3339(ended_at),
    ) else {
        return;
    };
    let start = s.with_timezone(&Utc).max(midnight_utc);
    // `ended_at` is exclusive; step back 1ns so a gap ending exactly on an hour
    // boundary doesn't spuriously mark the next hour.
    let end_inclusive = e.with_timezone(&Utc).min(day_end_utc) - Duration::nanoseconds(1);
    if end_inclusive < start {
        return;
    }
    let start_hour = start.with_timezone(&Local).hour() as i64;
    let end_hour = end_inclusive.with_timezone(&Local).hour() as i64;
    out.extend(start_hour..=end_hour);
}

/// Bind a slice of strings to a query's positional `?` placeholders — sqlx has
/// no native `IN (...)` binder, so this is a small extension trait instead of
/// repeating `.bind()` 24 times inline.
trait BindAll<'q> {
    fn bind_all(self, values: &'q [String]) -> Self;
}

impl<'q> BindAll<'q>
    for sqlx::query::QueryAs<'q, sqlx::Sqlite, WorklogHourRow, sqlx::sqlite::SqliteArguments<'q>>
{
    fn bind_all(mut self, values: &'q [String]) -> Self {
        for v in values {
            self = self.bind(v);
        }
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;

    async fn pool_with_tables() -> SqlitePool {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE pm_worklog_hours (hour_start TEXT PRIMARY KEY, day_utc TEXT, \
                hour_end TEXT, status TEXT, task_count INTEGER, processed_at TEXT)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "CREATE TABLE gaps (id INTEGER PRIMARY KEY AUTOINCREMENT, started_at TEXT, \
                ended_at TEXT, duration_s INTEGER, kind TEXT, etl_run_id INTEGER)",
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    #[tokio::test]
    async fn generating_hour_flagged() {
        let pool = pool_with_tables().await;
        let today = crate::date::today_string();
        let midnight = local_midnight_utc(&today).unwrap();
        let hs9 = (midnight + Duration::hours(9))
            .format("%Y-%m-%dT%H:%M:%S+00:00")
            .to_string();
        sqlx::query(
            "INSERT INTO pm_worklog_hours (hour_start, day_utc, hour_end, status, task_count) \
             VALUES (?, ?, '', 'generating', 0)",
        )
        .bind(&hs9)
        .bind(&today)
        .execute(&pool)
        .await
        .unwrap();

        let resp = get_hour_status(&pool, &today).await.unwrap();
        assert!(resp.hours.iter().find(|h| h.hour == 9).unwrap().generating);
        assert!(!resp.hours.iter().find(|h| h.hour == 10).unwrap().generating);
    }

    #[tokio::test]
    async fn pause_gap_marks_overlapping_hours() {
        let pool = pool_with_tables().await;
        let today = crate::date::today_string();
        let midnight = local_midnight_utc(&today).unwrap();
        // A pause from local hour 14:30 to 15:10 should mark both hour 14 and 15.
        let started = (midnight + Duration::hours(14) + Duration::minutes(30)).to_rfc3339();
        let ended = (midnight + Duration::hours(15) + Duration::minutes(10)).to_rfc3339();
        sqlx::query(
            "INSERT INTO gaps (started_at, ended_at, duration_s, kind) VALUES (?, ?, 2400, 'tracking_paused')",
        )
        .bind(&started)
        .bind(&ended)
        .execute(&pool)
        .await
        .unwrap();

        let resp = get_hour_status(&pool, &today).await.unwrap();
        assert!(resp.hours.iter().find(|h| h.hour == 14).unwrap().paused);
        assert!(resp.hours.iter().find(|h| h.hour == 15).unwrap().paused);
        assert!(!resp.hours.iter().find(|h| h.hour == 13).unwrap().paused);
        assert!(!resp.hours.iter().find(|h| h.hour == 16).unwrap().paused);
    }

    #[tokio::test]
    async fn unrelated_gap_kind_ignored() {
        let pool = pool_with_tables().await;
        let today = crate::date::today_string();
        let midnight = local_midnight_utc(&today).unwrap();
        let started = (midnight + Duration::hours(5)).to_rfc3339();
        let ended = (midnight + Duration::hours(6)).to_rfc3339();
        sqlx::query(
            "INSERT INTO gaps (started_at, ended_at, duration_s, kind) VALUES (?, ?, 3600, 'system_sleep')",
        )
        .bind(&started)
        .bind(&ended)
        .execute(&pool)
        .await
        .unwrap();

        let resp = get_hour_status(&pool, &today).await.unwrap();
        assert!(!resp.hours.iter().any(|h| h.paused));
    }
}
