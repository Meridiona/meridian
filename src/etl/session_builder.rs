//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity

use anyhow::Result;

use crate::db::meridian::ActiveSession;
use crate::db::screenpipe::{AudioSnippet, SignalEvent, WindowTitleCount};
use crate::etl::extractor::{BlockContext, FrameContribution, FRAME_CONTRIBUTION_CAP};
use crate::etl::text_merge::merge_session_texts;
use crate::intelligence::session_categorizer::{categorize, SessionSignals};

const AUDIO_SNIPPET_CAP: usize = 50;

struct ClassifyInput<'a> {
    app_name: &'a str,
    window_titles: &'a [WindowTitleCount],
    audio_snippets: &'a [AudioSnippet],
    signals: &'a [SignalEvent],
    started_at: &'a str,
    ended_at: &'a str,
    session_text: &'a str,
}

fn classify(i: &ClassifyInput<'_>) -> (String, f64) {
    let duration_secs = chrono::DateTime::parse_from_rfc3339(i.ended_at)
        .ok()
        .zip(chrono::DateTime::parse_from_rfc3339(i.started_at).ok())
        .map(|(end, start)| (end - start).num_seconds().max(0) as u64)
        .unwrap_or(0);
    let sig = SessionSignals {
        app_name: i.app_name,
        window_titles: i.window_titles,
        ocr_text: i.session_text,
        signals: i.signals,
        audio_present: !i.audio_snippets.is_empty(),
        duration_secs,
    };
    let (kind, confidence) = categorize(&sig);
    (kind.as_str().to_owned(), confidence as f64)
}

/// Builds a brand-new `ActiveSession` from a `BlockContext`.
pub(super) fn build_active_session(
    ctx: &BlockContext,
    idle_frame_count: i64,
) -> Result<ActiveSession> {
    let (category, confidence) = classify(&ClassifyInput {
        app_name: &ctx.app_name,
        window_titles: &ctx.window_titles,
        audio_snippets: &ctx.audio_snippets,
        signals: &ctx.signals,
        started_at: &ctx.started_at,
        ended_at: &ctx.ended_at,
        session_text: &ctx.session_text,
    });
    Ok(ActiveSession {
        id: 1,
        app_name: ctx.app_name.clone(),
        started_at: ctx.started_at.clone(),
        last_seen_at: ctx.ended_at.clone(),
        window_titles: serde_json::to_string(&ctx.window_titles)?,
        audio_snippets: Some(serde_json::to_string(&ctx.audio_snippets)?),
        signals: Some(serde_json::to_string(&ctx.signals)?),
        min_frame_id: ctx.min_frame_id,
        max_frame_id: ctx.max_frame_id,
        frame_count: ctx.frame_count,
        idle_frame_count,
        category,
        confidence,
        session_text: Some(ctx.session_text.clone()),
        frame_contributions: Some(ctx.frame_contributions.clone()),
    })
}

/// Merges a new `BlockContext` into an existing `ActiveSession` row and
/// returns the updated session.
///
/// Merge rules:
/// - `started_at`: kept from the existing session.
/// - `last_seen_at`: set to `ctx.ended_at`.
/// - `min_frame_id`: kept from the existing session.
/// - `max_frame_id`: updated to the new block's max.
/// - `frame_count`: summed.
/// - `window_titles`: counts from identical titles are incremented; new titles are appended.
/// - `audio_snippets`: appended, capped at `AUDIO_SNIPPET_CAP`.
/// - `signals`: all new signals appended.
pub(super) fn merge_into_active(
    existing: &ActiveSession,
    ctx: &BlockContext,
    new_idle_frame_count: i64,
) -> Result<ActiveSession> {
    let now = ctx.ended_at.clone();

    let mut merged_titles: Vec<WindowTitleCount> =
        serde_json::from_str(&existing.window_titles).unwrap_or_default();
    for new_t in &ctx.window_titles {
        if let Some(existing_t) = merged_titles
            .iter_mut()
            .find(|t| t.window_name == new_t.window_name)
        {
            existing_t.count += new_t.count;
        } else {
            merged_titles.push(new_t.clone());
        }
    }
    merged_titles.sort_by(|a, b| b.count.cmp(&a.count));

    let mut audio: Vec<AudioSnippet> = existing
        .audio_snippets
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default();
    for snippet in &ctx.audio_snippets {
        if audio.len() >= AUDIO_SNIPPET_CAP {
            break;
        }
        audio.push(snippet.clone());
    }

    let mut signals: Vec<SignalEvent> = existing
        .signals
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default();
    signals.extend(ctx.signals.iter().cloned());

    let merged_session_text = merge_session_texts(
        existing.session_text.as_deref().unwrap_or(""),
        &ctx.session_text,
    );

    // Union the per-frame provenance, deduped by frame_id (a re-extracted block
    // re-reports earlier frames), preserving order and capped like the source.
    let mut frame_contribs: Vec<FrameContribution> = existing
        .frame_contributions
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default();
    let mut seen_frame_ids: std::collections::HashSet<i64> =
        frame_contribs.iter().map(|f| f.frame_id).collect();
    let new_contribs: Vec<FrameContribution> =
        serde_json::from_str(&ctx.frame_contributions).unwrap_or_default();
    for fc in new_contribs {
        if frame_contribs.len() >= FRAME_CONTRIBUTION_CAP {
            break;
        }
        if seen_frame_ids.insert(fc.frame_id) {
            frame_contribs.push(fc);
        }
    }
    let merged_frame_contributions = serde_json::to_string(&frame_contribs)?;

    let (category, confidence) = classify(&ClassifyInput {
        app_name: &existing.app_name,
        window_titles: &merged_titles,
        audio_snippets: &audio,
        signals: &signals,
        started_at: &existing.started_at,
        ended_at: &now,
        session_text: &merged_session_text,
    });

    Ok(ActiveSession {
        id: 1,
        app_name: existing.app_name.clone(),
        started_at: existing.started_at.clone(),
        last_seen_at: now,
        window_titles: serde_json::to_string(&merged_titles)?,
        audio_snippets: Some(serde_json::to_string(&audio)?),
        signals: Some(serde_json::to_string(&signals)?),
        min_frame_id: existing.min_frame_id,
        max_frame_id: ctx.max_frame_id,
        frame_count: existing.frame_count + ctx.frame_count,
        idle_frame_count: existing.idle_frame_count + new_idle_frame_count,
        category,
        confidence,
        session_text: Some(merged_session_text),
        frame_contributions: Some(merged_frame_contributions),
    })
}

/// Returns `true` if `app` is a known browser.
pub(super) fn is_browser(app: &str) -> bool {
    let lc = app.to_lowercase();
    [
        "chrome", "safari", "firefox", "arc", "edge", "brave", "opera", "vivaldi",
    ]
    .iter()
    .any(|b| lc.contains(b))
}

/// Extracts the bare domain from a URL — strips scheme, path, query, and `www.`.
/// Returns the full string unchanged if it doesn't look like a URL.
pub(super) fn url_domain(url: &str) -> &str {
    let without_scheme = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url);
    let domain = without_scheme.split('/').next().unwrap_or(without_scheme);
    domain.strip_prefix("www.").unwrap_or(domain)
}

/// Known VS Code-like editor app names (lowercase).
const VSCODE_LIKE_APPS: &[&str] = &[
    "code",
    "cursor",
    "windsurf",
    "antigravity",
    "vscodium",
    "positron",
    "void",
    "aide",
    "trae",
];

/// Returns `true` when the app is a VS Code fork using Electron + xterm.js.
pub(super) fn is_vscode_like(app: &str) -> bool {
    let lc = app.to_lowercase();
    VSCODE_LIKE_APPS.iter().any(|name| lc.contains(name))
}

/// Extracts the VS Code project/repo name from a window title.
///
/// VS Code window title formats:
///   "build.rs — screenpipe"                        → "screenpipe"
///   "Terminal - meridian"                           → "meridian"
///   "macos.rs — screenpipe (crates/screenpipe-a11y)" → "screenpipe"
///   "● config.rs — myproject"                      → "myproject"
///
/// Returns `None` for windows without a recognisable separator (e.g. the
/// VS Code Welcome tab or an empty window).
pub(super) fn vscode_project(window_name: &str) -> Option<&str> {
    // Find the last occurrence of " — " (em-dash, VS Code's file separator)
    // or " - " (hyphen, used for Terminal and some older builds).
    let sep_pos = window_name
        .rfind(" \u{2014} ") // " — "
        .or_else(|| window_name.rfind(" - "));
    let sep_pos = sep_pos?;

    // Everything after the separator, trimmed.
    let after = window_name[sep_pos..].trim_start_matches([' ', '-', '\u{2014}']);
    let bare = after.trim();

    // Strip a parenthesised sub-path: "screenpipe (crates/...)" → "screenpipe"
    let bare = bare.split(" (").next().unwrap_or(bare).trim();

    if bare.is_empty() {
        None
    } else {
        Some(bare)
    }
}
