//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
// https://github.com/meridiona/meridian

use anyhow::Result;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

use crate::db::screenpipe::{
    get_audio_snippets, get_frame_full_texts, get_signals, get_window_titles, AudioSnippet,
    FrameText, SignalEvent, WindowTitleCount,
};
use crate::etl::text_merge::build_session_text;

/// Max number of per-frame provenance entries stored on a session. A long
/// session can span thousands of frames; keep the stored list (and the
/// `contributing_frames` span emitted from it) bounded. `frame_count` and
/// `min/max_frame_id` still record the full untruncated window.
pub const FRAME_CONTRIBUTION_CAP: usize = 300;

// ---------------------------------------------------------------------------
// FrameContribution
// ---------------------------------------------------------------------------

/// One screenpipe frame whose OCR / accessibility text actually fed a session's
/// `session_text`. Persisted as a JSON array on the session row so the frame
/// provenance survives screenpipe pruning and is visible in the classification
/// trace without screenpipe access.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FrameContribution {
    pub frame_id: i64,
    pub timestamp: String,
    /// "ocr" or "accessibility" — which capture stream supplied the text.
    pub text_source: String,
    /// Character count of this frame's raw text (pre-dedup) — a rough measure of
    /// how much it contributed.
    pub chars: i64,
}

impl FrameContribution {
    fn from_frame(f: &FrameText) -> Self {
        Self {
            frame_id: f.frame_id,
            timestamp: f.timestamp.clone(),
            text_source: f.text_source.clone(),
            chars: f.full_text.chars().count() as i64,
        }
    }
}

/// Build the capped per-frame provenance list (oldest-first, the order
/// `get_frame_full_texts` returns) from the frames that carried text.
pub fn build_frame_contributions(frames: &[FrameText]) -> Vec<FrameContribution> {
    frames
        .iter()
        .take(FRAME_CONTRIBUTION_CAP)
        .map(FrameContribution::from_frame)
        .collect()
}

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
    /// JSON array of [`FrameContribution`] — the screenpipe frames whose text
    /// fed `session_text`, capped at [`FRAME_CONTRIBUTION_CAP`]. `"[]"` when no
    /// frame carried text.
    pub frame_contributions: String,
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
        window_title_count = tracing::field::Empty,
        audio_snippet_count = tracing::field::Empty,
        signal_count = tracing::field::Empty,
        session_text_bytes = tracing::field::Empty,
        frames_with_text = tracing::field::Empty,
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

    let frames = frames_res?;
    // Per-frame provenance: which screenpipe frames carried the text that became
    // session_text. Built before build_session_text dedups/merges them — these
    // are the source frames, captured now because screenpipe may prune them
    // before classification.
    let frame_contributions_vec = build_frame_contributions(&frames);
    let frame_contributions =
        serde_json::to_string(&frame_contributions_vec).unwrap_or_else(|_| "[]".to_owned());
    let session_text = build_session_text(&frames);
    let audio_snippets = audio_res?;
    let window_titles = window_titles_res?;
    let signals = signals_res?;

    tracing::Span::current().record("window_title_count", window_titles.len());
    tracing::Span::current().record("audio_snippet_count", audio_snippets.len());
    tracing::Span::current().record("signal_count", signals.len());
    tracing::Span::current().record("session_text_bytes", session_text.len());
    tracing::Span::current().record("frames_with_text", frames.len());

    tracing::debug!(
        app_name,
        window_titles = window_titles.len(),
        audio_snippets = audio_snippets.len(),
        signals = signals.len(),
        session_text_bytes = session_text.len(),
        "block context extracted"
    );

    Ok(BlockContext {
        app_name: app_name.to_owned(),
        started_at: started_at.to_owned(),
        ended_at: ended_at.to_owned(),
        min_frame_id,
        max_frame_id,
        frame_count,
        window_titles,
        audio_snippets,
        signals,
        session_text,
        frame_contributions,
    })
}
