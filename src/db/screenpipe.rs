//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
// https://github.com/meridiona/meridian
//! Capture-source readers for the ETL.
//!
//! **Cutover note (Gap-2 Bucket 2, slice 4b):** despite the module name, these
//! readers now query meridian's own **`capture_frames` / `capture_ui_events`**
//! tables (written in-process by the tray — see `meridian_core::capture`), not
//! screenpipe's `frames` / `ui_events`. The column layout is identical by design,
//! so the cutover was a table-name + pool change. They take the **meridian** pool
//! (source == sink now). `get_audio_snippets` is stubbed empty (Audio OFF).
//! `open_screenpipe` survives only for the health module until slice 4b-2 retires
//! it; the `db::screenpipe` → `db::capture_source` rename is deferred (pure churn).

use anyhow::Result;
use serde::{Deserialize, Serialize};
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::SqlitePool;
use std::collections::HashMap;
use std::str::FromStr;

// ---------------------------------------------------------------------------
// Structs
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FrameRow {
    pub id: i64,
    pub app_name: String,
    pub window_name: Option<String>,
    pub browser_url: Option<String>,
    pub timestamp: String,
    pub capture_trigger: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowTitleCount {
    pub window_name: String,
    pub count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioSnippet {
    pub transcription: String,
    pub timestamp: String,
    pub speaker_id: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignalEvent {
    /// 'clipboard' or 'app_switch'
    pub event_type: String,
    /// text_content for clipboard; app_name for app_switch
    pub value: Option<String>,
    pub timestamp: String,
}

/// One frame's text content fetched from screenpipe for session_text building.
#[derive(Debug, Clone)]
pub struct FrameText {
    pub frame_id: i64,
    pub timestamp: String,
    pub full_text: String,
    pub text_source: String,
}

// ---------------------------------------------------------------------------
// Connection
// ---------------------------------------------------------------------------

/// Opens the screenpipe SQLite database at `path` in READ-ONLY mode.
/// The pool will never issue any write to the database.
pub async fn open_screenpipe(path: &str) -> Result<SqlitePool> {
    let opts = SqliteConnectOptions::from_str(path)?.read_only(true);
    let pool = SqlitePool::connect_with(opts).await?;
    Ok(pool)
}

// ---------------------------------------------------------------------------
// Query helpers
// ---------------------------------------------------------------------------

/// Returns frames recorded after `after_frame_id`, ordered oldest-first,
/// capped at `limit` rows.  Only rows with a non-empty `app_name` are returned.
pub async fn get_frames_since(
    pool: &SqlitePool,
    after_frame_id: i64,
    limit: i64,
) -> Result<Vec<FrameRow>> {
    let rows = sqlx::query_as::<
        _,
        (
            i64,
            String,
            Option<String>,
            Option<String>,
            String,
            Option<String>,
        ),
    >(
        "SELECT id, app_name, window_name, browser_url, timestamp, capture_trigger
         FROM capture_frames
         WHERE id > ? AND app_name IS NOT NULL AND app_name != ''
         ORDER BY id ASC
         LIMIT ?",
    )
    .bind(after_frame_id)
    .bind(limit)
    .fetch_all(pool)
    .await?;

    let result = rows
        .into_iter()
        .map(
            |(id, app_name, window_name, browser_url, timestamp, capture_trigger)| FrameRow {
                id,
                app_name,
                window_name,
                browser_url,
                timestamp,
                capture_trigger,
            },
        )
        .collect();

    Ok(result)
}

/// Counts ALL frames (including null-app) within a timestamp window.
/// Returns (total_count, idle_count).
/// Used to classify whether a gap was user-idle-while-awake or system-sleep.
pub async fn count_frames_in_window(
    pool: &SqlitePool,
    start_ts: &str,
    end_ts: &str,
) -> Result<(i64, i64)> {
    let row = sqlx::query_as::<_, (i64, i64)>(
        "SELECT COUNT(*),
                COALESCE(SUM(CASE WHEN capture_trigger = 'idle' THEN 1 ELSE 0 END), 0)
         FROM capture_frames
         WHERE timestamp > ? AND timestamp <= ?",
    )
    .bind(start_ts)
    .bind(end_ts)
    .fetch_one(pool)
    .await?;
    Ok((row.0, row.1))
}

/// Returns per-context frame counts for a given app within a frame-id range,
/// sorted by count descending.
///
/// For browser apps: groups by `browser_url` (the real URL screenpipe captures),
/// falling back to `window_name` for frames where the URL is NULL.
/// For all other apps: groups by `window_name`, trimmed and with the trailing
/// " — AppName" OS suffix stripped so dynamic titles collapse correctly.
pub async fn get_window_titles(
    pool: &SqlitePool,
    min_frame_id: i64,
    max_frame_id: i64,
    app_name: &str,
) -> Result<Vec<WindowTitleCount>> {
    let rows = if is_browser_app(app_name) {
        // Fetch raw URLs grouped by full URL, then aggregate by domain in Rust.
        // SQLite has no URL-parsing functions, and we want localhost:3939/ and
        // localhost:3939/sessions to count as one entry, not two.
        let raw = sqlx::query_as::<_, (String, i64)>(
            "SELECT COALESCE(browser_url, window_name) as context, COUNT(*) as count
             FROM capture_frames
             WHERE id BETWEEN ? AND ?
               AND app_name = ? COLLATE NOCASE
               AND (browser_url IS NOT NULL OR (window_name IS NOT NULL AND window_name != ''))
             GROUP BY context",
        )
        .bind(min_frame_id)
        .bind(max_frame_id)
        .bind(app_name)
        .fetch_all(pool)
        .await?;

        let mut by_domain: HashMap<String, i64> = HashMap::new();
        for (url, count) in raw {
            *by_domain.entry(url_domain(&url).to_owned()).or_insert(0) += count;
        }
        let mut aggregated: Vec<(String, i64)> = by_domain.into_iter().collect();
        aggregated.sort_by(|a, b| b.1.cmp(&a.1));
        aggregated
    } else {
        // TRIM in GROUP BY so "Title " and "Title" collapse into one bucket at
        // the SQL level before Rust normalization strips the app-name suffix.
        sqlx::query_as::<_, (String, i64)>(
            "SELECT TRIM(window_name) as window_name, COUNT(*) as count
             FROM capture_frames
             WHERE id BETWEEN ? AND ?
               AND app_name = ? COLLATE NOCASE
               AND window_name IS NOT NULL AND TRIM(window_name) != ''
             GROUP BY TRIM(window_name)
             ORDER BY count DESC",
        )
        .bind(min_frame_id)
        .bind(max_frame_id)
        .bind(app_name)
        .fetch_all(pool)
        .await?
    };

    Ok(rows
        .into_iter()
        .map(|(window_name, count)| WindowTitleCount {
            window_name: normalize_window_title(window_name, app_name),
            count,
        })
        .collect())
}

/// Normalize a window title:
///   1. Trim leading/trailing whitespace.
///   2. Strip the trailing " — AppName" OS suffix (case-insensitive) so that
///      "runner.rs — meridian — Antigravity" and "extractor.rs — Antigravity"
///      both collapse to their file/project portion instead of being one unique
///      entry per title string.
///   3. Reduce URL-shaped titles to their domain.
fn normalize_window_title(title: String, app_name: &str) -> String {
    let title = title.trim();

    // Strip trailing " — <app_name>" suffix (the macOS window title convention).
    let suffix = format!(" \u{2014} {}", app_name);
    let title = if let Some(stripped) = title
        .to_lowercase()
        .rfind(&suffix.to_lowercase())
        .map(|pos| title[..pos].trim())
    {
        stripped
    } else {
        title
    };

    if title.starts_with("https://") || title.starts_with("http://") {
        url_domain(title).to_owned()
    } else {
        title.to_owned()
    }
}

fn url_domain(url: &str) -> &str {
    let without_scheme = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url);
    let domain = without_scheme.split('/').next().unwrap_or(without_scheme);
    domain.strip_prefix("www.").unwrap_or(domain)
}

fn is_browser_app(app_name: &str) -> bool {
    let lc = app_name.to_lowercase();
    [
        "chrome", "safari", "firefox", "arc", "edge", "brave", "opera", "vivaldi",
    ]
    .iter()
    .any(|b| lc.contains(b))
}

/// Audio transcription snippets — **stubbed empty since the slice-4b cutover.**
///
/// In-process capture is text-only (Audio OFF — the privacy "we never record
/// audio" property), so there is no audio source. Kept (rather than removed) so
/// [`crate::etl::extractor`]'s `extract_block_context` and the session shape are
/// unchanged; the returned `audio_snippets` is simply always empty. Revisit only
/// if in-process audio capture is ever added.
pub async fn get_audio_snippets(
    _pool: &SqlitePool,
    _start_ts: &str,
    _end_ts: &str,
) -> Result<Vec<AudioSnippet>> {
    Ok(Vec::new())
}

/// Returns the timestamp of the last user-interaction ui_event for `app_name`
/// in the half-open window (after_ts, before_ts). Returns None if not found.
pub async fn get_last_ui_event_for_app(
    pool: &SqlitePool,
    app_name: &str,
    after_ts: &str,
    before_ts: &str,
) -> anyhow::Result<Option<String>> {
    let row: Option<(Option<String>,)> = sqlx::query_as(
        "SELECT MAX(timestamp) FROM capture_ui_events
         WHERE app_name = ?1
           AND event_type IN ('click', 'key', 'text')
           AND timestamp > ?2
           AND timestamp < ?3",
    )
    .bind(app_name)
    .bind(after_ts)
    .bind(before_ts)
    .fetch_optional(pool)
    .await?;
    Ok(row.and_then(|(ts,)| ts.filter(|s| !s.is_empty())))
}

/// Returns all frames in [min_frame_id, max_frame_id] that have non-empty text,
/// ordered oldest-first.  Falls back to accessibility_text when full_text is NULL
/// (e.g. frames inserted via non-standard paths like capture_trigger='claude_session').
pub async fn get_frame_full_texts(
    pool: &SqlitePool,
    min_frame_id: i64,
    max_frame_id: i64,
) -> Result<Vec<FrameText>> {
    let rows = sqlx::query_as::<_, (i64, String, String, String)>(
        "SELECT id, timestamp, COALESCE(full_text, accessibility_text), COALESCE(text_source, 'ocr')
         FROM capture_frames
         WHERE id BETWEEN ?1 AND ?2
           AND COALESCE(full_text, accessibility_text) IS NOT NULL
           AND COALESCE(full_text, accessibility_text) != ''
         ORDER BY id ASC",
    )
    .bind(min_frame_id)
    .bind(max_frame_id)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|(frame_id, timestamp, full_text, text_source)| FrameText {
            frame_id,
            timestamp,
            full_text,
            text_source,
        })
        .collect())
}

/// Returns clipboard and app_switch UI events within the given timestamp range.
/// Clipboard events are deduplicated by value — same text copied multiple times is stored once.
/// For clipboard events `value` is `text_content`; for app_switch it is `app_name`.
pub async fn get_signals(
    pool: &SqlitePool,
    start_ts: &str,
    end_ts: &str,
) -> Result<Vec<SignalEvent>> {
    let rows = sqlx::query_as::<_, (String, Option<String>, Option<String>, String)>(
        "SELECT event_type, text_content, app_name, MIN(timestamp) AS timestamp
         FROM capture_ui_events
         WHERE timestamp BETWEEN ? AND ?
           AND event_type IN ('clipboard', 'app_switch')
           AND (text_content IS NOT NULL OR app_name IS NOT NULL)
         GROUP BY event_type, COALESCE(text_content, app_name)
         ORDER BY timestamp",
    )
    .bind(start_ts)
    .bind(end_ts)
    .fetch_all(pool)
    .await?;

    let result = rows
        .into_iter()
        .map(|(event_type, text_content, app_name, timestamp)| {
            // clipboard → text_content; app_switch → app_name
            let value = if event_type == "clipboard" {
                text_content
            } else {
                app_name
            };
            SignalEvent {
                event_type,
                value,
                timestamp,
            }
        })
        .collect();

    Ok(result)
}
