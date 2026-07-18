//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Stages 1-3 of the distiller: line segmentation + the junk gate + the prose gate,
//! plus the shared regexes and small text helpers (`segment`, `is_hard_junk`,
//! `fails_prose_gate`, `norm`, `clean_window_title`).
//!
//! These gates are what turn raw OCR/a11y screen text into candidate prose spans:
//! `_segment` breaks over-long run-on lines on bullet/whitespace glyphs, `is_hard_junk`
//! drops UI chrome and non-prose noise, and `fails_prose_gate` keeps human-readable
//! sentences while letting code-ish lines through.

use once_cell::sync::Lazy;
use regex::Regex;
use std::collections::HashSet;

/// Named facts we must never drop: ticket keys (`ABC-123`), PR/issue numbers, and
/// source file paths. Used both as a keep-override in the DF/SemDeDup stages and as
/// the entity-rescue trigger.
pub static NAMED_ENTITY_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"(?i)\b[A-Z]{2,10}-\d+\b|(?:PR\s*#?|#)\d{2,5}\b|\b\w[\w./\\-]{3,}\.(?:rs|py|ts|tsx|js|jsx|md|json|toml|sh|sql|yaml|yml)\b",
    )
    .expect("NAMED_ENTITY_RE")
});

/// Substrings that mark a line as agent/terminal chrome rather than work content.
pub const JUNK_SUBSTRINGS: &[&str] = &[
    "<local-command-caveat>",
    "local-command-stdout",
    "ctrl+o to expand",
    "ctrl+o to",
    "lines (ctrl",
    "auto mode classifier",
    "Allowed by auto mode",
    "for agents",
    "shift+tab to cycle",
    "+ s ifi",
    "Type to",
    "tokens · ",
];

static SPINNER_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"Photosynthesizing|Thinking|tokens\)|esc to interrupt|[↓✶✳⠂⏵]").expect("SPINNER_RE")
});
static TIMESTAMP_ONLY_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^\[\d\d:\d\d:\d\d\]\s*$").expect("TIMESTAMP_ONLY_RE"));
static URL_PREFIX_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)^\[url\]\s*").expect("URL_PREFIX_RE"));
static NONWORD_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"[^a-z0-9]+").expect("NONWORD_RE"));
static WORDTOKEN_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"[A-Za-z][A-Za-z']+").expect("WORDTOKEN_RE"));
// NOTE: intentionally case-SENSITIVE (the Python has no re.I here) — the keyword list
// is lowercase/UPPERCASE-specific (SQL keywords upper, code keywords lower).
static CODEISH_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"[/\\.]|::|->|\b(?:def|fn|let|const|import|cargo|git|npm|python|SELECT|FROM|WHERE)\b",
    )
    .expect("CODEISH_RE")
});
static EXT_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"(?i)\.(?:py|md|sh|json|toml|rs|tsx?|jsx?|lock|txt|ya?ml|cfg|ini|sql|db|env|rb|go|c|h)\b",
    )
    .expect("EXT_RE")
});
static SEG_SPLIT_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"[•·▸►▶‣⁃◦●○➤➔→⟶»«›‹❯❮|©®✓✦✱✶✳※❘┃│]+|\s{2,}").expect("SEG_SPLIT_RE"));
static WS_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\s+").expect("WS_RE"));

static STOP: Lazy<HashSet<&'static str>> = Lazy::new(|| {
    "the a an and or but to of in on for with at by from is are was were be been \
     this that these those it its as into out up down over we i you he she they \
     have has had do does did not no so if then than when while there here can will \
     would should could about which who what your my our their let me now"
        .split_whitespace()
        .collect()
});

/// A line longer than this triggers segmentation on bullet/whitespace glyphs.
pub const SEG_TRIGGER: usize = 140;

/// Lowercase + collapse every non-alphanumeric run to a single space, trimmed. The
/// dedup key basis (`_norm`).
pub fn norm(line: &str) -> String {
    NONWORD_RE
        .replace_all(&line.to_lowercase(), " ")
        .trim()
        .to_string()
}

/// Clean a raw window title: collapse whitespace, fold any terminal/caveat title to
/// `"Terminal"`, cap at 50 chars.
pub fn clean_window_title(raw: &str) -> String {
    let t = WS_RE.replace_all(raw, " ").trim().to_string();
    if t.contains("local-command-caveat") || t.to_lowercase().starts_with("terminal") {
        return "Terminal".to_string();
    }
    t.chars().take(50).collect()
}

/// Strip a leading `[url] ` prefix (case-insensitive) — the a11y capture tags link
/// text this way.
pub fn strip_url_prefix(line: &str) -> String {
    URL_PREFIX_RE.replace(line, "").to_string()
}

/// Matches a bare `[HH:MM:SS]` timestamp-only line (no content) — dropped as junk.
pub fn is_timestamp_only(s: &str) -> bool {
    TIMESTAMP_ONLY_RE.is_match(s)
}

/// Segment a line: short lines pass through whole; a line over [`SEG_TRIGGER`] is split
/// on bullet/separator glyphs and 2+ whitespace runs, yielding each non-empty piece.
pub fn segment(line: &str) -> Vec<String> {
    if line.chars().count() <= SEG_TRIGGER {
        return vec![line.to_string()];
    }
    SEG_SPLIT_RE
        .split(line)
        .map(|p| p.trim())
        .filter(|p| !p.is_empty())
        .map(str::to_string)
        .collect()
}

/// True for a line that is definitely noise: too short, a bare timestamp, agent
/// chrome, a spinner, or too few / too sparse letters to be prose.
pub fn is_hard_junk(line: &str) -> bool {
    let s = line.trim();
    if s.chars().count() < 18 {
        return true;
    }
    if is_timestamp_only(s) {
        return true;
    }
    if JUNK_SUBSTRINGS.iter().any(|j| s.contains(j)) {
        return true;
    }
    if SPINNER_RE.is_match(s) {
        return true;
    }
    let letters = s.chars().filter(|c| c.is_alphabetic()).count();
    letters < 10 || (letters as f64) / (s.chars().count() as f64) < 0.45
}

/// The empty-session fallback filter: a segment substantial enough to represent a
/// session that otherwise contributed no prose spans — long, mostly letters, and not
/// chrome/spinner. Mirrors the inline `fallback.append` guard in the Python.
pub fn is_fallback_candidate(seg: &str) -> bool {
    let len = seg.chars().count();
    let letters = seg.chars().filter(|c| c.is_alphabetic()).count();
    len >= 18
        && letters >= 12
        && (letters as f64) / (len as f64) >= 0.5
        && !JUNK_SUBSTRINGS.iter().any(|j| seg.contains(j))
        && !SPINNER_RE.is_match(seg)
}

/// True when a line reads as noise rather than prose: no word tokens, a dense list of
/// file extensions with almost no stopwords, or (for non-code-ish lines) too few
/// stopwords to be a sentence. Code-ish lines bypass the stopword floor.
pub fn fails_prose_gate(line: &str) -> bool {
    let s = line.trim();
    let tokens: Vec<String> = WORDTOKEN_RE
        .find_iter(s)
        .map(|m| m.as_str().to_lowercase())
        .collect();
    if tokens.is_empty() {
        return true;
    }
    let n_stop = tokens.iter().filter(|t| STOP.contains(t.as_str())).count();
    if EXT_RE.find_iter(s).count() >= 3 && n_stop < 2 {
        return true;
    }
    if CODEISH_RE.is_match(s) {
        return false;
    }
    n_stop < 2 || (n_stop as f64) / (tokens.len() as f64) < 0.08
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn junk_gate_drops_chrome_and_short() {
        assert!(is_hard_junk("short"));
        assert!(is_hard_junk("[12:00:00]"));
        assert!(is_hard_junk("ctrl+o to expand the transcript"));
        assert!(!is_hard_junk(
            "Implemented the new distiller pipeline in Rust today"
        ));
    }

    #[test]
    fn prose_gate_keeps_sentences_and_code_drops_noise() {
        assert!(!fails_prose_gate(
            "We refactored the resolver to drop the local provider"
        ));
        // code-ish line bypasses the stopword floor
        assert!(!fails_prose_gate(
            "fn embed_batch(texts: &[String]) -> Result<Vec>"
        ));
        // a bare list of extensions with no stopwords fails
        assert!(fails_prose_gate("main.rs lib.rs mod.rs config.rs build.rs"));
    }

    #[test]
    fn named_entity_matches_keys_prs_and_paths() {
        assert!(NAMED_ENTITY_RE.is_match("closed KAN-309 today"));
        assert!(NAMED_ENTITY_RE.is_match("merged PR #443"));
        assert!(NAMED_ENTITY_RE.is_match("edited src/worklog_pipeline/hour.rs"));
        assert!(!NAMED_ENTITY_RE.is_match("just some plain words here"));
    }

    #[test]
    fn segment_splits_long_runon_lines() {
        let short = "a normal line";
        assert_eq!(segment(short), vec![short.to_string()]);
        let long = format!(
            "{} • {} • {}",
            "x".repeat(60),
            "y".repeat(60),
            "z".repeat(60)
        );
        let segs = segment(&long);
        assert_eq!(segs.len(), 3);
    }

    #[test]
    fn norm_and_window_title() {
        assert_eq!(norm("Foo, Bar! Baz"), "foo bar baz");
        assert_eq!(clean_window_title("Terminal — zsh"), "Terminal");
        assert_eq!(clean_window_title("  My   Editor  "), "My Editor");
    }
}
