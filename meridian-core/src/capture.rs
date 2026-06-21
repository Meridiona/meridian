//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Capture-frame writer for `meridian.db` (Gap-2 Bucket 2, slice 4a).
//!
//! **Inverted ownership.** Unlike every other `meridian.db` table, `capture_frames`
//! is *written by the tray* (the in-process capture engine — see
//! `tray/src-tauri/src/capture/`) and *read by the daemon's* ETL. This module is
//! the write half; the read half is the slice-4b repoint of the screenpipe
//! readers in `src/db/screenpipe.rs`. Nobody else writes this table — do not
//! assume the daemon owns it.
//!
//! **Schema contract.** The column layout (migration `046_capture_frames.sql`)
//! mirrors the read-subset of screenpipe's `frames` table so the 4b repoint is a
//! mechanical `FROM frames` → `FROM capture_frames` change. The one mapping that
//! matters: screenpipe splits extracted text across `full_text` (OCR) and
//! `accessibility_text` (a11y-tree) and the reader resolves it with
//! `COALESCE(full_text, accessibility_text)`. [`insert_capture_frame`] reproduces
//! that split — exactly one of the two columns is populated per row.
//!
//! # Who calls this
//! `tray/src-tauri/src/lib.rs`'s capture consumer task, once per captured frame.
//!
//! # Related
//! - [`crate::open_existing`] — the (read-write) pool the tray passes in.
//! - `src/db/screenpipe.rs` (daemon) — the reader this table will feed in slice 4b.

use anyhow::{Context, Result};
use chrono::{DateTime, SecondsFormat, Utc};
use sqlx::SqlitePool;

/// One captured frame to persist. `text` + `text_source` map onto screenpipe's
/// split text columns (see the module docs): `"accessibility"` →
/// `accessibility_text` (with `full_text` NULL), anything else → `full_text`.
#[derive(Debug, Clone)]
pub struct CaptureFrameInsert {
    /// Capture instant. Stored as RFC3339 UTC, microsecond precision.
    pub timestamp: DateTime<Utc>,
    /// Foreground app name, if known (the reader filters NULL/empty itself).
    pub app_name: Option<String>,
    /// Foreground window title, if known.
    pub window_name: Option<String>,
    /// Active browser URL when the foreground app is a browser, if detected.
    pub browser_url: Option<String>,
    /// Extracted text (a11y-tree or OCR).
    pub text: String,
    /// `"ocr"` | `"accessibility"` — selects which text column `text` lands in.
    pub text_source: String,
}

/// Insert one captured frame into `capture_frames`.
///
/// Degrades gracefully when the table is absent — an older daemon that hasn't
/// applied migration 046 yet, or a tray running against a pre-046 `meridian.db`
/// in dev: the frame is dropped with a debug log and `Ok` is returned, so a
/// schema lag never crashes the tray's capture loop. All other errors propagate.
pub async fn insert_capture_frame(pool: &SqlitePool, frame: &CaptureFrameInsert) -> Result<()> {
    // Mirror screenpipe's split text columns: exactly one is populated, so the
    // reader's COALESCE(full_text, accessibility_text) returns this row's text.
    let (full_text, accessibility_text) = if frame.text_source == "accessibility" {
        (None, Some(frame.text.as_str()))
    } else {
        (Some(frame.text.as_str()), None)
    };
    let ts = frame.timestamp.to_rfc3339_opts(SecondsFormat::Micros, true);

    let res = sqlx::query(
        "INSERT INTO capture_frames
           (timestamp, app_name, window_name, browser_url, full_text, accessibility_text, text_source)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
    )
    .bind(&ts)
    .bind(&frame.app_name)
    .bind(&frame.window_name)
    .bind(&frame.browser_url)
    .bind(full_text)
    .bind(accessibility_text)
    .bind(&frame.text_source)
    .execute(pool)
    .await;

    match res {
        Ok(_) => Ok(()),
        Err(e) if is_missing_table(&e) => {
            tracing::debug!(
                "capture_frames absent (daemon not yet migrated to 046) — frame dropped"
            );
            Ok(())
        }
        Err(e) => Err(e).context("insert capture_frame"),
    }
}

/// One input event to persist into `capture_ui_events`. Maps onto the
/// read-subset of screenpipe's `ui_events`: `get_signals` (clipboard/app_switch)
/// and `get_last_ui_event_for_app` (click/key/text timestamps). `text_content`
/// is set only for `"clipboard"` — every other type carries no typed text.
#[derive(Debug, Clone)]
pub struct CaptureUiEventInsert {
    /// Event instant. Stored as RFC3339 UTC, microsecond precision.
    pub timestamp: DateTime<Utc>,
    /// `"click"` | `"key"` | `"text"` | `"app_switch"` | `"window_focus"` | `"clipboard"`.
    pub event_type: String,
    /// App the event belongs to (for `app_switch`, the activated app).
    pub app_name: Option<String>,
    /// Clipboard text preview (truncated/filtered upstream); NULL otherwise.
    pub text_content: Option<String>,
}

/// Insert one input event into `capture_ui_events`. Same graceful missing-table
/// behaviour as [`insert_capture_frame`] (a schema lag never crashes the tray).
pub async fn insert_capture_ui_event(pool: &SqlitePool, ev: &CaptureUiEventInsert) -> Result<()> {
    let ts = ev.timestamp.to_rfc3339_opts(SecondsFormat::Micros, true);
    let res = sqlx::query(
        "INSERT INTO capture_ui_events (timestamp, event_type, app_name, text_content)
         VALUES (?1, ?2, ?3, ?4)",
    )
    .bind(&ts)
    .bind(&ev.event_type)
    .bind(&ev.app_name)
    .bind(&ev.text_content)
    .execute(pool)
    .await;

    match res {
        Ok(_) => Ok(()),
        Err(e) if is_missing_table(&e) => {
            tracing::debug!(
                "capture_ui_events absent (daemon not yet migrated to 047) — event dropped"
            );
            Ok(())
        }
        Err(e) => Err(e).context("insert capture_ui_event"),
    }
}

/// True when the error is SQLite's "no such table" — the schema-lag case
/// [`insert_capture_frame`] / [`insert_capture_ui_event`] swallow.
fn is_missing_table(e: &sqlx::Error) -> bool {
    matches!(e, sqlx::Error::Database(db) if db.message().contains("no such table"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;

    /// In-memory pool with the migration-046 DDL applied inline. The daemon's
    /// `tests/integration_etl.rs` (which runs the real migrations) is the schema
    /// guard once 4b reads the table; here we only need the columns to exist.
    async fn mem_pool() -> SqlitePool {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE capture_frames (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                timestamp TEXT NOT NULL,
                app_name TEXT, window_name TEXT, browser_url TEXT,
                full_text TEXT, accessibility_text TEXT, text_source TEXT,
                capture_trigger TEXT
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    fn frame(text_source: &str, text: &str) -> CaptureFrameInsert {
        CaptureFrameInsert {
            timestamp: DateTime::parse_from_rfc3339("2026-06-21T17:12:11.038283Z")
                .unwrap()
                .with_timezone(&Utc),
            app_name: Some("Code".into()),
            window_name: Some("foo.rs".into()),
            browser_url: None,
            text: text.into(),
            text_source: text_source.into(),
        }
    }

    /// a11y frame → accessibility_text populated, full_text NULL, and the
    /// reader's COALESCE picks the a11y text. This is the mapping 4b depends on.
    #[tokio::test]
    async fn accessibility_frame_lands_in_accessibility_text() {
        let pool = mem_pool().await;
        insert_capture_frame(&pool, &frame("accessibility", "fn main() {}"))
            .await
            .unwrap();

        let (full, a11y, src): (Option<String>, Option<String>, Option<String>) = sqlx::query_as(
            "SELECT full_text, accessibility_text, text_source FROM capture_frames WHERE id = 1",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(full, None, "full_text must be NULL for an a11y frame");
        assert_eq!(a11y.as_deref(), Some("fn main() {}"));
        assert_eq!(src.as_deref(), Some("accessibility"));

        let coalesced: String = sqlx::query_scalar(
            "SELECT COALESCE(full_text, accessibility_text) FROM capture_frames",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(coalesced, "fn main() {}");
    }

    /// OCR frame → full_text populated, accessibility_text NULL.
    #[tokio::test]
    async fn ocr_frame_lands_in_full_text() {
        let pool = mem_pool().await;
        insert_capture_frame(&pool, &frame("ocr", "$ ls"))
            .await
            .unwrap();

        let (full, a11y): (Option<String>, Option<String>) =
            sqlx::query_as("SELECT full_text, accessibility_text FROM capture_frames WHERE id = 1")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(full.as_deref(), Some("$ ls"));
        assert_eq!(
            a11y, None,
            "accessibility_text must be NULL for an OCR frame"
        );
    }

    /// Timestamp is stored at fixed microsecond precision with a 'Z' offset.
    #[tokio::test]
    async fn timestamp_stored_fixed_precision_utc() {
        let pool = mem_pool().await;
        insert_capture_frame(&pool, &frame("ocr", "x"))
            .await
            .unwrap();
        let ts: String = sqlx::query_scalar("SELECT timestamp FROM capture_frames WHERE id = 1")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(ts, "2026-06-21T17:12:11.038283Z");
    }

    /// Missing table (schema lag) is swallowed — capture must not crash the tray.
    #[tokio::test]
    async fn missing_table_is_swallowed() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        // No CREATE TABLE — table is absent on purpose.
        insert_capture_frame(&pool, &frame("ocr", "x"))
            .await
            .expect("missing table must return Ok, not Err");
    }

    // ── capture_ui_events ───────────────────────────────────────────────────

    /// In-memory pool with the migration-047 DDL applied inline.
    async fn mem_pool_ui() -> SqlitePool {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE capture_ui_events (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                timestamp TEXT NOT NULL,
                event_type TEXT NOT NULL,
                app_name TEXT,
                text_content TEXT
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    fn ui_event(
        ts: &str,
        event_type: &str,
        app: Option<&str>,
        text: Option<&str>,
    ) -> CaptureUiEventInsert {
        CaptureUiEventInsert {
            timestamp: DateTime::parse_from_rfc3339(ts)
                .unwrap()
                .with_timezone(&Utc),
            event_type: event_type.into(),
            app_name: app.map(Into::into),
            text_content: text.map(Into::into),
        }
    }

    /// Clipboard → text_content; app_switch → app_name. Verified by running the
    /// DAEMON's actual `get_signals` query body against `capture_ui_events` (the
    /// 4b repoint target), so this proves the write satisfies the read.
    #[tokio::test]
    async fn get_signals_query_resolves_clipboard_and_app_switch() {
        let pool = mem_pool_ui().await;
        // clipboard carries the copied text; app_switch carries the activated app.
        insert_capture_ui_event(
            &pool,
            &ui_event(
                "2026-06-21T10:00:00.000000Z",
                "clipboard",
                Some("Code"),
                Some("copied snippet"),
            ),
        )
        .await
        .unwrap();
        insert_capture_ui_event(
            &pool,
            &ui_event(
                "2026-06-21T10:00:01.000000Z",
                "app_switch",
                Some("Slack"),
                None,
            ),
        )
        .await
        .unwrap();
        // click is NOT a signal — must be excluded by the event_type filter.
        insert_capture_ui_event(
            &pool,
            &ui_event("2026-06-21T10:00:02.000000Z", "click", Some("Code"), None),
        )
        .await
        .unwrap();

        // Verbatim get_signals body (src/db/screenpipe.rs) with FROM repointed.
        let rows: Vec<(String, Option<String>, Option<String>, String)> = sqlx::query_as(
            "SELECT event_type, text_content, app_name, MIN(timestamp) AS timestamp
             FROM capture_ui_events
             WHERE timestamp BETWEEN ? AND ?
               AND event_type IN ('clipboard', 'app_switch')
               AND (text_content IS NOT NULL OR app_name IS NOT NULL)
             GROUP BY event_type, COALESCE(text_content, app_name)
             ORDER BY timestamp",
        )
        .bind("2026-06-21T00:00:00.000000Z")
        .bind("2026-06-21T23:59:59.000000Z")
        .fetch_all(&pool)
        .await
        .unwrap();

        assert_eq!(
            rows.len(),
            2,
            "click must be filtered out; clipboard + app_switch remain"
        );
        // value = text_content for clipboard, app_name for app_switch (the reader's mapping).
        assert_eq!(rows[0].0, "clipboard");
        assert_eq!(rows[0].1.as_deref(), Some("copied snippet"));
        assert_eq!(rows[1].0, "app_switch");
        assert_eq!(rows[1].2.as_deref(), Some("Slack"));
    }

    /// `get_last_ui_event_for_app` returns the latest click/key/text timestamp
    /// for an app — proving the timestamp-only rows (no typed text) suffice.
    #[tokio::test]
    async fn get_last_ui_event_query_returns_latest_interaction() {
        let pool = mem_pool_ui().await;
        insert_capture_ui_event(
            &pool,
            &ui_event("2026-06-21T10:00:00.000000Z", "text", Some("Code"), None),
        )
        .await
        .unwrap();
        insert_capture_ui_event(
            &pool,
            &ui_event("2026-06-21T10:05:00.000000Z", "click", Some("Code"), None),
        )
        .await
        .unwrap();
        // A different app + a clipboard (not in the click/key/text set) must be ignored.
        insert_capture_ui_event(
            &pool,
            &ui_event("2026-06-21T10:09:00.000000Z", "click", Some("Slack"), None),
        )
        .await
        .unwrap();
        insert_capture_ui_event(
            &pool,
            &ui_event(
                "2026-06-21T10:08:00.000000Z",
                "clipboard",
                Some("Code"),
                Some("x"),
            ),
        )
        .await
        .unwrap();

        // Verbatim get_last_ui_event_for_app body with FROM repointed.
        let last: Option<(Option<String>,)> = sqlx::query_as(
            "SELECT MAX(timestamp) FROM capture_ui_events
             WHERE app_name = ?1
               AND event_type IN ('click', 'key', 'text')
               AND timestamp > ?2
               AND timestamp < ?3",
        )
        .bind("Code")
        .bind("2026-06-21T09:00:00.000000Z")
        .bind("2026-06-21T11:00:00.000000Z")
        .fetch_optional(&pool)
        .await
        .unwrap();
        let last = last.and_then(|(t,)| t);
        assert_eq!(
            last.as_deref(),
            Some("2026-06-21T10:05:00.000000Z"),
            "latest click/text for Code"
        );
    }

    /// Missing table is swallowed for ui events too.
    #[tokio::test]
    async fn ui_event_missing_table_is_swallowed() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        insert_capture_ui_event(
            &pool,
            &ui_event("2026-06-21T10:00:00.000000Z", "click", Some("Code"), None),
        )
        .await
        .expect("missing table must return Ok, not Err");
    }
}
