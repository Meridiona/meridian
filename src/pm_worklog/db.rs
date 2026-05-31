// meridian — normalises screenpipe activity into structured app sessions
//
// Worklog persistence — `pm_worklogs` + `pm_worklog_evidence`. Port of the write
// paths in `pm_worklog_update/db.py`. The UPSERT is keyed on
// (task_key, day_utc, cycle_index); only POSTED rows carry the worklog-window
// unique index, so a DRAFTED row can be replaced but a posted one is immutable —
// that, plus `find_existing_worklog`, is what makes restarts/backfills never
// double-post to Jira.

use anyhow::{Context, Result};
use sqlx::{Row, SqlitePool};

use super::models::{GroundedNarrative, JiraUpdate, UpdateState};

/// Insert or update the worklog row for (task, day, cycle); rewrite its evidence
/// rows. Returns the `pm_worklogs.id`.
pub async fn upsert_pm_worklog(
    pool: &SqlitePool,
    grounded: &GroundedNarrative,
    state: UpdateState,
    day_utc: &str,
    session_id_min: Option<i64>,
    session_id_max: Option<i64>,
) -> Result<i64> {
    let update = &grounded.update;
    let payload_json = serde_json::to_string(update).context("serialise JiraUpdate payload")?;

    let row = sqlx::query(
        "INSERT INTO pm_worklogs (\
             task_key, day_utc, cycle_index, window_start, window_end, \
             state, confidence, coverage, time_spent_seconds, \
             payload_json, session_id_min, session_id_max) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT (task_key, day_utc, cycle_index) DO UPDATE SET \
             state              = excluded.state, \
             confidence         = excluded.confidence, \
             coverage           = excluded.coverage, \
             time_spent_seconds = excluded.time_spent_seconds, \
             payload_json       = excluded.payload_json, \
             session_id_min     = excluded.session_id_min, \
             session_id_max     = excluded.session_id_max \
         RETURNING id",
    )
    .bind(&update.task_key)
    .bind(day_utc)
    .bind(update.cycle_index)
    .bind(&update.window_start)
    .bind(&update.window_end)
    .bind(state.as_str())
    .bind(update.confidence)
    .bind(grounded.coverage)
    .bind(update.time_spent_seconds)
    .bind(&payload_json)
    .bind(session_id_min)
    .bind(session_id_max)
    .fetch_one(pool)
    .await
    .context("upsert pm_worklogs row")?;

    let pm_worklog_id: i64 = row.get("id");

    // Rewrite evidence rows for this worklog.
    sqlx::query("DELETE FROM pm_worklog_evidence WHERE pm_worklog_id = ?")
        .bind(pm_worklog_id)
        .execute(pool)
        .await
        .context("clear stale evidence rows")?;

    for (kind, bullets) in update.bullet_groups() {
        for (idx, b) in bullets.iter().enumerate() {
            let excerpt: String = b.text.chars().take(400).collect();
            for session_id in &b.evidence_refs {
                sqlx::query(
                    "INSERT OR IGNORE INTO pm_worklog_evidence \
                     (pm_worklog_id, bullet_kind, bullet_index, session_id, excerpt) \
                     VALUES (?, ?, ?, ?, ?)",
                )
                .bind(pm_worklog_id)
                .bind(kind)
                .bind(idx as i64)
                .bind(session_id)
                .bind(&excerpt)
                .execute(pool)
                .await
                .context("insert evidence row")?;
            }
        }
    }

    Ok(pm_worklog_id)
}

/// Stamp a row POSTED with the Jira worklog id (sets `posted_at` once).
pub async fn mark_worklog_posted(
    pool: &SqlitePool,
    pm_worklog_id: i64,
    posted_worklog_id: &str,
) -> Result<()> {
    sqlx::query(
        "UPDATE pm_worklogs \
         SET state = 'posted', posted_worklog_id = ?, \
             posted_at = COALESCE(posted_at, strftime('%Y-%m-%dT%H:%M:%SZ', 'now')) \
         WHERE id = ?",
    )
    .bind(posted_worklog_id)
    .bind(pm_worklog_id)
    .execute(pool)
    .await
    .context("mark worklog posted")?;
    Ok(())
}

/// If a worklog has already been POSTED for this exact (task, window), return
/// its (row id, jira worklog id) so the caller short-circuits instead of posting
/// again. This is the idempotency backstop against double-posting.
pub async fn find_existing_worklog(
    pool: &SqlitePool,
    task_key: &str,
    window_start: &str,
    window_end: &str,
) -> Result<Option<(i64, String)>> {
    let row = sqlx::query(
        "SELECT id, posted_worklog_id FROM pm_worklogs \
         WHERE task_key = ? AND window_start = ? AND window_end = ? \
           AND posted_worklog_id IS NOT NULL \
         LIMIT 1",
    )
    .bind(task_key)
    .bind(window_start)
    .bind(window_end)
    .fetch_optional(pool)
    .await
    .context("find existing posted worklog")?;

    Ok(row.map(|r| {
        let id: i64 = r.get("id");
        let wid: String = r.get("posted_worklog_id");
        (id, wid)
    }))
}

/// Convenience: serialise the payload for logging/dry-run.
pub fn payload_preview(update: &JiraUpdate) -> String {
    serde_json::to_string_pretty(update).unwrap_or_else(|_| "<unserialisable>".to_string())
}
