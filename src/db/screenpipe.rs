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
    pub timestamp: String,
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
    let rows = sqlx::query_as::<_, (i64, String, Option<String>, String)>(
        "SELECT id, app_name, window_name, timestamp
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
        .map(|(id, app_name, window_name, timestamp)| FrameRow {
            id,
            app_name,
            window_name,
            timestamp,
        })
        .collect();

    Ok(result)
}

/// Returns per-window-title frame counts for a given app within a frame-id
/// range, sorted by count descending (top 20).
pub async fn get_window_titles(
    pool: &SqlitePool,
    min_frame_id: i64,
    max_frame_id: i64,
    app_name: &str,
) -> Result<Vec<WindowTitleCount>> {
    let rows = sqlx::query_as::<_, (String, i64)>(
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
    .await?;

    let result = rows
        .into_iter()
        .map(|(window_name, count)| WindowTitleCount { window_name, count })
        .collect();

    Ok(result)
}

/// Returns up to 10 OCR text samples from the given frame-id range.
/// Only samples with more than 30 characters are included.
pub async fn get_ocr_samples(
    pool: &SqlitePool,
    min_frame_id: i64,
    max_frame_id: i64,
) -> Result<Vec<OcrSample>> {
    let rows = sqlx::query_as::<_, (String, Option<String>, String)>(
        "SELECT ot.text, f.window_name, f.timestamp
         FROM ocr_text ot
         JOIN frames f ON ot.frame_id = f.id
         WHERE f.id BETWEEN ? AND ?
           AND length(ot.text) > 30
         ORDER BY f.timestamp
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

/// Returns up to 10 accessibility element samples from the given frame-id range.
/// Only elements with more than 20 characters and source = 'accessibility' are included.
pub async fn get_element_samples(
    pool: &SqlitePool,
    min_frame_id: i64,
    max_frame_id: i64,
) -> Result<Vec<ElementSample>> {
    let rows = sqlx::query_as::<_, (String, Option<String>, Option<String>, String)>(
        "SELECT e.text, e.role, f.window_name, f.timestamp
         FROM elements e
         JOIN frames f ON e.frame_id = f.id
         WHERE f.id BETWEEN ? AND ?
           AND e.text IS NOT NULL AND length(e.text) > 20
           AND e.source = 'accessibility'
         ORDER BY f.timestamp
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

/// Returns audio transcription snippets within the given timestamp range.
/// Hallucinations and very short snippets (<=10 chars) are excluded.
pub async fn get_audio_snippets(
    pool: &SqlitePool,
    start_ts: &str,
    end_ts: &str,
) -> Result<Vec<AudioSnippet>> {
    let rows = sqlx::query_as::<_, (String, String, Option<i64>)>(
        "SELECT transcription, timestamp, speaker_id
         FROM audio_transcriptions
         WHERE timestamp BETWEEN ? AND ?
           AND transcription IS NOT NULL AND length(transcription) > 10
         ORDER BY timestamp",
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

/// Returns clipboard and app_switch UI events within the given timestamp range.
/// For clipboard events `value` is `text_content`; for app_switch it is `app_name`.
pub async fn get_signals(
    pool: &SqlitePool,
    start_ts: &str,
    end_ts: &str,
) -> Result<Vec<SignalEvent>> {
    let rows = sqlx::query_as::<_, (String, Option<String>, Option<String>, String)>(
        "SELECT event_type, text_content, app_name, timestamp
         FROM ui_events
         WHERE timestamp BETWEEN ? AND ?
           AND event_type IN ('clipboard', 'app_switch')
           AND (text_content IS NOT NULL OR app_name IS NOT NULL)
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
