//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// SQLite read/write layer for coding-agent rows (app_sessions rows carrying a
// non-NULL coding_agent_session_uuid). One row per segment, keyed on
// (coding_agent_session_uuid, segment_started_at).
//
// Lifecycle:
//   * LIVE   — sealed_at IS NULL, task_method = 'coding_agent_live'.
//              Re-UPSERTed each poll while the burst is still growing.
//   * SEALED — sealed_at set, task_method = 'pending_summariser'.
//              Immutable: the UPSERT carries `WHERE sealed_at IS NULL`, so a
//              sealed row is never mutated again. Downstream (summariser /
//              classifier) only ever reads sealed rows.
//
// Faithful port of the former Python indexer/db.py. Uses the daemon's
// shared meridian RW pool (sqlx) instead of short-lived sqlite3 connections.

use std::collections::HashMap;

use anyhow::{Context, Result};
use chrono::Duration;
use sqlx::SqlitePool;

use super::segment::{iso_utc, parse_iso, Segment};

// Sentinel for rows we own — not produced by any ETL run.
const INDEXER_ETL_RUN_ID: i64 = 0;

const EMPTY_JSON_LIST: &str = "[]";
const CATEGORY: &str = "coding"; // we are sure: this IS coding work
const CATEGORY_METHOD: &str = "coding_agent_indexer";

/// `task_method` is non-NULL in BOTH states so the MLX classifier (which
/// selects `WHERE task_method IS NULL`) skips these rows whether live or
/// sealed. The summariser queue is `WHERE task_method = 'pending_summariser'`.
pub const TASK_METHOD_LIVE: &str = "coding_agent_live";
pub const TASK_METHOD_PENDING: &str = "pending_summariser";

/// Agent flavour → app_name. We use the agent's product name, not the host
/// terminal/IDE: a Claude Code session is `Claude Code` whether run inside VS
/// Code, iTerm2, or any other terminal. Agent products that share a name with
/// a screen-captured app (Cursor the IDE vs Cursor's agent) get a distinct
/// suffix so transcript rows don't mix with screen-ETL rows in dashboards.
fn app_name_for(agent: &str) -> &'static str {
    match agent {
        "codex" => "Codex",
        "copilot_cli" | "copilot_vscode" => "GitHub Copilot",
        "cursor" | "cursor_cli" => "Cursor Agent",
        "antigravity" => "Antigravity Agent",
        _ => "Claude Code",
    }
}

fn text_source_for(agent: &str) -> Option<&'static str> {
    match agent {
        "claude_code" => Some("claude_jsonl"),
        "codex" => Some("codex_jsonl"),
        "copilot_cli" => Some("copilot_events_jsonl"),
        "copilot_vscode" => Some("copilot_chat_jsonl"),
        "cursor" => Some("cursor_vscdb"),
        "cursor_cli" => Some("cursor_cli_store"),
        "antigravity" => Some("antigravity"),
        _ => None,
    }
}

// ──────────────────────── Write paths ──────────────────────────────────────

/// INSERT or UPDATE one (uuid, segment_started_at) row.
///
/// `sealed=false` writes/refreshes a LIVE row (mutable, re-UPSERTed each poll).
/// `sealed=true` seals the row — `task_method` flips to `pending_summariser`
/// and `sealed_at` is stamped. The UPDATE branch carries `WHERE sealed_at IS
/// NULL`, so once sealed a row is immutable: subsequent UPSERTs for the same
/// key are no-ops.
///
/// Returns the row id, or None if the segment was rejected as invalid.
pub async fn upsert_segment(
    pool: &SqlitePool,
    segment: &Segment,
    sealed: bool,
    sealed_at: Option<&str>,
) -> Result<Option<i64>> {
    if !segment.is_valid() {
        tracing::info!(
            uuid = %segment.session_uuid,
            seg_start = %segment.segment_started_at,
            user = segment.user_turns,
            asst = segment.assistant_turns,
            "skip upsert: segment invalid",
        );
        return Ok(None);
    }

    let app_name = app_name_for(&segment.agent);
    let has_text = !segment.transcript.is_empty();
    let session_text: Option<&str> = if has_text {
        Some(&segment.transcript)
    } else {
        None
    };
    let text_source: Option<&str> = if has_text {
        text_source_for(&segment.agent)
    } else {
        None
    };
    let frame_count: i64 = segment.user_turns as i64 + segment.assistant_turns as i64;
    let task_method = if sealed {
        TASK_METHOD_PENDING
    } else {
        TASK_METHOD_LIVE
    };
    let sealed_stamp: Option<&str> = if sealed { sealed_at } else { None };
    // The agent's own session name rides in window_titles using the canonical
    // ETL shape ([{"window_name", "count"}]) so the dashboard and classifier
    // read coding-agent rows exactly like screen-capture rows. No name → [].
    let window_titles: String = match &segment.title {
        Some(t) => serde_json::json!([{ "window_name": t, "count": 1 }]).to_string(),
        None => EMPTY_JSON_LIST.to_string(),
    };

    sqlx::query(
        r#"
        INSERT INTO app_sessions (
            app_name, started_at, ended_at, duration_s,
            window_titles, min_frame_id, max_frame_id, frame_count,
            etl_run_id, idle_frame_count,
            category, confidence, category_method,
            session_text, session_text_source,
            task_method,
            coding_agent_session_uuid, segment_started_at, sealed_at
        )
        VALUES (?, ?, ?, ?,  ?, ?, ?, ?,  ?, ?,  ?, ?, ?,  ?, ?,  ?,  ?, ?, ?)
        ON CONFLICT (coding_agent_session_uuid, segment_started_at)
        WHERE coding_agent_session_uuid IS NOT NULL
        DO UPDATE SET
            started_at          = excluded.started_at,
            ended_at            = excluded.ended_at,
            duration_s          = excluded.duration_s,
            frame_count         = excluded.frame_count,
            session_text        = excluded.session_text,
            session_text_source = excluded.session_text_source,
            window_titles       = excluded.window_titles,
            task_method         = excluded.task_method,
            sealed_at           = excluded.sealed_at
        WHERE app_sessions.sealed_at IS NULL
        "#,
    )
    .bind(app_name)
    .bind(&segment.started_at)
    .bind(&segment.ended_at)
    .bind(segment.active_seconds)
    .bind(&window_titles)
    .bind(0_i64) // min_frame_id (sentinel)
    .bind(0_i64) // max_frame_id (sentinel)
    .bind(frame_count)
    .bind(INDEXER_ETL_RUN_ID)
    .bind(0_i64) // idle_frame_count
    .bind(CATEGORY)
    .bind(1.0_f64) // confidence
    .bind(CATEGORY_METHOD)
    .bind(session_text)
    .bind(text_source)
    .bind(task_method)
    .bind(&segment.session_uuid)
    .bind(&segment.segment_started_at)
    .bind(sealed_stamp)
    .execute(pool)
    .await
    .context("upsert coding-agent segment")?;

    let id: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM app_sessions WHERE coding_agent_session_uuid = ? AND segment_started_at = ?",
    )
    .bind(&segment.session_uuid)
    .bind(&segment.segment_started_at)
    .fetch_optional(pool)
    .await
    .context("fetch upserted segment id")?;

    Ok(id)
}

/// Seal every LIVE coding-agent row that is settled, by either rule (no JSONL
/// re-parse needed; idempotent — sealed rows are excluded; returns rows sealed):
///
/// 1. idle — last activity is > `idle_seconds` old: the crash / force-quit /
///    sleep / deleted-file backstop.
/// 2. hour-elapsed — its bucket hour has fully passed. Coding rows are floored
///    to their completion-hour (`started_at` = `…:00:00`), so once
///    `started_at + 1h + hour_grace_seconds` is in the past, no future exchange
///    can join the row; it is final and seals promptly without waiting out the
///    idle window. This is what lets an hour's worklog run shortly after the
///    hour ends. The current hour's live row never matches (its floor is < 1h
///    old), so this can't seal mid-hour work.
pub async fn seal_stale_open_rows(
    pool: &SqlitePool,
    now_iso: &str,
    idle_seconds: i64,
    hour_grace_seconds: i64,
) -> Result<u64> {
    const HOUR_SECONDS: i64 = 3600;
    let idle_cutoff = shift_iso(now_iso, -idle_seconds);
    let hour_cutoff = shift_iso(now_iso, -(HOUR_SECONDS + hour_grace_seconds));
    let res = sqlx::query(
        r#"
        UPDATE app_sessions
        SET    sealed_at = ?, task_method = ?
        WHERE  coding_agent_session_uuid IS NOT NULL
          AND  sealed_at IS NULL
          AND  (ended_at < ? OR started_at < ?)
        "#,
    )
    .bind(now_iso)
    .bind(TASK_METHOD_PENDING)
    .bind(&idle_cutoff)
    .bind(&hour_cutoff)
    .execute(pool)
    .await
    .context("seal stale open coding-agent rows")?;
    Ok(res.rows_affected())
}

/// Seal every LIVE row of one `session_text_source`, regardless of idle time.
/// Used when the source's CLI process is GONE (Ctrl+C, window closed): with no
/// process running the store cannot grow, so its live rows are finished —
/// sealing now instead of waiting out the idle window. Returns rows sealed.
pub async fn seal_live_rows_of_source(
    pool: &SqlitePool,
    now_iso: &str,
    text_source: &str,
) -> Result<u64> {
    let res = sqlx::query(
        r#"
        UPDATE app_sessions
        SET    sealed_at = ?, task_method = ?
        WHERE  coding_agent_session_uuid IS NOT NULL
          AND  sealed_at IS NULL
          AND  session_text_source = ?
        "#,
    )
    .bind(now_iso)
    .bind(TASK_METHOD_PENDING)
    .bind(text_source)
    .execute(pool)
    .await
    .context("seal live rows of exited CLI source")?;
    Ok(res.rows_affected())
}

/// Seal LIVE rows of one source that were SUPERSEDED by a newer session of
/// the same source — the /clear (or /new) case: the CLI keeps running but the
/// user started a fresh conversation, so the previous one is over. A parallel
/// still-active session that gets sealed merely splits into a new segment on
/// its next turn (the sealed high-water rule), so acceleration is safe.
pub async fn seal_superseded_rows_of_source(
    pool: &SqlitePool,
    now_iso: &str,
    text_source: &str,
) -> Result<u64> {
    let res = sqlx::query(
        r#"
        UPDATE app_sessions
        SET    sealed_at = ?1, task_method = ?2
        WHERE  coding_agent_session_uuid IS NOT NULL
          AND  sealed_at IS NULL
          AND  session_text_source = ?3
          AND  EXISTS (
                 SELECT 1 FROM app_sessions n
                 WHERE  n.session_text_source = ?3
                   AND  n.coding_agent_session_uuid <> app_sessions.coding_agent_session_uuid
                   AND  n.started_at > app_sessions.ended_at
               )
        "#,
    )
    .bind(now_iso)
    .bind(TASK_METHOD_PENDING)
    .bind(text_source)
    .execute(pool)
    .await
    .context("seal superseded rows of CLI source")?;
    Ok(res.rows_affected())
}

/// Delete every Claude/Codex-owned app_sessions row (reseed). Only touches rows
/// the indexer owns (`coding_agent_session_uuid IS NOT NULL`); never screen-frame
/// rows. Returns rows deleted.
pub async fn delete_claude_session_rows(pool: &SqlitePool) -> Result<u64> {
    let res = sqlx::query("DELETE FROM app_sessions WHERE coding_agent_session_uuid IS NOT NULL")
        .execute(pool)
        .await
        .context("delete coding-agent rows")?;
    Ok(res.rows_affected())
}

// ──────────────────────── Read paths ────────────────────────────────────────

/// Return {coding_agent_session_uuid: latest_ended_at} across all its segments.
/// Used by the daemon's change-detection: skip parsing a JSONL whose mtime
/// hasn't moved past the latest stored `ended_at`.
pub async fn fetch_session_endpoints(pool: &SqlitePool) -> Result<HashMap<String, String>> {
    let rows: Vec<(String, String)> = sqlx::query_as(
        r#"
        SELECT coding_agent_session_uuid, MAX(ended_at) AS ended_at
        FROM   app_sessions
        WHERE  coding_agent_session_uuid IS NOT NULL
        GROUP  BY coding_agent_session_uuid
        "#,
    )
    .fetch_all(pool)
    .await
    .context("fetch coding-agent session endpoints")?;
    Ok(rows.into_iter().collect())
}

/// Latest `ended_at` among this session's SEALED segments, or None. Passed to
/// the parser as `start_after_ts` so already-sealed content is excluded and any
/// newer record opens a fresh segment (makes a post-SessionEnd resume safe).
pub async fn sealed_high_water(pool: &SqlitePool, uuid: &str) -> Result<Option<String>> {
    let hwm: Option<String> = sqlx::query_scalar(
        "SELECT MAX(ended_at) FROM app_sessions \
         WHERE coding_agent_session_uuid = ? AND sealed_at IS NOT NULL",
    )
    .bind(uuid)
    .fetch_one(pool)
    .await
    .context("fetch sealed high-water mark")?;
    Ok(hwm.filter(|s| !s.is_empty()))
}

// ──────────────────────── Helpers ──────────────────────────────────────────

/// Shift an ISO timestamp by `delta_seconds`, in the canonical µs+'+00:00' UTC
/// format. Lexicographic comparison of two same-format UTC strings is a valid
/// chronological comparison, so the seal sweep's `ended_at < cutoff` works as a
/// plain string compare.
fn shift_iso(iso: &str, delta_seconds: i64) -> String {
    match parse_iso(iso) {
        Some(dt) => iso_utc(dt + Duration::seconds(delta_seconds)),
        None => iso.to_string(),
    }
}

// ──────────────────────── Tests ─────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coding_agent_session_ingest::segment::Segment;
    use sqlx::sqlite::SqliteConnectOptions;
    use std::str::FromStr;

    async fn fresh_db() -> SqlitePool {
        let opts = SqliteConnectOptions::from_str("sqlite::memory:")
            .unwrap()
            .create_if_missing(true);
        let pool = SqlitePool::connect_with(opts).await.unwrap();
        sqlx::migrate!("src/migrations").run(&pool).await.unwrap();
        pool
    }

    fn seg(uuid: &str, seg_start: &str, ended: &str, turns: u32) -> Segment {
        Segment {
            session_uuid: uuid.to_string(),
            agent: "claude_code".to_string(),
            cwd: Some("/repo".to_string()),
            segment_started_at: seg_start.to_string(),
            started_at: seg_start.to_string(),
            ended_at: ended.to_string(),
            user_turns: turns,
            assistant_turns: turns,
            active_seconds: 100,
            transcript: "[user] hi\n\n[claude-code] yo".to_string(),
            is_last: true,
            title: None,
        }
    }

    async fn row_fields(pool: &SqlitePool, id: i64) -> (Option<String>, String) {
        sqlx::query_as::<_, (Option<String>, String)>(
            "SELECT sealed_at, task_method FROM app_sessions WHERE id = ?",
        )
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn upsert_live_then_seal_then_immutable() {
        let pool = fresh_db().await;
        let s = seg(
            "u1",
            "2026-05-20T08:00:00.000000+00:00",
            "2026-05-20T08:10:00.000000+00:00",
            2,
        );

        // Live insert.
        let id = upsert_segment(&pool, &s, false, None)
            .await
            .unwrap()
            .unwrap();
        let (sealed, method) = row_fields(&pool, id).await;
        assert!(sealed.is_none());
        assert_eq!(method, TASK_METHOD_LIVE);

        // Live re-upsert updates the same row (ended_at grows).
        let mut s2 = s.clone();
        s2.ended_at = "2026-05-20T08:20:00.000000+00:00".to_string();
        let id2 = upsert_segment(&pool, &s2, false, None)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(id, id2, "same key → same row updated, not duplicated");

        // Seal it.
        let id3 = upsert_segment(&pool, &s2, true, Some("2026-05-20T09:00:00.000000+00:00"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(id, id3);
        let (sealed, method) = row_fields(&pool, id).await;
        assert_eq!(sealed.as_deref(), Some("2026-05-20T09:00:00.000000+00:00"));
        assert_eq!(method, TASK_METHOD_PENDING);

        // Immutability: a re-upsert after sealing is a no-op (ended_at frozen).
        let mut s3 = s2.clone();
        s3.ended_at = "2026-05-20T08:55:00.000000+00:00".to_string();
        upsert_segment(&pool, &s3, false, None).await.unwrap();
        let frozen: String = sqlx::query_scalar("SELECT ended_at FROM app_sessions WHERE id = ?")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(
            frozen, "2026-05-20T08:20:00.000000+00:00",
            "sealed row must not mutate"
        );
    }

    #[tokio::test]
    async fn title_written_as_window_titles_and_refreshed_while_live() {
        let pool = fresh_db().await;
        let wt = |pool: &SqlitePool, id: i64| {
            let pool = pool.clone();
            async move {
                sqlx::query_scalar::<_, String>(
                    "SELECT window_titles FROM app_sessions WHERE id = ?",
                )
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap()
            }
        };

        // No title → the [] sentinel.
        let s1 = seg(
            "t1",
            "2026-05-20T08:00:00.000000+00:00",
            "2026-05-20T08:10:00.000000+00:00",
            2,
        );
        let id = upsert_segment(&pool, &s1, false, None)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(wt(&pool, id).await, "[]");

        // Title appearing later (Cursor names after the first reply) refreshes
        // the LIVE row in the canonical ETL shape.
        let mut s2 = s1.clone();
        s2.title = Some("Fix the login bug".into());
        let id2 = upsert_segment(&pool, &s2, false, None)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(id, id2);
        assert_eq!(
            wt(&pool, id).await,
            r#"[{"count":1,"window_name":"Fix the login bug"}]"#
        );

        // Sealed rows stay immutable — a title change after sealing is a no-op.
        upsert_segment(&pool, &s2, true, Some("2026-05-20T09:00:00.000000+00:00"))
            .await
            .unwrap();
        let mut s3 = s2.clone();
        s3.title = Some("Renamed later".into());
        upsert_segment(&pool, &s3, false, None).await.unwrap();
        assert_eq!(
            wt(&pool, id).await,
            r#"[{"count":1,"window_name":"Fix the login bug"}]"#,
            "sealed row keeps its title"
        );
    }

    #[tokio::test]
    async fn invalid_segment_returns_none() {
        let pool = fresh_db().await;
        let mut s = seg(
            "u2",
            "2026-05-20T08:00:00.000000+00:00",
            "2026-05-20T08:10:00.000000+00:00",
            0,
        );
        s.user_turns = 0;
        s.assistant_turns = 0; // no turns → invalid
        assert!(upsert_segment(&pool, &s, false, None)
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn cli_source_seals_are_scoped_and_supersede_correctly() {
        let pool = fresh_db().await;
        // Two codex sessions: A ended before B started (the /clear shape).
        let mut a = seg(
            "codex-a",
            "2026-05-20T05:00:00.000000+00:00",
            "2026-05-20T05:30:00.000000+00:00",
            2,
        );
        a.agent = "codex".to_string();
        let a_id = upsert_segment(&pool, &a, false, None)
            .await
            .unwrap()
            .unwrap();
        let mut b = seg(
            "codex-b",
            "2026-05-20T06:00:00.000000+00:00",
            "2026-05-20T06:10:00.000000+00:00",
            2,
        );
        b.agent = "codex".to_string();
        let b_id = upsert_segment(&pool, &b, false, None)
            .await
            .unwrap()
            .unwrap();
        // A claude row in the same shape must be untouched by codex seals.
        let c = seg(
            "claude-c",
            "2026-05-20T05:00:00.000000+00:00",
            "2026-05-20T05:30:00.000000+00:00",
            2,
        );
        let c_id = upsert_segment(&pool, &c, false, None)
            .await
            .unwrap()
            .unwrap();

        let now = "2026-05-20T08:00:00.000000+00:00";
        // Superseded pass: A (older, superseded by B) seals; B stays live.
        let n = seal_superseded_rows_of_source(&pool, now, "codex_jsonl")
            .await
            .unwrap();
        assert_eq!(n, 1);
        assert!(row_fields(&pool, a_id).await.0.is_some(), "A sealed");
        assert!(row_fields(&pool, b_id).await.0.is_none(), "B still live");

        // Process-gone pass: every remaining live codex row seals.
        let n = seal_live_rows_of_source(&pool, now, "codex_jsonl")
            .await
            .unwrap();
        assert_eq!(n, 1);
        assert!(row_fields(&pool, b_id).await.0.is_some(), "B sealed");
        assert!(
            row_fields(&pool, c_id).await.0.is_none(),
            "claude row untouched by codex-scoped seals"
        );
    }

    #[tokio::test]
    async fn seal_stale_seals_old_live_only() {
        let pool = fresh_db().await;
        // Old live row (ended 3h before now) → should seal.
        let old = seg(
            "old",
            "2026-05-20T05:00:00.000000+00:00",
            "2026-05-20T05:30:00.000000+00:00",
            2,
        );
        let old_id = upsert_segment(&pool, &old, false, None)
            .await
            .unwrap()
            .unwrap();
        // Recent live row (ended 10 min before now) → should NOT seal.
        let recent = seg(
            "recent",
            "2026-05-20T07:50:00.000000+00:00",
            "2026-05-20T07:50:00.000000+00:00",
            2,
        );
        let recent_id = upsert_segment(&pool, &recent, false, None)
            .await
            .unwrap()
            .unwrap();

        let now = "2026-05-20T08:00:00.000000+00:00";
        let n = seal_stale_open_rows(&pool, now, 3600, 90).await.unwrap();
        assert_eq!(n, 1);
        assert!(
            row_fields(&pool, old_id).await.0.is_some(),
            "old row sealed"
        );
        assert!(
            row_fields(&pool, recent_id).await.0.is_none(),
            "recent row stays live"
        );
    }

    #[tokio::test]
    async fn seal_stale_seals_elapsed_hour_even_when_not_idle() {
        let pool = fresh_db().await;
        // A PAST hour's row, floored to its hour, with RECENT activity (33 min
        // ago → not idle). The hour-elapsed rule must still seal it promptly.
        let past_hour = seg(
            "past",
            "2026-05-20T06:00:00.000000+00:00", // hour floor
            "2026-05-20T06:58:00.000000+00:00", // last completion 33 min before now
            2,
        );
        let past_id = upsert_segment(&pool, &past_hour, false, None)
            .await
            .unwrap()
            .unwrap();
        // The CURRENT hour's live row must NOT seal (its floor is < 1h old).
        let cur_hour = seg(
            "cur",
            "2026-05-20T07:00:00.000000+00:00",
            "2026-05-20T07:30:00.000000+00:00",
            2,
        );
        let cur_id = upsert_segment(&pool, &cur_hour, false, None)
            .await
            .unwrap()
            .unwrap();

        let now = "2026-05-20T07:31:00.000000+00:00";
        // idle_seconds huge (1 day) so the idle rule can't fire — only the
        // hour-elapsed rule can.
        let n = seal_stale_open_rows(&pool, now, 86_400, 90).await.unwrap();
        assert_eq!(n, 1, "only the elapsed-hour row seals");
        assert!(
            row_fields(&pool, past_id).await.0.is_some(),
            "past-hour row sealed by the hour-elapsed rule"
        );
        assert!(
            row_fields(&pool, cur_id).await.0.is_none(),
            "current-hour live row stays live"
        );
    }

    #[tokio::test]
    async fn sealed_high_water_and_endpoints() {
        let pool = fresh_db().await;
        let a1 = seg(
            "a",
            "2026-05-20T08:00:00.000000+00:00",
            "2026-05-20T08:30:00.000000+00:00",
            2,
        );
        upsert_segment(&pool, &a1, true, Some("2026-05-20T09:00:00.000000+00:00"))
            .await
            .unwrap();
        let a2 = seg(
            "a",
            "2026-05-20T10:00:00.000000+00:00",
            "2026-05-20T10:30:00.000000+00:00",
            2,
        );
        upsert_segment(&pool, &a2, false, None).await.unwrap(); // live, not sealed

        // High-water = latest SEALED ended_at (ignores the live a2).
        let hwm = sealed_high_water(&pool, "a").await.unwrap();
        assert_eq!(hwm.as_deref(), Some("2026-05-20T08:30:00.000000+00:00"));

        // Endpoints = latest ended_at across ALL segments (includes live a2).
        let eps = fetch_session_endpoints(&pool).await.unwrap();
        assert_eq!(
            eps.get("a").map(String::as_str),
            Some("2026-05-20T10:30:00.000000+00:00")
        );
    }

    #[tokio::test]
    async fn delete_reseed_clears_only_coding_rows() {
        let pool = fresh_db().await;
        let s = seg(
            "z",
            "2026-05-20T08:00:00.000000+00:00",
            "2026-05-20T08:10:00.000000+00:00",
            2,
        );
        upsert_segment(&pool, &s, false, None).await.unwrap();
        let n = delete_claude_session_rows(&pool).await.unwrap();
        assert_eq!(n, 1);
        let eps = fetch_session_endpoints(&pool).await.unwrap();
        assert!(eps.is_empty());
    }
}
