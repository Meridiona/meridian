//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity

use anyhow::Result;

use crate::db::meridian::ActiveSession;
use crate::db::screenpipe::{AudioSnippet, SignalEvent, WindowTitleCount};
use crate::etl::extractor::BlockContext;
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
/// - `session_text`: when `session_text_override` is `Some(text)`, that text is used
///   directly (full rebuild from all frames — avoids chrome leaks from early sub-threshold
///   batches). When `None`, falls back to the additive `merge_session_texts` path.
pub(super) fn merge_into_active(
    existing: &ActiveSession,
    ctx: &BlockContext,
    new_idle_frame_count: i64,
    session_text_override: Option<String>,
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

    let merged_session_text = match session_text_override {
        Some(text) => text,
        None => merge_session_texts(
            existing.session_text.as_deref().unwrap_or(""),
            &ctx.session_text,
        ),
    };

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

/// Process names coding agents use as their terminal tab label.
/// VS Code sets the tab title to the foreground process name by default.
/// Agents that override it via OSC escape sequences (Claude Code active task)
/// are caught by the CLAUDE_SPINNERS check instead.
///
/// "q" (Amazon Q / Kiro) is absent here — it is matched by exact first-word
/// to avoid false-positives on "qemu", "queue-worker", etc.
const CODING_AGENT_NAMES: &[&str] = &[
    "claude",       // Claude Code CLI
    "codex",        // OpenAI Codex CLI
    "cursor-agent", // Cursor agent CLI
    "copilot",      // GitHub Copilot CLI
    "gemini",       // Google Gemini CLI
    "aider",        // Aider AI
];

/// Unicode spinner characters Claude Code emits via OSC escape sequences while
/// a task is running. These appear at the START of the terminal tab label and
/// are the most reliable per-frame signal that Claude Code is active.
const CLAUDE_SPINNERS: &[char] = &[
    '✳', '⠐', '⠂', '⠁', '⠄', '⠈', '⠘', '⠸', '⠴', '⠦', '⠧', '⠇', '⠏', '✢', '✻', '⏺',
];

/// Returns `true` when a VS Code terminal window title indicates the focused
/// terminal tab is running a coding agent (Claude Code, Codex, Cursor agent,
/// etc.) rather than a regular shell or build process.
///
/// The session label comes from the xterm.js `AXDescription` attribute read by
/// the screenpipe a11y tree walker — it is the authoritative per-tab signal.
///
/// VS Code uses either `"Terminal - "` (ASCII hyphen, default) or
/// `"Terminal — "` (em-dash, U+2014, some locales / custom title templates).
/// Both separators are tried.
///
/// Detection tiers:
///   0. Bare semver label: Claude Code idle/startup sets the tab title to its
///      own version ("Terminal - 2.1.193"). No task spinner is emitted in this
///      state, so the label is indistinguishable from a Node REPL or Python
///      interpreter by name alone — we suppress all bare X.Y.Z labels because
///      the coding-agent indexer tracks these sessions and VS Code terminal
///      REPL use is not meaningful focus time.
///   1. Claude Code active: spinner char at the very start of the label via OSC
///      escape sequences (e.g. "Terminal - ⠂ agentic-worklog-…").
///   2. Agent name: the first space-delimited word of the label starts with a
///      known agent binary name — anchored to avoid false-positives on tabs like
///      "Terminal - decodex-runner" (contains "codex" but isn't Codex).
///      "codex-aarch64-ap", "cursor-agent.2026", "copilot-node" all match.
///   3. Amazon Q / Kiro ("q"): exact first-word match only.
pub(super) fn is_coding_agent_terminal(window_name: &str) -> bool {
    // Try ASCII hyphen then em-dash separator.
    let session = window_name
        .strip_prefix("Terminal - ")
        .or_else(|| window_name.strip_prefix("Terminal \u{2014} "))
        .map(str::trim);
    let session = match session {
        Some(s) if !s.is_empty() => s,
        _ => return false,
    };

    // Tier 1 — Claude Code active: spinner char at the very start.
    if session.starts_with(|c: char| CLAUDE_SPINNERS.contains(&c)) {
        return true;
    }

    // Tier 2 & 3 — match on the first space-delimited word (the process name).
    // starts_with anchors to the process-name prefix:
    //   "codex-aarch64-ap".starts_with("codex")   → true  ✓
    //   "decodex-runner".starts_with("codex")      → false ✓
    //   "gitclaude".starts_with("claude")          → false ✓
    let session_lower = session.to_lowercase();
    let first_word = session_lower
        .split_whitespace()
        .next()
        .unwrap_or(&session_lower);

    if first_word == "q" {
        return true; // Amazon Q / Kiro — exact match to avoid "qemu", "queue-worker"
    }

    CODING_AGENT_NAMES
        .iter()
        .any(|&name| first_word.starts_with(name))
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

#[cfg(test)]
mod tests {
    use super::is_coding_agent_terminal;

    #[test]
    fn detects_claude_code_spinner() {
        // Spinner at start — Claude Code active task
        assert!(is_coding_agent_terminal(
            "Terminal - ⠂ agentic-worklog-matcher-pipeline"
        ));
        assert!(is_coding_agent_terminal(
            "Terminal - ✳ Fix PM sync start date..."
        ));
        assert!(is_coding_agent_terminal(
            "Terminal - ⠐ Understand code capturing"
        ));
        assert!(is_coding_agent_terminal("Terminal - ⏺ Some other task"));
    }

    #[test]
    fn detects_other_agents_by_process_name() {
        assert!(is_coding_agent_terminal("Terminal - codex"));
        assert!(is_coding_agent_terminal("Terminal - cursor-agent"));
        assert!(is_coding_agent_terminal("Terminal - copilot"));
        assert!(is_coding_agent_terminal("Terminal - gemini"));
        assert!(is_coding_agent_terminal("Terminal - aider"));
        assert!(is_coding_agent_terminal("Terminal - q"));
        assert!(is_coding_agent_terminal("Terminal - claude")); // fallback when no OSC title
    }

    #[test]
    fn detects_architecture_suffixed_binaries() {
        // Codex ARM Mac binary: codex-aarch64-apple-darwin (VS Code truncates it)
        assert!(is_coding_agent_terminal("Terminal - codex-aarch64-ap"));
        assert!(is_coding_agent_terminal(
            "Terminal - codex-aarch64-apple-darwin"
        ));
        assert!(is_coding_agent_terminal(
            "Terminal - codex-x86_64-apple-darwin"
        ));
        // cursor-agent with version suffix
        assert!(is_coding_agent_terminal("Terminal - cursor-agent.2026"));
        // "q" stays exact-word to avoid matching "qemu", "queue-worker" etc.
        assert!(!is_coding_agent_terminal("Terminal - qemu"));
        assert!(!is_coding_agent_terminal("Terminal - queue-worker"));
    }

    #[test]
    fn starts_with_anchoring_avoids_false_positives() {
        // "decodex-runner" contains "codex" but does NOT start with it
        assert!(!is_coding_agent_terminal("Terminal - decodex-runner"));
        // "gitclaude" contains "claude" but does NOT start with it
        assert!(!is_coding_agent_terminal("Terminal - gitclaude"));
        // "aider-helper" starts with "aider" → matches (it likely IS aider)
        assert!(is_coding_agent_terminal("Terminal - aider-helper"));
    }

    #[test]
    fn detects_em_dash_separator() {
        // VS Code with custom terminal.integrated.tabs.title or certain locales
        // uses U+2014 (—) instead of ASCII hyphen.
        assert!(is_coding_agent_terminal("Terminal \u{2014} claude"));
        assert!(is_coding_agent_terminal("Terminal \u{2014} ⠂ agentic-task"));
        assert!(is_coding_agent_terminal(
            "Terminal \u{2014} codex-aarch64-ap"
        ));
        assert!(!is_coding_agent_terminal("Terminal \u{2014} zsh"));
    }

    #[test]
    fn does_not_suppress_normal_terminals() {
        assert!(!is_coding_agent_terminal("Terminal - zsh"));
        assert!(!is_coding_agent_terminal("Terminal - bash"));
        assert!(!is_coding_agent_terminal("Terminal - npm"));
        assert!(!is_coding_agent_terminal("Terminal - node"));
        assert!(!is_coding_agent_terminal("Terminal - fish"));
        assert!(!is_coding_agent_terminal("Terminal")); // bare terminal, no dash
        assert!(!is_coding_agent_terminal("build.rs — screenpipe")); // editor tab, not terminal
        assert!(!is_coding_agent_terminal("")); // empty
    }
}
