// meridian — normalises screenpipe activity into structured app sessions
// https://github.com/meridiona/meridian

use anyhow::Result;
use serde::{Deserialize, Serialize};
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::SqlitePool;
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
pub struct OcrSample {
    pub text: String,
    pub window_name: Option<String>,
    pub timestamp: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ElementSample {
    pub text: String,
    pub role: Option<String>,
    pub window_name: Option<String>,
    pub timestamp: String,
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
    let rows = sqlx::query_as::<_, (i64, String, Option<String>, Option<String>, String, Option<String>)>(
        "SELECT id, app_name, window_name, browser_url, timestamp, capture_trigger
         FROM frames
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
        .map(|(id, app_name, window_name, browser_url, timestamp, capture_trigger)| FrameRow {
            id,
            app_name,
            window_name,
            browser_url,
            timestamp,
            capture_trigger,
        })
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
         FROM frames
         WHERE timestamp > ? AND timestamp <= ?",
    )
    .bind(start_ts)
    .bind(end_ts)
    .fetch_one(pool)
    .await?;
    Ok((row.0, row.1))
}

/// Returns per-context frame counts for a given app within a frame-id range,
/// sorted by count descending (top 20).
///
/// For browser apps: groups by `browser_url` (the real URL screenpipe captures),
/// falling back to `window_name` for frames where the URL is NULL.
/// For all other apps: groups by `window_name` as before.
pub async fn get_window_titles(
    pool: &SqlitePool,
    min_frame_id: i64,
    max_frame_id: i64,
    app_name: &str,
) -> Result<Vec<WindowTitleCount>> {
    let rows = if is_browser_app(app_name) {
        sqlx::query_as::<_, (String, i64)>(
            "SELECT COALESCE(browser_url, window_name) as context, COUNT(*) as count
             FROM frames
             WHERE id BETWEEN ? AND ?
               AND app_name = ?
               AND (browser_url IS NOT NULL OR (window_name IS NOT NULL AND window_name != ''))
             GROUP BY context
             ORDER BY count DESC
             LIMIT 20",
        )
        .bind(min_frame_id)
        .bind(max_frame_id)
        .bind(app_name)
        .fetch_all(pool)
        .await?
    } else {
        sqlx::query_as::<_, (String, i64)>(
            "SELECT window_name, COUNT(*) as count
             FROM frames
             WHERE id BETWEEN ? AND ?
               AND app_name = ?
               AND window_name IS NOT NULL AND window_name != ''
             GROUP BY window_name
             ORDER BY count DESC
             LIMIT 20",
        )
        .bind(min_frame_id)
        .bind(max_frame_id)
        .bind(app_name)
        .fetch_all(pool)
        .await?
    };

    Ok(rows
        .into_iter()
        .map(|(window_name, count)| WindowTitleCount { window_name, count })
        .collect())
}

fn is_browser_app(app_name: &str) -> bool {
    let lc = app_name.to_lowercase();
    ["chrome", "safari", "firefox", "arc", "edge", "brave", "opera", "vivaldi"]
        .iter()
        .any(|b| lc.contains(b))
}

/// Returns up to 10 unique OCR text samples from the given frame-id range.
/// Deduplicates on exact text — keeps the earliest occurrence of each unique string.
/// Only samples with more than 30 characters are included.
pub async fn get_ocr_samples(
    pool: &SqlitePool,
    min_frame_id: i64,
    max_frame_id: i64,
) -> Result<Vec<OcrSample>> {
    let rows = sqlx::query_as::<_, (String, Option<String>, String)>(
        "SELECT ot.text, MIN(f.window_name) AS window_name, MIN(f.timestamp) AS timestamp
         FROM ocr_text ot
         JOIN frames f ON ot.frame_id = f.id
         WHERE f.id BETWEEN ? AND ?
           AND length(ot.text) > 30
         GROUP BY ot.text
         ORDER BY timestamp
         LIMIT 10",
    )
    .bind(min_frame_id)
    .bind(max_frame_id)
    .fetch_all(pool)
    .await?;

    let result = rows
        .into_iter()
        .map(|(text, window_name, timestamp)| OcrSample {
            text,
            window_name,
            timestamp,
        })
        .collect();

    Ok(result)
}

/// Returns up to 10 unique accessibility element samples from the given frame-id range.
/// Deduplicates on (text, role) — same element appearing in multiple frames is stored once.
/// Only elements with more than 20 characters and source = 'accessibility' are included.
pub async fn get_element_samples(
    pool: &SqlitePool,
    min_frame_id: i64,
    max_frame_id: i64,
) -> Result<Vec<ElementSample>> {
    let rows = sqlx::query_as::<_, (String, Option<String>, Option<String>, String)>(
        "SELECT e.text, e.role, MIN(f.window_name) AS window_name, MIN(f.timestamp) AS timestamp
         FROM elements e
         JOIN frames f ON e.frame_id = f.id
         WHERE f.id BETWEEN ? AND ?
           AND e.text IS NOT NULL AND length(e.text) > 20
           AND e.source = 'accessibility'
         GROUP BY e.text, e.role
         ORDER BY timestamp
         LIMIT 10",
    )
    .bind(min_frame_id)
    .bind(max_frame_id)
    .fetch_all(pool)
    .await?;

    let result = rows
        .into_iter()
        .map(|(text, role, window_name, timestamp)| ElementSample {
            text,
            role,
            window_name,
            timestamp,
        })
        .collect();

    Ok(result)
}

/// Returns unique audio transcription snippets within the given timestamp range.
/// Deduplicates on exact transcription text — repeated chunks are stored once (earliest timestamp).
/// Hallucinations and very short snippets (<=10 chars) are excluded.
pub async fn get_audio_snippets(
    pool: &SqlitePool,
    start_ts: &str,
    end_ts: &str,
) -> Result<Vec<AudioSnippet>> {
    let rows = sqlx::query_as::<_, (String, String, Option<i64>)>(
        "SELECT transcription, MIN(timestamp) AS timestamp, MIN(speaker_id) AS speaker_id
         FROM audio_transcriptions
         WHERE timestamp BETWEEN ? AND ?
           AND transcription IS NOT NULL AND length(transcription) > 10
         GROUP BY transcription
         ORDER BY timestamp
         LIMIT 50",
    )
    .bind(start_ts)
    .bind(end_ts)
    .fetch_all(pool)
    .await?;

    let result = rows
        .into_iter()
        .map(|(transcription, timestamp, speaker_id)| AudioSnippet {
            transcription,
            timestamp,
            speaker_id,
        })
        .collect();

    Ok(result)
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
        "SELECT MAX(timestamp) FROM ui_events
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
         FROM ui_events
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
