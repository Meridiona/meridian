// meridian — normalises screenpipe activity into structured app sessions
// https://github.com/meridiona/meridian

use anyhow::Result;
use sqlx::SqlitePool;

use crate::db::screenpipe::{
    get_audio_snippets, get_element_samples, get_ocr_samples, get_signals, get_window_titles,
    AudioSnippet, ElementSample, OcrSample, SignalEvent, WindowTitleCount,
};

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
    pub ocr_samples: Vec<OcrSample>,
    pub elements_samples: Vec<ElementSample>,
    pub audio_snippets: Vec<AudioSnippet>,
    pub signals: Vec<SignalEvent>,
}

// ---------------------------------------------------------------------------
// extract_block_context
// ---------------------------------------------------------------------------

/// Fetches all enrichment data for a single contiguous block of frames in
/// parallel.  All five screenpipe query functions are called concurrently via
/// `tokio::join!`.
pub async fn extract_block_context(
    screenpipe: &SqlitePool,
    app_name: &str,
    started_at: &str,
    ended_at: &str,
    min_frame_id: i64,
    max_frame_id: i64,
    frame_count: i64,
) -> Result<BlockContext> {
    // All five reads are independent — fire them all at once.
    let (window_titles_res, ocr_res, elements_res, audio_res, signals_res) = tokio::join!(
        get_window_titles(screenpipe, min_frame_id, max_frame_id, app_name),
        get_ocr_samples(screenpipe, min_frame_id, max_frame_id),
        get_element_samples(screenpipe, min_frame_id, max_frame_id),
        get_audio_snippets(screenpipe, started_at, ended_at),
        get_signals(screenpipe, started_at, ended_at),
    );

    Ok(BlockContext {
        app_name: app_name.to_owned(),
        started_at: started_at.to_owned(),
        ended_at: ended_at.to_owned(),
        min_frame_id,
        max_frame_id,
        frame_count,
        window_titles: window_titles_res?,
        ocr_samples: ocr_res?,
        elements_samples: elements_res?,
        audio_snippets: audio_res?,
        signals: signals_res?,
    })
}
