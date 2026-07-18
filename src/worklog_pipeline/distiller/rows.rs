//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Stage 0 — load the hour's screen sessions from `app_sessions`.
//!
//! Ports `session_distiller._load_rows` (the `exclude_coding=True` worklog variant): the
//! same column set, the same `EXCLUDE_APPS` / `duration_s` / non-empty-`session_text`
//! filters, plus `coding_agent_session_uuid IS NULL` so coding-agent transcripts (whose
//! clean summaries are folded in verbatim elsewhere) never reach this OCR-tuned
//! compressor. Reuses the live [`sqlx::SqlitePool`] rather than opening its own
//! connection like the Python did.

use sqlx::{Row, SqlitePool};
use tracing::Instrument;

use super::{EXCLUDE_APPS, MIN_SESSION_DUR_S};

/// One screen session, exactly the columns the compressor reads. (The Python also
/// selected `task_key`/`task_session_type`, but only for optional per-session spans that
/// aren't emitted here, so they're omitted.)
pub struct SessionRow {
    pub id: i64,
    pub app_name: String,
    pub started_at: String,
    pub duration_s: i64,
    pub window_titles: String,
    pub session_text: String,
}

/// Load the `[hs, he)` window's OCR/app sessions in time order. `hs`/`he` are the UTC
/// `+00:00` bounds the hour driver already computes. A missing table/column degrades to
/// an empty vec (never fail the hour on a read).
pub async fn load_sessions(pool: &SqlitePool, hs: &str, he: &str) -> Vec<SessionRow> {
    // EXCLUDE_APPS is a fixed 4-element list; build its placeholder list inline.
    let placeholders = vec!["?"; EXCLUDE_APPS.len()].join(", ");
    let sql = format!(
        "SELECT id, app_name, started_at, CAST(COALESCE(duration_s, 0) AS INTEGER) AS duration_s, \
                COALESCE(window_titles, '') AS window_titles, \
                COALESCE(session_text, '') AS session_text \
         FROM app_sessions \
         WHERE app_name NOT IN ({placeholders}) \
           AND COALESCE(duration_s, 0) >= ? \
           AND session_text IS NOT NULL AND LENGTH(session_text) > 40 \
           AND coding_agent_session_uuid IS NULL \
           AND started_at >= ? AND started_at < ? \
         ORDER BY started_at",
    );
    let mut q = sqlx::query(&sql);
    for app in EXCLUDE_APPS.iter() {
        q = q.bind(*app);
    }
    q = q.bind(*MIN_SESSION_DUR_S).bind(hs).bind(he);

    let res = q
        .fetch_all(pool)
        .instrument(tracing::debug_span!("distil.read.app_sessions"))
        .await;

    match res {
        Ok(rows) => {
            let out: Vec<SessionRow> = rows
                .into_iter()
                .map(|r| SessionRow {
                    id: r.try_get::<i64, _>("id").unwrap_or(0),
                    app_name: r.try_get::<String, _>("app_name").unwrap_or_default(),
                    started_at: r.try_get::<String, _>("started_at").unwrap_or_default(),
                    duration_s: r.try_get::<i64, _>("duration_s").unwrap_or(0),
                    window_titles: r.try_get::<String, _>("window_titles").unwrap_or_default(),
                    session_text: r.try_get::<String, _>("session_text").unwrap_or_default(),
                })
                .collect();
            tracing::debug!(rows = out.len(), "distil: loaded app_sessions");
            out
        }
        Err(e) => {
            tracing::warn!(error = %e, "distil: app_sessions query failed — treating as empty");
            Vec::new()
        }
    }
}
