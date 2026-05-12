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

/// Keeps only the last AXTextArea element and drops all earlier ones.
/// AXTextArea in terminal apps is the full scroll buffer — each frame is a
/// superset of the previous, so only the final snapshot is needed.
/// SQL orders by timestamp ASC so the last entry in the vec is the most recent.
fn dedup_ax_textarea(mut elements: Vec<ElementSample>) -> Vec<ElementSample> {
    let last = elements
        .iter()
        .rfind(|e| e.role.as_deref() == Some("AXTextArea"))
        .cloned();
    elements.retain(|e| e.role.as_deref() != Some("AXTextArea"));
    if let Some(textarea) = last {
        elements.push(textarea);
    }
    elements
}

/// If screenpipe reports "Terminal" but the OCR/element text contains
/// unambiguous Antigravity IDE fingerprints, return the corrected name.
/// Only triggers on high-confidence signals to avoid false positives.
fn reclassify_terminal<'a>(
    app_name: &'a str,
    ocr_samples: &[OcrSample],
    elements_samples: &[ElementSample],
) -> &'a str {
    if app_name != "Terminal" {
        return app_name;
    }
    // Combine all text into one scan pass.
    let ide_signals = ["Antigravity", "Open Agent Manager"];
    let hit = ocr_samples
        .iter()
        .map(|s| s.text.as_str())
        .chain(elements_samples.iter().map(|e| e.text.as_str()))
        .any(|t| ide_signals.iter().any(|sig| t.contains(sig)));
    if hit {
        "Antigravity"
    } else {
        app_name
    }
}

/// Fetches all enrichment data for a single contiguous block of frames in
/// parallel.  All five screenpipe query functions are called concurrently via
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
    // All five reads are independent — fire them all at once.
    let (window_titles_res, ocr_res, elements_res, audio_res, signals_res) = tokio::join!(
        get_window_titles(screenpipe, min_frame_id, max_frame_id, app_name),
        get_ocr_samples(screenpipe, min_frame_id, max_frame_id),
        get_element_samples(screenpipe, min_frame_id, max_frame_id),
        get_audio_snippets(screenpipe, started_at, ended_at),
        get_signals(screenpipe, started_at, ended_at),
    );

    let ocr_samples = ocr_res?;
    let elements_samples = dedup_ax_textarea(elements_res?);
    let true_app_name = reclassify_terminal(app_name, &ocr_samples, &elements_samples).to_owned();
    let audio_snippets = audio_res?;

    tracing::Span::current().record("ocr_sample_count", ocr_samples.len());
    tracing::Span::current().record("audio_snippet_count", audio_snippets.len());

    Ok(BlockContext {
        app_name: true_app_name,
        started_at: started_at.to_owned(),
        ended_at: ended_at.to_owned(),
        min_frame_id,
        max_frame_id,
        frame_count,
        window_titles: window_titles_res?,
        ocr_samples,
        elements_samples,
        audio_snippets,
        signals: signals_res?,
    })
}
