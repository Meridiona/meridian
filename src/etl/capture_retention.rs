//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity

//! Age + processed-based retention sweep for the capture_* tables
//! (`capture_frames`, `capture_ui_events`, `capture_secondary_screens`) the
//! tray's in-process capture engine writes into `meridian.db`.
//!
//! Nothing previously pruned these — a frame every ~2s per monitor plus every
//! click/key/clipboard event, each carrying full OCR/a11y text, grows the DB
//! forever. This module prunes rows that are BOTH:
//!
//!   (a) older than `MERIDIAN_CAPTURE_RETENTION_DAYS` (default 30), AND
//!   (b) already fully processed by the ETL — strictly before the timestamp
//!       of the frame at `etl_cursor.last_frame_id` — so a frame the ETL
//!       hasn't consumed yet is never deleted regardless of age.
//!
//! `capture_ui_events` and `capture_secondary_screens` have no frame-id link,
//! so they're bounded by the same cursor-frame timestamp: any row timestamped
//! at or after it could still belong to a session the ETL hasn't closed, so
//! it is left alone.
//!
//! Deletes alone don't shrink the SQLite file — `run_incremental_vacuum`
//! reclaims freed pages on a coarser cadence (not every tick, since it's
//! extra I/O), bounded per call via `PRAGMA incremental_vacuum(N)` so it
//! can't stall the daemon scanning a large freelist in one call. This only
//! does anything once the database's `auto_vacuum` mode is `INCREMENTAL`
//! (set in `db::meridian::setup_db` for brand-new databases only — retrofitting
//! an already-populated database would require a full, slow `VACUUM`, which
//! this module deliberately never runs on a live daemon). On a pre-existing
//! database still in the default `auto_vacuum = NONE` mode,
//! `incremental_vacuum` is simply a documented no-op — see SQLite's pragma
//! docs — so pruned rows still stop growing the *logical* table size, they
//! just don't shrink the on-disk file until the user runs a manual `VACUUM`.
//!
//! # Who calls this
//!
//! `src/main.rs`'s poll loop, once per tick, right after `run_etl`.

use anyhow::{Context, Result};
use sqlx::SqlitePool;
use std::sync::atomic::{AtomicU64, Ordering};

/// Default retention window, in days, for the capture_* tables.
const DEFAULT_CAPTURE_RETENTION_DAYS: u64 = 30;

/// Run `PRAGMA incremental_vacuum` once every this many `prune_capture_tables`
/// calls (i.e. roughly once every N poll ticks) rather than every tick.
const VACUUM_EVERY_N_TICKS: u64 = 60;

/// Max pages reclaimed per `incremental_vacuum` call — bounds how long a single
/// call can run so it can't stall the daemon on a large freelist.
const VACUUM_MAX_PAGES: i64 = 500;

/// Tick counter driving the vacuum cadence. Process-lifetime only (resets on
/// restart) — an off-by-a-few-ticks cadence drift after a restart is harmless.
static TICK_COUNT: AtomicU64 = AtomicU64::new(0);

fn capture_retention_days() -> u64 {
    std::env::var("MERIDIAN_CAPTURE_RETENTION_DAYS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_CAPTURE_RETENTION_DAYS)
}

/// Prunes old, already-ETL-processed rows from `capture_frames`,
/// `capture_ui_events`, and `capture_secondary_screens`, then — on a coarser
/// cadence — runs a bounded `incremental_vacuum` to reclaim freed pages.
///
/// Call once per ETL poll tick, after `run_etl` has advanced the cursor.
#[tracing::instrument(skip(pool))]
pub async fn prune_capture_tables(pool: &SqlitePool) -> Result<()> {
    let retention_days = capture_retention_days();
    let cutoff_ts = (chrono::Utc::now() - chrono::Duration::days(retention_days as i64))
        .to_rfc3339_opts(chrono::SecondsFormat::Micros, true);

    // etl_cursor.last_frame_id is the last frame the ETL has actually
    // consumed — never prune a frame at or after it.
    let cursor: Option<(i64,)> =
        sqlx::query_as("SELECT last_frame_id FROM etl_cursor WHERE id = 1")
            .fetch_optional(pool)
            .await
            .context("read etl_cursor for capture retention")?;
    let Some((last_frame_id,)) = cursor else {
        tracing::debug!("capture retention: no etl_cursor row yet — nothing to prune");
        return Ok(());
    };
    if last_frame_id <= 0 {
        tracing::debug!(
            last_frame_id,
            "capture retention: cursor at/before 0 — nothing processed yet"
        );
        return Ok(());
    }

    let cursor_frame: Option<(String,)> =
        sqlx::query_as("SELECT timestamp FROM capture_frames WHERE id = ?1")
            .bind(last_frame_id)
            .fetch_optional(pool)
            .await
            .context("read cursor frame timestamp for capture retention")?;
    let Some((cursor_ts,)) = cursor_frame else {
        // The cursor's own frame is already gone (previously pruned, or a
        // fresh DB with a stale cursor) — be conservative and skip this tick
        // rather than guess a boundary.
        tracing::debug!(
            last_frame_id,
            "capture retention: cursor frame not found — skipping this tick"
        );
        return Ok(());
    };

    // capture_frames has an explicit processed marker (id <= last_frame_id),
    // so "processed" and "age" are independent conditions there — no need to
    // also bound by the cursor frame's own timestamp.
    let frames_deleted =
        sqlx::query("DELETE FROM capture_frames WHERE id <= ?1 AND timestamp < ?2")
            .bind(last_frame_id)
            .bind(&cutoff_ts)
            .execute(pool)
            .await
            .context("prune capture_frames")?
            .rows_affected();

    // capture_ui_events / capture_secondary_screens have no frame-id link, so
    // "processed" is approximated by "strictly before the cursor frame's own
    // timestamp" — a row AT that timestamp might belong to a session the ETL
    // hasn't finished closing yet, so it's left alone. Combined with the age
    // cutoff via the earlier (chronologically smaller/older) of the two —
    // never delete anything at or after either boundary.
    let ui_secondary_boundary = if cutoff_ts < cursor_ts {
        &cutoff_ts
    } else {
        &cursor_ts
    };

    let ui_events_deleted = sqlx::query("DELETE FROM capture_ui_events WHERE timestamp < ?1")
        .bind(ui_secondary_boundary)
        .execute(pool)
        .await
        .context("prune capture_ui_events")?
        .rows_affected();

    let secondary_screens_deleted =
        sqlx::query("DELETE FROM capture_secondary_screens WHERE timestamp < ?1")
            .bind(ui_secondary_boundary)
            .execute(pool)
            .await
            .context("prune capture_secondary_screens")?
            .rows_affected();

    if frames_deleted > 0 || ui_events_deleted > 0 || secondary_screens_deleted > 0 {
        tracing::info!(
            frames_deleted,
            ui_events_deleted,
            secondary_screens_deleted,
            cutoff_ts = %cutoff_ts,
            cursor_ts = %cursor_ts,
            retention_days,
            "capture retention: pruned old, already-processed rows"
        );
    } else {
        tracing::debug!(
            cutoff_ts = %cutoff_ts,
            cursor_ts = %cursor_ts,
            retention_days,
            "capture retention: nothing to prune this tick"
        );
    }

    let tick = TICK_COUNT.fetch_add(1, Ordering::Relaxed);
    if tick.is_multiple_of(VACUUM_EVERY_N_TICKS) {
        run_incremental_vacuum(pool).await?;
    }

    Ok(())
}

/// Runs a page-bounded `incremental_vacuum` to reclaim freed pages from the
/// deletes above. A no-op (per SQLite's own pragma semantics) unless the
/// database's `auto_vacuum` mode is `INCREMENTAL` — see this module's header
/// doc for why we never force that conversion (it requires a full `VACUUM`)
/// on an already-populated database.
async fn run_incremental_vacuum(pool: &SqlitePool) -> Result<()> {
    sqlx::query(&format!("PRAGMA incremental_vacuum({VACUUM_MAX_PAGES})"))
        .execute(pool)
        .await
        .context("incremental_vacuum on capture retention cadence")?;
    tracing::debug!(
        max_pages = VACUUM_MAX_PAGES,
        "capture retention: incremental_vacuum ran"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqliteConnectOptions;
    use std::str::FromStr;

    async fn make_db() -> SqlitePool {
        let opts = SqliteConnectOptions::from_str("sqlite::memory:")
            .unwrap()
            .create_if_missing(true);
        let pool = SqlitePool::connect_with(opts).await.unwrap();
        sqlx::migrate!("src/migrations").run(&pool).await.unwrap();
        pool
    }

    async fn insert_frame(pool: &SqlitePool, id: i64, ts: &str, text: &str) {
        sqlx::query(
            "INSERT INTO capture_frames (id, app_name, window_name, timestamp, accessibility_text, text_source)
             VALUES (?, 'Code', NULL, ?, ?, 'accessibility')",
        )
        .bind(id)
        .bind(ts)
        .bind(text)
        .execute(pool)
        .await
        .unwrap();
    }

    async fn insert_ui_event(pool: &SqlitePool, ts: &str) {
        sqlx::query(
            "INSERT INTO capture_ui_events (timestamp, event_type, app_name) VALUES (?, 'app_switch', 'Code')",
        )
        .bind(ts)
        .execute(pool)
        .await
        .unwrap();
    }

    async fn set_cursor(pool: &SqlitePool, last_frame_id: i64) {
        // No row exists until the real ETL's get_cursor() lazily seeds one —
        // upsert here rather than UPDATE so the test doesn't depend on that.
        sqlx::query(
            "INSERT INTO etl_cursor (id, last_frame_id) VALUES (1, ?1)
             ON CONFLICT (id) DO UPDATE SET last_frame_id = excluded.last_frame_id",
        )
        .bind(last_frame_id)
        .execute(pool)
        .await
        .unwrap();
    }

    /// A row older than retention but AT/AFTER the cursor frame (not yet
    /// processed by the ETL) must survive.
    #[tokio::test]
    async fn unprocessed_frame_survives_even_if_old() {
        let pool = make_db().await;

        let old_ts = (chrono::Utc::now() - chrono::Duration::days(60))
            .to_rfc3339_opts(chrono::SecondsFormat::Micros, true);
        insert_frame(&pool, 1, &old_ts, "processed old frame").await;
        insert_frame(&pool, 2, &old_ts, "unprocessed old frame").await;
        // Cursor only covers frame 1 — frame 2 is not yet processed.
        set_cursor(&pool, 1).await;

        prune_capture_tables(&pool).await.unwrap();

        let remaining: Vec<(i64,)> = sqlx::query_as("SELECT id FROM capture_frames ORDER BY id")
            .fetch_all(&pool)
            .await
            .unwrap();
        assert_eq!(
            remaining,
            vec![(2,)],
            "frame 1 (processed + old) must be pruned; frame 2 (unprocessed) must survive"
        );
    }

    /// A processed row inside the retention window must survive even though
    /// the ETL has moved past it.
    #[tokio::test]
    async fn processed_recent_frame_survives_retention_window() {
        let pool = make_db().await;

        let recent_ts = (chrono::Utc::now() - chrono::Duration::days(1))
            .to_rfc3339_opts(chrono::SecondsFormat::Micros, true);
        insert_frame(&pool, 1, &recent_ts, "recent processed frame").await;
        set_cursor(&pool, 1).await;

        prune_capture_tables(&pool).await.unwrap();

        let remaining: Vec<(i64,)> = sqlx::query_as("SELECT id FROM capture_frames ORDER BY id")
            .fetch_all(&pool)
            .await
            .unwrap();
        assert_eq!(
            remaining,
            vec![(1,)],
            "a recent frame must survive regardless of processed status"
        );
    }

    /// A processed row older than retention must be pruned.
    #[tokio::test]
    async fn processed_old_frame_is_pruned() {
        let pool = make_db().await;

        let old_ts = (chrono::Utc::now() - chrono::Duration::days(45))
            .to_rfc3339_opts(chrono::SecondsFormat::Micros, true);
        insert_frame(&pool, 1, &old_ts, "old processed frame").await;
        set_cursor(&pool, 1).await;

        prune_capture_tables(&pool).await.unwrap();

        let remaining: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM capture_frames")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(remaining, 0, "old, already-processed frame must be pruned");
    }

    /// capture_ui_events rows are bounded by the same cursor-frame timestamp
    /// boundary — old-and-before-cursor rows are pruned, old-but-at-or-after
    /// rows survive.
    #[tokio::test]
    async fn ui_events_bounded_by_cursor_frame_timestamp() {
        let pool = make_db().await;

        let old_ts = (chrono::Utc::now() - chrono::Duration::days(60))
            .to_rfc3339_opts(chrono::SecondsFormat::Micros, true);
        let cursor_ts = (chrono::Utc::now() - chrono::Duration::days(45))
            .to_rfc3339_opts(chrono::SecondsFormat::Micros, true);

        insert_frame(&pool, 1, &cursor_ts, "cursor frame").await;
        set_cursor(&pool, 1).await;

        insert_ui_event(&pool, &old_ts).await; // before cursor ts — must prune
        insert_ui_event(&pool, &cursor_ts).await; // at cursor ts — must survive (not strictly before)

        prune_capture_tables(&pool).await.unwrap();

        let remaining: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM capture_ui_events")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(
            remaining, 1,
            "only the ui_event strictly before the cursor timestamp must be pruned"
        );
    }

    /// No etl_cursor row (or last_frame_id == 0) means nothing has been
    /// processed yet — retention must be a no-op, never guess.
    #[tokio::test]
    async fn no_cursor_progress_means_no_pruning() {
        let pool = make_db().await;

        let old_ts = (chrono::Utc::now() - chrono::Duration::days(90))
            .to_rfc3339_opts(chrono::SecondsFormat::Micros, true);
        insert_frame(&pool, 1, &old_ts, "old frame, cursor untouched").await;
        // etl_cursor is seeded with last_frame_id = 0 by migration — leave as-is.

        prune_capture_tables(&pool).await.unwrap();

        let remaining: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM capture_frames")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(
            remaining, 1,
            "nothing processed yet — nothing must be pruned"
        );
    }
}
