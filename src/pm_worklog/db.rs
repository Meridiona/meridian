//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
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

    // The DO UPDATE guard is the safety net for the human-in-the-loop flow: once
    // a row is `approved` or `posted`, a later driver re-run (backfill, aging
    // retry, manual `meridian pm-worklog --day …`) MUST NOT overwrite the user's
    // edit/approval back to a fresh draft. When the guard blocks the update the
    // conflict becomes a no-op and `RETURNING` yields nothing, so we fall back to
    // reading the existing id.
    // `provider` is snapshotted from pm_tasks so the poster can route this
    // worklog to the right backend even if the ticket is later pruned. Falls
    // back to 'jira' for safety (the only provider before multi-provider support).
    let row = sqlx::query(
        "INSERT INTO pm_worklogs (\
             task_key, day_utc, cycle_index, window_start, window_end, \
             state, confidence, coverage, time_spent_seconds, \
             payload_json, session_id_min, session_id_max, provider) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, \
                 COALESCE((SELECT provider FROM pm_tasks WHERE task_key = ?), 'jira')) \
         ON CONFLICT (task_key, day_utc, cycle_index) DO UPDATE SET \
             state              = excluded.state, \
             confidence         = excluded.confidence, \
             coverage           = excluded.coverage, \
             time_spent_seconds = excluded.time_spent_seconds, \
             payload_json       = excluded.payload_json, \
             session_id_min     = excluded.session_id_min, \
             session_id_max     = excluded.session_id_max, \
             provider           = excluded.provider \
         WHERE pm_worklogs.state NOT IN ('approved', 'posted') \
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
    .bind(&update.task_key)
    .fetch_optional(pool)
    .await
    .context("upsert pm_worklogs row")?;

    let pm_worklog_id: i64 = match row {
        Some(r) => r.get("id"),
        None => {
            // Guard blocked the update — the row is approved/posted and immutable.
            // Return its id without touching it (and skip the evidence rewrite).
            let existing = sqlx::query(
                "SELECT id FROM pm_worklogs \
                 WHERE task_key = ? AND day_utc = ? AND cycle_index = ?",
            )
            .bind(&update.task_key)
            .bind(day_utc)
            .bind(update.cycle_index)
            .fetch_one(pool)
            .await
            .context("read existing approved/posted worklog id")?;
            let id: i64 = existing.get("id");
            tracing::debug!(
                pm_worklog_id = id,
                task_key = %update.task_key,
                "worklog already approved/posted — draft re-run left it untouched"
            );
            return Ok(id);
        }
    };

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
         SET state = 'posted', posted_worklog_id = ?, last_post_error = NULL, \
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

/// One worklog the user has approved in the dashboard, ready to post to Jira.
/// `comment` is the (possibly user-edited) summary lifted from `payload_json`.
#[derive(Debug, Clone)]
pub struct ApprovedWorklog {
    pub id: i64,
    pub task_key: String,
    pub window_start: String,
    pub window_end: String,
    pub time_spent_seconds: i64,
    pub comment: String,
    /// Which tracker this worklog posts to ('jira' | 'github' | 'linear').
    pub provider: String,
    /// Lifecycle timestamps (stored UTC ISO) — surfaced on the `worklog_post`
    /// span so the dashboard shows when each worklog was drafted / approved.
    pub created_at: Option<String>,
    pub approved_at: Option<String>,
    pub post_attempt_count: i64,
}

/// All worklogs awaiting a post (`state = 'approved'`), oldest window first.
/// The summary text is read straight from the stored payload so any UI edit is
/// honoured. Rows with an empty summary are skipped — there is nothing to post.
pub async fn fetch_approved_worklogs(pool: &SqlitePool) -> Result<Vec<ApprovedWorklog>> {
    let rows = sqlx::query(
        "SELECT id, task_key, window_start, window_end, time_spent_seconds, payload_json, \
                COALESCE(provider, 'jira') AS provider, created_at, approved_at, \
                COALESCE(post_attempt_count, 0) AS post_attempt_count \
         FROM pm_worklogs WHERE state = 'approved' ORDER BY window_start, task_key",
    )
    .fetch_all(pool)
    .await
    .context("fetch approved worklogs")?;

    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        let payload: String = r.get("payload_json");
        let comment = serde_json::from_str::<JiraUpdate>(&payload)
            .map(|u| u.summary)
            .unwrap_or_default();
        out.push(ApprovedWorklog {
            id: r.get("id"),
            task_key: r.get("task_key"),
            window_start: r.get("window_start"),
            window_end: r.get("window_end"),
            time_spent_seconds: r.try_get("time_spent_seconds").unwrap_or(0),
            comment,
            provider: r.try_get("provider").unwrap_or_else(|_| "jira".to_string()),
            created_at: r.try_get("created_at").ok(),
            approved_at: r.try_get("approved_at").ok(),
            post_attempt_count: r.try_get("post_attempt_count").unwrap_or(0),
        });
    }
    Ok(out)
}

/// Record a TRANSIENT failed post attempt (network/5xx): bump the counter and
/// stash the error for the UI. The row stays `approved` so the next sweep retries.
pub async fn mark_post_failed(pool: &SqlitePool, pm_worklog_id: i64, error: &str) -> Result<()> {
    let truncated: String = error.chars().take(500).collect();
    sqlx::query(
        "UPDATE pm_worklogs \
         SET post_attempt_count = post_attempt_count + 1, last_post_error = ? \
         WHERE id = ?",
    )
    .bind(&truncated)
    .bind(pm_worklog_id)
    .execute(pool)
    .await
    .context("mark worklog post failed")?;
    Ok(())
}

/// Record a TERMINAL post failure (empty comment, below Jira's minimum): flip the
/// row to `failed` so the sweep stops retrying. The user can edit + re-approve or
/// dismiss it in the dashboard.
pub async fn fail_worklog(pool: &SqlitePool, pm_worklog_id: i64, error: &str) -> Result<()> {
    let truncated: String = error.chars().take(500).collect();
    sqlx::query(
        "UPDATE pm_worklogs \
         SET state = 'failed', last_post_error = ?, \
             post_attempt_count = post_attempt_count + 1 \
         WHERE id = ?",
    )
    .bind(&truncated)
    .bind(pm_worklog_id)
    .execute(pool)
    .await
    .context("mark worklog terminally failed")?;
    Ok(())
}
