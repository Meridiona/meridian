// meridian — normalises screenpipe activity into structured app sessions

use std::collections::HashMap;

use anyhow::{Context, Result};
use chrono::Utc;
use sqlx::SqlitePool;

use super::SessionClassification;

/// Mark a trivial (empty session_text) session as overhead/skip without calling the LLM.
pub(super) async fn update_session_overhead(pool: &SqlitePool, session_id: i64) -> Result<()> {
    sqlx::query(
        "UPDATE app_sessions
         SET task_key = NULL, task_confidence = 0.0, task_routing = 'skip',
             task_method = 'prefilter_trivial', task_reasoning = NULL,
             task_session_type = 'overhead'
         WHERE id = ?",
    )
    .bind(session_id)
    .execute(pool)
    .await
    .with_context(|| format!("marking session {} as overhead", session_id))?;
    Ok(())
}

/// Persist a hermes classification result into `app_sessions`.
pub(super) async fn update_session_task(
    pool: &SqlitePool,
    r: &SessionClassification,
) -> Result<()> {
    let reasoning = if r.reasoning.is_empty() {
        None
    } else {
        Some(&r.reasoning)
    };
    sqlx::query(
        "UPDATE app_sessions
         SET task_key = ?, task_confidence = ?, task_routing = ?,
             task_method = ?, task_reasoning = ?, task_session_type = ?
         WHERE id = ?",
    )
    .bind(&r.task_key)
    .bind(r.confidence)
    .bind(&r.routing)
    .bind(&r.method)
    .bind(reasoning)
    .bind(&r.session_type)
    .bind(r.session_id)
    .execute(pool)
    .await
    .with_context(|| format!("updating task classification for session {}", r.session_id))?;
    Ok(())
}

/// Persist multi-label dimension tags returned by Python.
pub(super) async fn write_dimensions(
    pool: &SqlitePool,
    session_id: i64,
    dims: &HashMap<String, Vec<String>>,
) -> Result<()> {
    for (dimension, values) in dims {
        for value in values {
            sqlx::query(
                "INSERT OR IGNORE INTO session_dimensions \
                 (session_id, dimension, value, confidence, source) \
                 VALUES (?, ?, ?, 0.75, 'hermes_standalone')",
            )
            .bind(session_id)
            .bind(dimension)
            .bind(value)
            .execute(pool)
            .await
            .with_context(|| {
                format!(
                    "writing dimension {}={} for session {}",
                    dimension, value, session_id
                )
            })?;
        }
    }
    Ok(())
}

/// Advance the cursor monotonically — only updates when `session_id` is strictly
/// greater than the stored value so out-of-order writes are safe.
pub(super) async fn advance_agent_cursor(pool: &SqlitePool, session_id: i64) -> Result<()> {
    let now = Utc::now().to_rfc3339();
    sqlx::query(
        "UPDATE agent_cursor SET last_session_id = ?, updated_at = ? \
         WHERE id = 1 AND ? > last_session_id",
    )
    .bind(session_id)
    .bind(&now)
    .bind(session_id)
    .execute(pool)
    .await
    .with_context(|| format!("advancing agent_cursor to {}", session_id))?;
    Ok(())
}

/// Insert an `agent_runs` row with `status = 'running'` and return its id.
pub(super) async fn start_agent_run(pool: &SqlitePool) -> Result<i64> {
    let now = Utc::now().to_rfc3339();
    let row = sqlx::query_as::<_, (i64,)>(
        "INSERT INTO agent_runs (started_at, status) VALUES (?, 'running') RETURNING id",
    )
    .bind(&now)
    .fetch_one(pool)
    .await
    .context("inserting agent_run row")?;
    Ok(row.0)
}

/// Mark an `agent_runs` row as finished.
pub(super) async fn complete_agent_run(
    pool: &SqlitePool,
    run_id: i64,
    status: &str,
    sessions: i64,
    links: i64,
) -> Result<()> {
    let now = Utc::now().to_rfc3339();
    sqlx::query(
        "UPDATE agent_runs \
         SET finished_at = ?, status = ?, sessions_processed = ?, links_written = ? \
         WHERE id = ?",
    )
    .bind(&now)
    .bind(status)
    .bind(sessions)
    .bind(links)
    .bind(run_id)
    .execute(pool)
    .await
    .with_context(|| format!("completing agent_run {}", run_id))?;
    Ok(())
}
