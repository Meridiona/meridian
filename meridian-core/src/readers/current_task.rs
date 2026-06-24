//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! The "current task" the menu-bar pill names — the live session carries no task
//! link, so this resolves one heuristically and computes the progress-ring fill.
//!
//! # What this is
//! Two things the tray's menu-bar item needs and that no existing reader exposes
//! in one shot:
//! - **Which task is "current"** — the `task_key` of today's *most recently
//!   classified* foreground task session (`task_session_type = 'task'`). Tracks
//!   what the dev just did; resets at the local day boundary.
//! - **A completion percentage** — `time-spent-today ÷ (story_points × 1 h)`,
//!   clamped to `[0, 1]`. Meridian stores no time-estimate, so we treat the
//!   ticket's `story_points` (a free-text field, migration 039) as an hour budget
//!   — a deliberate, rough convention. `None` when there are no usable points, so
//!   the caller can draw an un-filled ring rather than a fake number.
//!
//! # Who calls this
//! The tray poll loop (`poll::refresh::refresh_current_task`) → `AppState`
//! (`current_task_key` / `task_percent`) → the 1 s menu-bar ticker, which renders
//! the `◔ MER-142 · 2:05:11` pill.
//!
//! # Related
//! - [`crate::tasks`] — the full per-task payload (this is a narrow, cheap subset).
//! - [`crate::util::date::local_day_bounds`] — the day window both queries share.

use crate::util::date::local_day_bounds;
use crate::SqlitePool;
use anyhow::Context;
use serde::Serialize;
use tracing::Instrument;

/// The current task plus its progress-ring fill and tooltip details.
#[derive(Debug, Clone, Serialize)]
pub struct CurrentTask {
    /// The tracker key, e.g. `MER-142`.
    pub key: String,
    /// Ring fill in `[0.0, 1.0]`, or `None` when no usable story-point budget
    /// exists (the caller then draws an un-filled ring, not `0%`).
    pub percent: Option<f64>,
    /// Task title from `pm_tasks`. `None` when the task row is absent.
    pub title: Option<String>,
    /// Status category from `pm_tasks` (`"todo"`, `"in_progress"`, `"done"`, …).
    pub status_category: Option<String>,
    /// Priority string from `pm_tasks` (`"High"`, `"Medium"`, `"Low"`, …).
    pub priority: Option<String>,
    /// Seconds spent on this task today — the numerator of the progress %.
    pub spent_today_s: i64,
    /// Estimated duration in seconds (`story_points × 3600`), or `None` when
    /// no usable story-point budget exists.
    pub estimate_s: Option<i64>,
}

/// Resolve the current task for `today` (a local `YYYY-MM-DD`), or `None` when no
/// task session has been classified yet today. `today` is passed in so the fn
/// stays deterministic and timezone math lives in one place.
#[tracing::instrument(skip(pool))]
pub async fn get_current_task(
    pool: &SqlitePool,
    today: &str,
) -> anyhow::Result<Option<CurrentTask>> {
    let (day_start, day_end) = local_day_bounds(today);

    // Most recently classified task session today. `task_session_type = 'task'`
    // excludes overhead/untracked blocks so the pill names real work.
    let key: Option<String> = sqlx::query_scalar::<_, String>(
        r#"
        SELECT task_key
        FROM app_sessions
        WHERE task_key IS NOT NULL AND task_key != ''
          AND task_session_type = 'task'
          AND started_at >= ?1 AND started_at < ?2
        ORDER BY started_at DESC
        LIMIT 1
        "#,
    )
    .bind(&day_start)
    .bind(&day_end)
    .fetch_optional(pool)
    .instrument(tracing::debug_span!("current_task.read.recent_task"))
    .await
    .context("current_task: fetch most recent task session")?;
    tracing::debug!(found = key.is_some(), "current_task.read.recent_task");

    let key = match key {
        Some(k) => k,
        None => return Ok(None),
    };

    // Fetch title, status, priority, and story_points from pm_tasks in one shot.
    // Tolerates a DB that predates migration 039 (missing story_points column) —
    // any column error is silenced and treated as no-estimate / no-details.
    struct TaskRow {
        title: Option<String>,
        status_category: Option<String>,
        priority: Option<String>,
        story_points: Option<String>,
    }
    let task_row: Option<TaskRow> = match sqlx::query_as::<
        _,
        (
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
        ),
    >(
        "SELECT title, status_category, priority, story_points FROM pm_tasks WHERE task_key = ?1",
    )
    .bind(&key)
    .fetch_optional(pool)
    .instrument(tracing::debug_span!("current_task.read.task_detail"))
    .await
    {
        Ok(Some((title, status_category, priority, story_points))) => Some(TaskRow {
            title,
            status_category,
            priority,
            story_points,
        }),
        Ok(None) => None,
        Err(e) => {
            tracing::warn!(error = %e, task_key = %key, "current_task: task detail fetch failed");
            None
        }
    };

    // Foreground seconds spent on this task today.
    let spent_today_s: i64 = sqlx::query_scalar::<_, i64>(
        r#"
        SELECT COALESCE(SUM(duration_s), 0)
        FROM app_sessions
        WHERE task_key = ?1
          AND started_at >= ?2 AND started_at < ?3
        "#,
    )
    .bind(&key)
    .bind(&day_start)
    .bind(&day_end)
    .fetch_one(pool)
    .instrument(tracing::debug_span!("current_task.read.spent_today"))
    .await
    .context("current_task: sum today's seconds on task")?;

    let story_points = task_row.as_ref().and_then(|r| r.story_points.as_deref());
    let percent = compute_percent(spent_today_s, story_points);
    let estimate_s = story_points
        .and_then(|s| s.trim().parse::<f64>().ok())
        .filter(|&p| p > 0.0)
        .map(|p| (p * 3600.0).round() as i64);

    tracing::info!(key = %key, spent_today_s, ?percent, "current_task.resolved");

    Ok(Some(CurrentTask {
        key,
        percent,
        title: task_row.as_ref().and_then(|r| r.title.clone()),
        status_category: task_row.as_ref().and_then(|r| r.status_category.clone()),
        priority: task_row.as_ref().and_then(|r| r.priority.clone()),
        spent_today_s,
        estimate_s,
    }))
}

/// `spent_s ÷ (story_points × 3600)`, clamped to `[0, 1]`. `None` when the points
/// field is absent, non-numeric, or `≤ 0` — i.e. there is no honest budget to
/// divide against. Kept pure so the mapping is unit-testable without a DB.
fn compute_percent(spent_s: i64, story_points: Option<&str>) -> Option<f64> {
    let points: f64 = story_points?.trim().parse().ok()?;
    if points <= 0.0 {
        return None;
    }
    let budget_s = points * 3600.0;
    Some((spent_s as f64 / budget_s).clamp(0.0, 1.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_from_points() {
        // 2h5m on a 3-point (=3h) ticket ≈ 69%.
        let p = compute_percent(2 * 3600 + 5 * 60, Some("3")).unwrap();
        assert!((p - 0.6944).abs() < 0.001, "got {p}");
    }

    #[test]
    fn percent_clamps_at_full() {
        assert_eq!(compute_percent(10 * 3600, Some("3")), Some(1.0));
    }

    #[test]
    fn percent_none_without_budget() {
        assert_eq!(compute_percent(3600, None), None);
        assert_eq!(compute_percent(3600, Some("")), None);
        assert_eq!(compute_percent(3600, Some("none")), None);
        assert_eq!(compute_percent(3600, Some("0")), None);
    }
}
