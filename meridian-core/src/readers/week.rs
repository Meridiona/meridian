//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! `/api/week` ported to Rust — a faithful port of `ui/app/api/week/route.ts`.
//!
//! Last 7 local days; per day `SUM(duration_s) GROUP BY category` over the
//! FOREGROUND stream only (`claude_session_uuid IS NULL` — the coding-agent
//! overlay records the same wall-clock a second time and would double-count).
//!
//! NOTE: this route uses NAIVE local-date string bounds (`YYYY-MM-DDT00:00:00`)
//! compared as strings against the stored timestamps — NOT the UTC bounds that
//! `/api/today` uses. We replicate that exactly (don't "fix" it) so the output
//! matches byte-for-byte.

use crate::SqlitePool;
use chrono::{Datelike, Local, TimeZone, Utc};
use serde::Serialize;
use std::collections::BTreeMap;
use tracing::Instrument;

#[derive(Debug, Clone, Serialize)]
pub struct DaySummary {
    pub day: String,                 // weekday short, e.g. "Mon"
    pub date: String,                // "M/D", e.g. "6/16"
    pub total_s: i64,                // seconds
    pub cats: BTreeMap<String, f64>, // category → HOURS
    #[serde(rename = "isToday")]
    pub is_today: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct WeekResponse {
    pub days: Vec<DaySummary>,
    pub total_s: i64,
}

#[tracing::instrument(skip(pool))]
pub async fn get_week(pool: &SqlitePool, now_iso: &str) -> anyhow::Result<WeekResponse> {
    let now_ms =
        crate::intervals::parse_ms(now_iso).unwrap_or_else(|| Utc::now().timestamp_millis());

    let mut days: Vec<DaySummary> = Vec::with_capacity(7);
    for i in (0..=6).rev() {
        // `now - i*24h` then formatted in LOCAL tz — replicates the TS ms math
        // (so DST behaves identically rather than via calendar-day arithmetic).
        let day_ms = now_ms - i * 86_400_000;
        let day_local = Utc
            .timestamp_millis_opt(day_ms)
            .single()
            .unwrap_or_else(Utc::now)
            .with_timezone(&Local);
        let date_str = day_local.format("%Y-%m-%d").to_string();
        let dow = day_local.format("%a").to_string();
        let mmdd = format!("{}/{}", day_local.month(), day_local.day());
        let is_today = i == 0;

        // Naive local-string bounds, string-compared (matches the TS route).
        let start = format!("{date_str}T00:00:00");
        let end = format!("{date_str}T23:59:59.999");

        let rows: Vec<(Option<String>, Option<i64>)> =
            sqlx::query_as::<_, (Option<String>, Option<i64>)>(
                r#"
                SELECT category, SUM(duration_s) AS dur_s
                FROM app_sessions
                WHERE started_at >= ? AND started_at < ? AND claude_session_uuid IS NULL
                GROUP BY category
                "#,
            )
            .bind(&start)
            .bind(&end)
            .fetch_all(pool)
            .instrument(tracing::debug_span!("week.read.day", day = %date_str))
            .await?;

        let mut cats: BTreeMap<String, f64> = BTreeMap::new();
        let mut total_s: i64 = 0;
        for (cat, dur) in rows {
            let dur_s = dur.unwrap_or(0);
            let key = cat.unwrap_or_else(|| "null".to_string());
            *cats.entry(key).or_insert(0.0) += dur_s as f64 / 3600.0;
            total_s += dur_s;
        }

        // The live active session counts toward today (graceful: ignore errors).
        if is_today {
            match sqlx::query_as::<_, (String, Option<String>)>(
                r#"SELECT started_at, category FROM active_session WHERE id = 1"#,
            )
            .fetch_optional(pool)
            .instrument(tracing::debug_span!("week.read.active"))
            .await
            {
                Ok(Some((started_at, category))) => {
                    let elapsed = crate::intervals::parse_ms(&started_at)
                        .map(|s| (now_ms - s) / 1000)
                        .unwrap_or(0);
                    // Mirror the TS route's `|| 'idle_personal'`: map both NULL
                    // and empty-string to idle_personal so they don't land under
                    // a blank key in the cats map.
                    let cat = category
                        .filter(|s| !s.is_empty())
                        .unwrap_or_else(|| "idle_personal".to_string());
                    *cats.entry(cat).or_insert(0.0) += elapsed as f64 / 3600.0;
                    total_s += elapsed;
                }
                Ok(None) => {}
                Err(e) => tracing::warn!(
                    error = %e,
                    "week: active_session read failed, today excludes the live session"
                ),
            }
        }

        tracing::debug!(day = %date_str, total_s, cats = cats.len(), "week.read.day");
        days.push(DaySummary {
            day: dow,
            date: mmdd,
            total_s,
            cats,
            is_today,
        });
    }

    let total_s = days.iter().map(|d| d.total_s).sum();
    tracing::info!(days = days.len(), total_s, "week computed");
    Ok(WeekResponse { days, total_s })
}
