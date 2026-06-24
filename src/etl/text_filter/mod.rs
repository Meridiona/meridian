//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity

//! Per-line noise filters and cross-frame chrome detection for session text.
//!
//! `is_landmark`, `is_log_noise`, `is_quality_line`, and `alphabetic_ratio` are
//! pure and allocation-free (ASCII fast paths; no `to_ascii_lowercase` heap copy).
//! `build_chrome_set` is the only allocating function: one `HashMap<String, u32>`
//! for line frequencies + one reused `HashSet<u64>` for per-frame dedup, both
//! freed when the set is returned from `build_session_text`.

use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};

use crate::db::screenpipe::FrameText;

/// Case-insensitive substring search over ASCII content — no allocation.
/// Both `haystack` and `needle` must be ASCII-only for correct results.
#[inline]
fn ascii_icontains(haystack: &str, needle: &str) -> bool {
    let hn = needle.len();
    if hn == 0 {
        return true;
    }
    if haystack.len() < hn {
        return false;
    }
    haystack.as_bytes().windows(hn).any(|w| {
        w.iter()
            .zip(needle.as_bytes())
            .all(|(h, n)| h.eq_ignore_ascii_case(n))
    })
}

/// Case-insensitive prefix check — no allocation.
#[inline]
fn ascii_istarts_with(line: &str, prefix: &str) -> bool {
    line.len() >= prefix.len()
        && line.as_bytes()[..prefix.len()]
            .iter()
            .zip(prefix.as_bytes())
            .all(|(h, n)| h.eq_ignore_ascii_case(n))
}

// ── Constants ─────────────────────────────────────────────────────────────────

/// Lines appearing in this many or more distinct frames are persistent UI chrome
/// (sidebars, toolbars, status bars).  Raised above 3 to reduce false positives
/// on lines that repeat in a short burst without being persistent chrome.
pub const CHROME_FREQ_THRESHOLD: u32 = 4;

/// Minimum character length for non-landmark lines.  Lines shorter than this
/// carry too little text to be useful classification signal.
/// Backed by LLM data-filtering research: ~15 chars ≈ 3 words minimum unit.
const MIN_LINE_LEN: usize = 15;

/// Minimum fraction of alphabetic characters for non-landmark lines.
/// Gopher Quality Filter uses 80% word-level; at character level for mixed
/// code/terminal OCR that maps to ~35%.  Landmarks bypass this check entirely.
const ALPHA_RATIO_MIN: f64 = 0.35;

// ── Landmark detection ────────────────────────────────────────────────────────

/// Returns `true` when a line contains high-value developer signal that must
/// survive all noise filters regardless of alpha ratio or length.
///
/// Patterns: URLs, shell prompts, errors/tracebacks, code signatures, SQL
/// keywords, git branch refs, issue refs (#123), commit hashes (7–40 hex chars).
pub fn is_landmark(line: &str) -> bool {
    // URLs — early exit before any search
    if line.contains("http://") || line.contains("https://") {
        return true;
    }

    // Shell prompts: `$ cmd`, `% cmd`, `# comment`, `> input`, `❯ cmd` (Oh My Zsh / Starship)
    if line.starts_with("$ ")
        || line.starts_with("% ")
        || line.starts_with("# ")
        || line.starts_with("> ")
        || line.starts_with("❯ ")
    {
        return true;
    }

    // Error / warning keywords — ascii_icontains avoids to_ascii_lowercase() allocation
    if ascii_icontains(line, "error")
        || ascii_icontains(line, "warning")
        || ascii_icontains(line, "failed")
        || ascii_icontains(line, "exit code")
        || ascii_icontains(line, "traceback")
    {
        return true;
    }

    // Code signatures — these keywords are lowercase in all languages
    if line.contains("def ")
        || line.contains("fn ")
        || line.contains("class ")
        || line.contains("impl ")
        || line.contains("function ")
    {
        return true;
    }

    // SQL keywords — ascii_icontains handles SELECT / select / Select uniformly
    if ascii_icontains(line, "select ")
        || ascii_icontains(line, "insert ")
        || ascii_icontains(line, "update ")
        || ascii_icontains(line, "delete ")
        || ascii_icontains(line, "create table")
    {
        return true;
    }

    // Git branch prefixes — always lowercase in branch names
    if line.contains("feat/")
        || line.contains("fix/")
        || line.contains("chore/")
        || line.contains("refactor/")
    {
        return true;
    }

    // Issue / PR reference: `#` followed by 2–6 digits
    if contains_issue_ref(line) {
        return true;
    }

    // Ticket keys: Jira/Linear style — 2+ uppercase letters, dash, 1+ digits (e.g. KAN-141).
    // Critical: the classifier's core job is task-linking; losing a visible ticket key
    // from session_text is the worst possible false-negative.
    if contains_ticket_key(line) {
        return true;
    }

    // Code filenames: a word containing a dot followed by a known source extension.
    // Short filenames like `main.rs` or `app.py` are under MIN_LINE_LEN but carry
    // high signal about what file the developer was editing.
    if contains_code_filename(line) {
        return true;
    }

    // Commit hash: 7–40 hex chars with ≥1 letter a–f (excludes pure-digit dates)
    if contains_commit_hash(line) {
        return true;
    }

    false
}

/// Returns `true` for Jira/Linear-style ticket keys: 2+ uppercase ASCII letters
/// followed by a dash and at least one digit (e.g. `KAN-141`, `PROJ-1`, `MER-42`).
fn contains_ticket_key(line: &str) -> bool {
    let bytes = line.as_bytes();
    let n = bytes.len();
    let mut i = 0;
    while i < n {
        if bytes[i].is_ascii_uppercase() {
            let alpha_start = i;
            while i < n && bytes[i].is_ascii_uppercase() {
                i += 1;
            }
            let alpha_len = i - alpha_start;
            if alpha_len >= 2 && i < n && bytes[i] == b'-' {
                i += 1; // skip dash
                let digit_start = i;
                while i < n && bytes[i].is_ascii_digit() {
                    i += 1;
                }
                if i > digit_start {
                    return true;
                }
            }
        } else {
            i += 1;
        }
    }
    false
}

/// Returns `true` if `line` contains a word with a known source-code file extension.
/// Catches short filenames (`main.rs`, `app.py`) that would otherwise fall below
/// `MIN_LINE_LEN` and lose meaningful "what file was open" signal.
fn contains_code_filename(line: &str) -> bool {
    const EXTS: &[&str] = &[
        ".rs", ".py", ".ts", ".tsx", ".js", ".jsx", ".go", ".toml", ".sql", ".sh", ".yaml", ".yml",
    ];
    EXTS.iter().any(|ext| line.contains(ext))
}

/// Tighter landmark check used only inside `build_chrome_set` to decide
/// whether a frequently-repeating line should be exempted from chrome detection.
///
/// Differences from `is_landmark`:
/// - `# ` and `> ` prefixes are **excluded** — they appear in status bars and
///   blockquotes and would prevent common UI chrome from being filtered.
/// - SQL keywords are **anchored to line start** — substring matches on prose
///   like "last update" or "select all" should not protect a chrome line.
/// - Ticket keys and code filenames are included (high signal, exempt from chrome).
pub(crate) fn is_chrome_exempt(line: &str) -> bool {
    if line.contains("http://") || line.contains("https://") {
        return true;
    }
    // Only unambiguous interactive prompts — not `#` (comment) or `>` (blockquote)
    if line.starts_with("$ ") || line.starts_with("% ") || line.starts_with("❯ ") {
        return true;
    }
    if ascii_icontains(line, "error")
        || ascii_icontains(line, "warning")
        || ascii_icontains(line, "failed")
        || ascii_icontains(line, "traceback")
    {
        return true;
    }
    if line.contains("def ")
        || line.contains("fn ")
        || line.contains("class ")
        || line.contains("impl ")
        || line.contains("function ")
    {
        return true;
    }
    // SQL anchored to line start to avoid prose false-positives
    if ascii_istarts_with(line, "select ")
        || ascii_istarts_with(line, "insert ")
        || ascii_istarts_with(line, "update ")
        || ascii_istarts_with(line, "delete ")
        || ascii_istarts_with(line, "create table")
    {
        return true;
    }
    if line.contains("feat/")
        || line.contains("fix/")
        || line.contains("chore/")
        || line.contains("refactor/")
    {
        return true;
    }
    // Ticket keys and issue refs are high-signal enough to protect from chrome detection.
    // Code filenames are intentionally excluded: short filenames like `# main.py` appear
    // in VS Code tab bars (chrome) and should not be unconditionally exempted.
    if contains_issue_ref(line) || contains_ticket_key(line) {
        return true;
    }
    if contains_commit_hash(line) {
        return true;
    }
    false
}

/// `#` followed by 2–6 ASCII digits anywhere in `line`.
fn contains_issue_ref(line: &str) -> bool {
    let bytes = line.as_bytes();
    let n = bytes.len();
    let mut i = 0;
    while i < n {
        if bytes[i] == b'#' && i + 2 < n {
            let start = i + 1;
            let end = (start + 6).min(n);
            let digits = bytes[start..end]
                .iter()
                .take_while(|b| b.is_ascii_digit())
                .count();
            if digits >= 2 {
                return true;
            }
        }
        i += 1;
    }
    false
}

/// A run of 7–40 consecutive hex digits that contains ≥1 alphabetic hex letter
/// (a–f / A–F).  Pure-digit runs (e.g. dates like `20240101`) are rejected.
fn contains_commit_hash(line: &str) -> bool {
    let bytes = line.as_bytes();
    let n = bytes.len();
    let mut i = 0;
    while i < n {
        if is_hex_byte(bytes[i]) {
            let start = i;
            while i < n && is_hex_byte(bytes[i]) {
                i += 1;
            }
            let len = i - start;
            if (7..=40).contains(&len) {
                let has_letter = bytes[start..i].iter().any(|&b| b.is_ascii_alphabetic());
                if has_letter {
                    return true;
                }
            }
        } else {
            i += 1;
        }
    }
    false
}

#[inline]
fn is_hex_byte(b: u8) -> bool {
    b.is_ascii_digit() || (b'a'..=b'f').contains(&b) || (b'A'..=b'F').contains(&b)
}

// ── Log-noise detection ───────────────────────────────────────────────────────

/// Returns `true` when a line is an operational daemon/server log — not developer
/// activity.  Only called for non-landmark lines; callers must check `is_landmark`
/// first (e.g. an `ERROR` log line is a landmark and must not be dropped).
///
/// Covers:
///  - Structured JSON logs (`{"level": …, "timestamp": …}`)
///  - Python/Go level-prefixed logs (`INFO:logger:msg`, `DEBUG:…`)
///  - Rust tracing format (`2024-01-01T12:00:00Z  INFO meridian::…`)
///  - Model/server operational messages (Active mem, Fetching N files, …)
///  - Progress bars and percentage lines (`████ 100%|`)
pub fn is_log_noise(line: &str) -> bool {
    // JSON log blob: starts with `{` and carries a log-level or service field
    if line.starts_with('{')
        && (line.contains("\"level\"")
            || line.contains("\"timestamp\"")
            || line.contains("\"service.name\""))
    {
        return true;
    }

    // Python/Go/generic level-prefixed: INFO:module:msg or INFO:  msg (uvicorn)
    for prefix in ["INFO:", "DEBUG:", "WARNING:", "CRITICAL:", "TRACE:"] {
        if let Some(rest) = line.strip_prefix(prefix) {
            // Two-colon form (INFO:logger:msg) or space-padded form (INFO:  msg, INFO:     msg)
            // Uvicorn uses 5 spaces, StatReload uses 2 — accept 2+ spaces as the threshold.
            if rest.contains(':') || rest.starts_with("  ") {
                return true;
            }
        }
    }

    // Rust tracing — two forms:
    //   Timestamped:  "2024-01-01T12:00:00Z  INFO meridian::etl: ..."
    //   Compact:      "INFO meridian::config: ..." / "WARN sqlx::query: ..."
    // Both are daemon operational logs, not developer activity.

    // Compact form: "LEVEL module::path: message" — "::" is the Rust module-path
    // separator; absent from natural English text.  No length guard needed — the
    // strip_prefix call handles short strings safely.
    for level in ["INFO ", "WARN ", "DEBUG ", "TRACE "] {
        if let Some(rest) = line.strip_prefix(level) {
            if rest.contains("::") {
                return true;
            }
        }
    }

    // Timestamped form: starts with YYYY- and carries a level keyword.
    // Rust tracing emits uppercase levels ("  INFO ", "  WARN "); use
    // ascii_icontains to catch any casing without allocating.
    if line.len() > 20 {
        let b = line.as_bytes();
        if b[0].is_ascii_digit()
            && b[1].is_ascii_digit()
            && b[2].is_ascii_digit()
            && b[3].is_ascii_digit()
            && b[4] == b'-'
            && (ascii_icontains(line, " info ")
                || ascii_icontains(line, " debug ")
                || ascii_icontains(line, " trace ")
                || ascii_icontains(line, " warn "))
        {
            return true;
        }
    }

    // Model/server operational messages
    if line.starts_with("Active mem")
        || line.starts_with("Peak mem")
        || line.starts_with("Loading ")
        || line.starts_with("Compiling FSM")
        || line.starts_with("FSM ready")
        || line.starts_with("Finished in")
        || line.starts_with("Finished `")
    {
        return true;
    }

    // "Fetching N files:" — model weight download progress
    if line.starts_with("Fetching ") && line.as_bytes().get(9).is_some_and(|b| b.is_ascii_digit()) {
        return true;
    }

    // Progress bars: block characters or `%|` marker
    if line.contains("|█")
        || line.contains("█|")
        || line.contains("███")
        || line.contains("|░")
        || line.contains("░░░")
        || line.contains("%|")
    {
        return true;
    }

    false
}

// ── Alpha ratio ───────────────────────────────────────────────────────────────

/// Fraction of alphabetic characters in `line` (0.0–1.0). Empty string → 0.0.
///
/// Fast path: pure ASCII content uses byte iteration (3-4× faster than char
/// decoding). Falls back to char iteration for Unicode content.
pub fn alphabetic_ratio(line: &str) -> f64 {
    if line.is_empty() {
        return 0.0;
    }
    if line.is_ascii() {
        let alpha = line.bytes().filter(|b| b.is_ascii_alphabetic()).count();
        return alpha as f64 / line.len() as f64;
    }
    // Unicode slow path
    let mut total = 0usize;
    let mut alpha = 0usize;
    for c in line.chars() {
        total += 1;
        if c.is_alphabetic() {
            alpha += 1;
        }
    }
    if total == 0 {
        0.0
    } else {
        alpha as f64 / total as f64
    }
}

// ── Combined quality gate ─────────────────────────────────────────────────────

/// Returns `true` if a non-landmark line passes all quality thresholds.
///
/// Callers must check `is_landmark` first — landmark lines always pass
/// independently of length, alpha ratio, or log-noise status.
pub fn is_quality_line(line: &str) -> bool {
    if line.len() < MIN_LINE_LEN {
        return false;
    }
    if is_log_noise(line) {
        return false;
    }
    if alphabetic_ratio(line) < ALPHA_RATIO_MIN {
        return false;
    }
    true
}

// ── Chrome pre-pass ───────────────────────────────────────────────────────────

/// Returns the set of lines that appear in ≥ `CHROME_FREQ_THRESHOLD` distinct
/// frames AND are not landmarks.
///
/// These are persistent UI elements (sidebars, toolbars, status bars) that
/// repeat across every captured frame regardless of what the developer is doing.
///
/// **Memory model**: keys are owned `String`s cloned from frame text, but only
/// stored in the temporary `HashMap`.  For a 1 000-frame block with ~50 unique
/// lines averaging 40 chars each → ~2 MB peak, freed when this function returns
/// (the returned `HashSet` contains only the small chrome subset).
pub fn build_chrome_set(frames: &[FrameText]) -> HashSet<String> {
    // Count how many frames each trimmed line appears in (per-frame deduped).
    let mut freq: HashMap<String, u32> = HashMap::new();

    // Reuse one HashSet across all frames (allocated once, cleared per frame).
    // Using u64 hashes avoids lifetime conflicts with &str frame borrows and
    // removes the per-frame allocation overhead on large blocks.
    let mut seen_hashes: HashSet<u64> = HashSet::with_capacity(256);

    for frame in frames {
        seen_hashes.clear();
        for raw in frame.full_text.split('\n') {
            let line = raw.trim();
            if line.len() < 3 {
                continue;
            }
            let mut h = DefaultHasher::new();
            line.hash(&mut h);
            let hash = h.finish();
            if seen_hashes.insert(hash) {
                *freq.entry(line.to_owned()).or_insert(0) += 1;
            }
        }
    }

    // Keep lines at or above threshold that are not chrome-exempt (tighter than is_landmark:
    // excludes `# `/`> ` prefixes, anchors SQL to line start).
    freq.into_iter()
        .filter(|(line, count)| *count >= CHROME_FREQ_THRESHOLD && !is_chrome_exempt(line))
        .map(|(line, _)| line)
        .collect()
}

#[cfg(test)]
mod tests;
