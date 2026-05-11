// meridian — normalises screenpipe activity into structured app sessions
// https://github.com/meridiona/meridian

use anyhow::Result;
use sqlx::SqlitePool;

use crate::db::screenpipe::{
    get_audio_snippets, get_frame_full_texts, get_signals, get_window_titles, AudioSnippet,
    SignalEvent, WindowTitleCount,
};
use crate::etl::text_merge::build_session_text;

// ---------------------------------------------------------------------------
// BlockContext
// ---------------------------------------------------------------------------

/// All enrichment data gathered for one contiguous block of frames belonging
/// to a single app.
pub struct BlockContext {
    pub app_name: String,
    pub started_at: String,
    pub ended_at: String,
    pub min_frame_id: i64,
    pub max_frame_id: i64,
    pub frame_count: i64,
    pub window_titles: Vec<WindowTitleCount>,
    pub audio_snippets: Vec<AudioSnippet>,
    pub signals: Vec<SignalEvent>,
    /// Deduplicated, timestamped union of all frame full_text for this block.
    pub session_text: String,
}

// ---------------------------------------------------------------------------
// extract_block_context
// ---------------------------------------------------------------------------

/// Fetches all enrichment data for a single contiguous block of frames in
/// parallel.  All three screenpipe query functions are called concurrently via
/// `tokio::join!`.
#[tracing::instrument(
    skip_all,
    fields(
        app_name = %app_name,
        min_frame_id,
        max_frame_id,
        frame_count,
        ocr_sample_count = tracing::field::Empty,
        audio_snippet_count = tracing::field::Empty,
    )
)]
pub async fn extract_block_context(
    screenpipe: &SqlitePool,
    app_name: &str,
    started_at: &str,
    ended_at: &str,
    min_frame_id: i64,
    max_frame_id: i64,
    frame_count: i64,
) -> Result<BlockContext> {
    let (window_titles_res, audio_res, signals_res, frames_res) = tokio::join!(
        get_window_titles(screenpipe, min_frame_id, max_frame_id, app_name),
        get_audio_snippets(screenpipe, started_at, ended_at),
        get_signals(screenpipe, started_at, ended_at),
        get_frame_full_texts(screenpipe, min_frame_id, max_frame_id),
    );

    let session_text = build_session_text(&frames_res?);
    let audio_snippets = audio_res?;

    tracing::Span::current().record("ocr_sample_count", ocr_samples.len());
    tracing::Span::current().record("audio_snippet_count", audio_snippets.len());

    Ok(BlockContext {
        app_name: app_name.to_owned(),
        started_at: started_at.to_owned(),
        ended_at: ended_at.to_owned(),
        min_frame_id,
        max_frame_id,
        frame_count,
        window_titles: window_titles_res?,
        audio_snippets,
        signals: signals_res?,
        session_text,
    })
}
