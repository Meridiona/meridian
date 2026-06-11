//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Collect: assemble the SessionBundle for one (task, hour) window straight from
// meridian.db. Port of `pm_worklog_update/db.py::fetch_session_bundle`. Pure SQL
// read — no LLM. Only classified `task`-typed sessions are pulled; each carries
// the summary (or a capped text excerpt) the synth reasons over, plus the
// idle-discounted real_seconds that becomes the Jira worklog time.

use anyhow::{Context, Result};
use serde_json::Value;
use sqlx::{Row, SqlitePool};

use super::models::{SessionBundle, SessionDigest};

/// Per-session excerpt cap (chars) when no summary is present.
const EXCERPT_CAP_CHARS: usize = 2_000;
/// Heavy-path signal thresholds (informational; stored on the row).
const HEAVY_SESSION_COUNT: usize = 60;
const HEAVY_TEXT_BYTES: usize = 400_000;

/// Build the bundle for `task_key` over `[window_start_iso, window_end_iso)`.
/// Bounds must be `+00:00`-style ISO (matching the stored `started_at` format).
pub async fn fetch_session_bundle(
    pool: &SqlitePool,
    task_key: &str,
    window_start_iso: &str,
    window_end_iso: &str,
    cycle_index: i64,
    day_utc: &str,
) -> Result<SessionBundle> {
    // Ticket context.
    let task_row = sqlx::query(
        "SELECT title, status_category, assignee_name, \
                COALESCE(description_text, '') AS description_text \
         FROM pm_tasks WHERE task_key = ?",
    )
    .bind(task_key)
    .fetch_optional(pool)
    .await
    .context("fetch pm_task context")?;

    let (pm_task_title, pm_task_status, assignee_name, pm_task_description) = match &task_row {
        Some(r) => (
            r.try_get::<Option<String>, _>("title").ok().flatten(),
            r.try_get::<Option<String>, _>("status_category")
                .ok()
                .flatten(),
            r.try_get::<Option<String>, _>("assignee_name")
                .ok()
                .flatten(),
            r.try_get::<String, _>("description_text")
                .ok()
                .map(|d| d.chars().take(1000).collect::<String>()),
        ),
        None => (None, None, None, None),
    };

    // Classified task sessions in the window.
    let rows = sqlx::query(
        "SELECT id, app_name, started_at, ended_at, duration_s, \
                idle_frame_count, frame_count, window_titles, \
                session_text, session_text_source, category, session_summary \
         FROM app_sessions \
         WHERE task_key = ? \
           AND started_at >= ? \
           AND started_at <  ? \
           AND COALESCE(task_session_type, '') = 'task' \
         ORDER BY id ASC",
    )
    .bind(task_key)
    .bind(window_start_iso)
    .bind(window_end_iso)
    .fetch_all(pool)
    .await
    .context("fetch session rows for bundle")?;

    let mut digests: Vec<SessionDigest> = Vec::with_capacity(rows.len());
    let mut raw_bytes: usize = 0;
    let mut total_s: i64 = 0;
    let mut real_s: i64 = 0;

    for r in &rows {
        let id: i64 = r.get("id");
        let text: String = r
            .try_get::<Option<String>, _>("session_text")
            .ok()
            .flatten()
            .unwrap_or_default();
        let summary: String = r
            .try_get::<Option<String>, _>("session_summary")
            .ok()
            .flatten()
            .unwrap_or_default()
            .trim()
            .to_string();

        // Skip rows with neither summary nor text (mirrors Python).
        if summary.is_empty() && text.trim().is_empty() {
            continue;
        }

        let duration_s: i64 = r.try_get("duration_s").unwrap_or(0);
        let frame_count: i64 = r
            .try_get::<Option<i64>, _>("frame_count")
            .ok()
            .flatten()
            .unwrap_or(0);
        let idle_frame_count: i64 = r
            .try_get::<Option<i64>, _>("idle_frame_count")
            .ok()
            .flatten()
            .unwrap_or(0);
        let idle_share = if frame_count > 0 {
            idle_frame_count as f64 / frame_count as f64
        } else {
            0.0
        };
        let real_session_s = (duration_s as f64 * (1.0 - idle_share)).round() as i64;
        let idle_frame_s = (duration_s as f64 * idle_share).round() as i64;

        let (excerpt, text_source) = if !summary.is_empty() {
            (summary.clone(), Some("summary".to_string()))
        } else {
            (
                text.chars().take(EXCERPT_CAP_CHARS).collect::<String>(),
                r.try_get::<Option<String>, _>("session_text_source")
                    .ok()
                    .flatten(),
            )
        };

        let window_titles: Option<String> = r
            .try_get::<Option<String>, _>("window_titles")
            .ok()
            .flatten();
        let category: Option<String> = r.try_get::<Option<String>, _>("category").ok().flatten();

        digests.push(SessionDigest {
            id,
            app_name: r
                .try_get::<Option<String>, _>("app_name")
                .ok()
                .flatten()
                .unwrap_or_default(),
            started_at: r
                .try_get::<Option<String>, _>("started_at")
                .ok()
                .flatten()
                .unwrap_or_default(),
            ended_at: r
                .try_get::<Option<String>, _>("ended_at")
                .ok()
                .flatten()
                .unwrap_or_default(),
            duration_s,
            idle_frame_s,
            top_titles: parse_top_titles(window_titles.as_deref(), 3),
            dimensions: Default::default(), // unused by the synth prompt
            excerpt,
            category,
            text_source,
        });

        raw_bytes += text.len();
        total_s += duration_s;
        real_s += real_session_s;
    }

    let is_heavy = digests.len() > HEAVY_SESSION_COUNT || raw_bytes > HEAVY_TEXT_BYTES;

    Ok(SessionBundle {
        task_key: task_key.to_string(),
        window_start: window_start_iso.to_string(),
        window_end: window_end_iso.to_string(),
        cycle_index,
        sessions: digests,
        total_seconds: total_s,
        real_seconds: real_s,
        raw_text_bytes: raw_bytes as i64,
        is_heavy,
        pm_task_status,
        pm_task_title,
        pm_task_description,
        assignee_name,
        earlier_today_summaries: fetch_earlier_today_summaries(pool, task_key, day_utc).await?,
    })
}

/// Already-posted worklog summaries for this ticket today — fed to the synth as
/// "do not repeat" context.
pub async fn fetch_earlier_today_summaries(
    pool: &SqlitePool,
    task_key: &str,
    day_utc: &str,
) -> Result<Vec<String>> {
    let rows = sqlx::query(
        "SELECT payload_json FROM pm_worklogs \
         WHERE task_key = ? AND day_utc = ? AND state = 'posted' \
         ORDER BY cycle_index ASC",
    )
    .bind(task_key)
    .bind(day_utc)
    .fetch_all(pool)
    .await
    .context("fetch earlier-today worklog summaries")?;

    let mut out = Vec::new();
    for r in &rows {
        let payload: Option<String> = r.try_get("payload_json").ok();
        if let Some(p) = payload {
            if let Ok(v) = serde_json::from_str::<Value>(&p) {
                if let Some(s) = v.get("summary").and_then(Value::as_str) {
                    if !s.trim().is_empty() {
                        out.push(s.to_string());
                    }
                }
            }
        }
    }
    Ok(out)
}

/// Top-`n` window titles by count from the JSON `window_titles` column.
fn parse_top_titles(raw: Option<&str>, n: usize) -> Vec<String> {
    let Some(raw) = raw else { return Vec::new() };
    let Ok(Value::Array(items)) = serde_json::from_str::<Value>(raw) else {
        return Vec::new();
    };
    let mut titled: Vec<(&str, i64)> = items
        .iter()
        .filter_map(|it| {
            let title = it.get("title")?.as_str()?;
            let count = it.get("count").and_then(Value::as_i64).unwrap_or(0);
            Some((title, count))
        })
        .collect();
    titled.sort_by(|a, b| b.1.cmp(&a.1));
    titled
        .into_iter()
        .take(n)
        .map(|(t, _)| t.to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn top_titles_sorted_by_count_desc() {
        let raw = r#"[{"title":"a","count":2},{"title":"b","count":9},{"title":"c","count":5}]"#;
        assert_eq!(parse_top_titles(Some(raw), 2), vec!["b", "c"]);
    }

    #[test]
    fn top_titles_handles_garbage() {
        assert!(parse_top_titles(Some("not json"), 3).is_empty());
        assert!(parse_top_titles(None, 3).is_empty());
    }
}
