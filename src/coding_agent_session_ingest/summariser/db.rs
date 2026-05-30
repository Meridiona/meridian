// meridian — normalises screenpipe activity into structured app sessions
//
// SQLite layer for the summariser — read the queue, write the summary. The
// single write path is idempotent (`UPDATE ... WHERE session_summary IS NULL`),
// so retries / concurrent runs can never double-write. Port of
// the former Python summariser/db.py.

use anyhow::{Context, Result};
use sqlx::SqlitePool;

use super::config::SummariserConfig;

/// task_method the indexer sets on a sealed row awaiting summary.
pub const TASK_METHOD_PENDING: &str = "pending_summariser";
/// task_method we set after summarising — the classifier's queue (P3). NOT the
/// Python terminal 'summarised': summarising is no longer the end of the line.
pub const TASK_METHOD_PENDING_CLASSIFIER: &str = "pending_classifier";

/// A sealed coding-agent segment awaiting a summary (metadata only;
/// `session_text` is fetched separately, one at a time, to keep memory flat).
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PendingRow {
    pub id: i64,
    #[sqlx(rename = "claude_session_uuid")]
    pub session_uuid: String,
    #[sqlx(rename = "app_name")]
    pub agent: String,
    pub segment_started_at: String,
    pub started_at: String,
    pub ended_at: String,
    pub duration_s: i64,
}

/// Idempotently add the `summary_source` column to app_sessions.
///
/// app_sessions is migration-owned, but `summary_source` was historically added
/// out-of-band (the Python summariser's `ensure_schema`), so the live DB already
/// has it while a freshly-migrated DB does not — a static ADD COLUMN migration
/// would fail on the live DB. This runtime guard works for both. Safe to call on
/// every startup / before tests.
pub async fn ensure_summary_source_column(pool: &SqlitePool) -> Result<()> {
    let has: bool = sqlx::query_scalar(
        "SELECT COUNT(*) FROM pragma_table_info('app_sessions') WHERE name = 'summary_source'",
    )
    .fetch_one(pool)
    .await
    .map(|n: i64| n > 0)
    .context("check summary_source column")?;
    if !has {
        sqlx::query("ALTER TABLE app_sessions ADD COLUMN summary_source TEXT")
            .execute(pool)
            .await
            .context("add summary_source column")?;
        tracing::info!("added summary_source column to app_sessions");
    }
    Ok(())
}

const ROW_COLS: &str = "id, claude_session_uuid, app_name, segment_started_at, \
                        started_at, ended_at, duration_s";

/// Sealed coding segments needing a summary, oldest-ended first. `day`
/// (`YYYY-MM-DD`, matched against `substr(started_at,1,10)`) scopes the queue to
/// one calendar day so a tick never drains all history at once. Oldest-first so
/// a session's earlier bursts summarise before later ones (prior-burst context).
pub async fn fetch_pending(
    pool: &SqlitePool,
    cfg: &SummariserConfig,
    limit: i64,
    day: Option<&str>,
) -> Result<Vec<PendingRow>> {
    let day_clause = if day.is_some() {
        "AND substr(started_at, 1, 10) = ?"
    } else {
        ""
    };
    let sql = format!(
        "SELECT {cols}
         FROM   app_sessions
         WHERE  claude_session_uuid IS NOT NULL
           AND  sealed_at IS NOT NULL
           AND  task_method = ?
           AND  session_summary IS NULL
           AND  session_text IS NOT NULL
           AND  session_text <> ''
           AND  frame_count >= ?
           AND  length(session_text) >= ?
           {day}
         ORDER BY ended_at ASC
         LIMIT ?",
        cols = ROW_COLS,
        day = day_clause,
    );

    let mut q = sqlx::query_as::<_, PendingRow>(&sql)
        .bind(TASK_METHOD_PENDING)
        .bind(cfg.min_turns)
        .bind(cfg.min_text_bytes);
    if let Some(d) = day {
        q = q.bind(d);
    }
    q = q.bind(limit);
    q.fetch_all(pool).await.context("fetch pending summaries")
}

/// Full `session_text` for one row (fetched per-row to bound memory).
pub async fn fetch_transcript(pool: &SqlitePool, row_id: i64) -> Result<String> {
    let text: Option<String> =
        sqlx::query_scalar("SELECT session_text FROM app_sessions WHERE id = ?")
            .bind(row_id)
            .fetch_optional(pool)
            .await
            .context("fetch transcript")?
            .flatten();
    Ok(text.unwrap_or_default())
}

/// The summary of this session's most recent EARLIER burst, if any — passed to
/// the model as continuation context so a resumed session reads as one story.
pub async fn fetch_prior_summary(
    pool: &SqlitePool,
    session_uuid: &str,
    segment_started_at: &str,
) -> Result<Option<String>> {
    let s: Option<String> = sqlx::query_scalar(
        "SELECT session_summary FROM app_sessions
         WHERE claude_session_uuid = ?
           AND segment_started_at < ?
           AND session_summary IS NOT NULL
         ORDER BY segment_started_at DESC
         LIMIT 1",
    )
    .bind(session_uuid)
    .bind(segment_started_at)
    .fetch_optional(pool)
    .await
    .context("fetch prior summary")?
    .flatten();
    Ok(s.filter(|x| !x.is_empty()))
}

/// Persist summary + engine source + flip task_method to the classifier queue.
/// Idempotent: returns true if this call wrote the row, false if already
/// summarised (another worker / retry won the race).
pub async fn write_summary(
    pool: &SqlitePool,
    row_id: i64,
    summary: &str,
    source: &str,
) -> Result<bool> {
    let res = sqlx::query(
        "UPDATE app_sessions
         SET    session_summary = ?, task_method = ?, summary_source = ?
         WHERE  id = ? AND session_summary IS NULL",
    )
    .bind(summary)
    .bind(TASK_METHOD_PENDING_CLASSIFIER)
    .bind(source)
    .bind(row_id)
    .execute(pool)
    .await
    .context("write summary")?;
    Ok(res.rows_affected() > 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coding_agent_session_ingest::db as cdb;
    use crate::coding_agent_session_ingest::segment::Segment;
    use crate::coding_agent_session_ingest::summariser::config::SummariserConfig;
    use sqlx::sqlite::SqliteConnectOptions;
    use std::str::FromStr;

    async fn fresh_db() -> SqlitePool {
        let opts = SqliteConnectOptions::from_str("sqlite::memory:")
            .unwrap()
            .create_if_missing(true);
        let pool = SqlitePool::connect_with(opts).await.unwrap();
        sqlx::migrate!("src/migrations").run(&pool).await.unwrap();
        ensure_summary_source_column(&pool).await.unwrap();
        pool
    }

    /// A sealed, pending, summary-worthy segment (clears MIN_TURNS + MIN_TEXT_BYTES).
    fn pending_seg(uuid: &str, seg_start: &str, ended: &str) -> Segment {
        Segment {
            session_uuid: uuid.into(),
            agent: "claude_code".into(),
            cwd: Some("/repo".into()),
            segment_started_at: seg_start.into(),
            started_at: seg_start.into(),
            ended_at: ended.into(),
            user_turns: 2,
            assistant_turns: 2,
            active_seconds: 300,
            transcript: "x".repeat(900), // > MIN_TEXT_BYTES
            is_last: false,
        }
    }

    #[tokio::test]
    async fn fetch_pending_then_write_flips_to_classifier_queue() {
        let pool = fresh_db().await;
        let cfg = SummariserConfig::from_env();
        let s = pending_seg(
            "u1",
            "2026-05-20T08:00:00.000000+00:00",
            "2026-05-20T08:30:00.000000+00:00",
        );
        let id = cdb::upsert_segment(&pool, &s, true, Some("2026-05-20T09:00:00.000000+00:00"))
            .await
            .unwrap()
            .unwrap();

        // Appears in the queue (day-scoped to its started_at date).
        let rows = fetch_pending(&pool, &cfg, 10, Some("2026-05-20"))
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, id);
        assert_eq!(rows[0].agent, "Claude Code"); // app_name

        // Write summary → row leaves the summariser queue for the classifier queue.
        assert!(write_summary(&pool, id, "did the work", "claude")
            .await
            .unwrap());
        let (method, src, summ): (String, Option<String>, Option<String>) = sqlx::query_as(
            "SELECT task_method, summary_source, session_summary FROM app_sessions WHERE id = ?",
        )
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(method, TASK_METHOD_PENDING_CLASSIFIER);
        assert_eq!(src.as_deref(), Some("claude"));
        assert_eq!(summ.as_deref(), Some("did the work"));

        // No longer pending; second write is a no-op (idempotent).
        assert!(fetch_pending(&pool, &cfg, 10, None)
            .await
            .unwrap()
            .is_empty());
        assert!(!write_summary(&pool, id, "again", "mlx").await.unwrap());
    }

    #[tokio::test]
    async fn fetch_pending_excludes_live_and_thin_rows() {
        let pool = fresh_db().await;
        let cfg = SummariserConfig::from_env();

        // Live (unsealed) row → not in queue.
        let live = pending_seg(
            "live",
            "2026-05-20T08:00:00.000000+00:00",
            "2026-05-20T08:30:00.000000+00:00",
        );
        cdb::upsert_segment(&pool, &live, false, None)
            .await
            .unwrap();

        // Sealed but too little text → not in queue.
        let mut thin = pending_seg(
            "thin",
            "2026-05-20T10:00:00.000000+00:00",
            "2026-05-20T10:05:00.000000+00:00",
        );
        thin.transcript = "tiny".into();
        cdb::upsert_segment(&pool, &thin, true, Some("2026-05-20T11:00:00.000000+00:00"))
            .await
            .unwrap();

        assert!(fetch_pending(&pool, &cfg, 10, None)
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn prior_summary_is_the_latest_earlier_burst() {
        let pool = fresh_db().await;
        // Seg A (earlier) summarised, Seg B (later) not yet.
        let a = pending_seg(
            "s",
            "2026-05-20T08:00:00.000000+00:00",
            "2026-05-20T08:30:00.000000+00:00",
        );
        let aid = cdb::upsert_segment(&pool, &a, true, Some("2026-05-20T09:00:00.000000+00:00"))
            .await
            .unwrap()
            .unwrap();
        write_summary(&pool, aid, "burst A summary", "claude")
            .await
            .unwrap();

        let prior = fetch_prior_summary(&pool, "s", "2026-05-20T10:00:00.000000+00:00")
            .await
            .unwrap();
        assert_eq!(prior.as_deref(), Some("burst A summary"));

        // For the earliest burst there is no prior.
        let none = fetch_prior_summary(&pool, "s", "2026-05-20T08:00:00.000000+00:00")
            .await
            .unwrap();
        assert!(none.is_none());
    }
}
