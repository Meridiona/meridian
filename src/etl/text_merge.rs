//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity

use std::collections::HashSet;

use super::text_filter::{build_chrome_set, is_landmark, is_quality_line};
use crate::db::screenpipe::FrameText;

/// Minimum elapsed seconds between consecutive [HH:MM:SS] markers.
const MARKER_GAP_SECS: i64 = 30;

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Build session_text from scratch from an ordered sequence of frames.
///
/// Runs a chrome pre-pass (lines appearing in ≥4 frames that are not landmarks
/// are classified as persistent UI chrome and dropped).  Each remaining unique
/// non-noise line appears exactly once, in the order it first appeared.
/// Timestamp markers ([HH:MM:SS]) are injected when new content appears more
/// than MARKER_GAP_SECS after the previous marker.
pub fn build_session_text(frames: &[FrameText]) -> String {
    // Chrome pre-pass: build the persistent-UI-chrome set before processing any frame.
    // This is a single O(N·L) scan where N=frames and L=lines per frame.
    // The HashSet is freed at the end of this function — ~400KB peak for 1000-frame blocks.
    let chrome = build_chrome_set(frames);

    let mut seen: HashSet<String> = HashSet::with_capacity(4096);
    let mut out = String::with_capacity(8192);
    let mut last_marker_secs: Option<i64> = None;

    for frame in frames {
        process_frame(frame, &mut seen, &mut out, &mut last_marker_secs, &chrome);
    }
    dedup_cursor_prefixes(out)
}

/// Merge new frame content into an existing session_text string (incremental update).
///
/// Parses the existing text to seed the seen-set so no line is ever duplicated.
/// Marker lines from new_content are included only when they pass the 30s threshold
/// against the last marker already in existing.  If new_content is a pre-built
/// session_text string (with [HH:MM:SS] markers), those markers are reused;
/// if it is raw text without markers, a single marker is emitted at the start.
pub fn merge_session_texts(existing: &str, new_content: &str) -> String {
    if new_content.is_empty() {
        return existing.to_owned();
    }
    if existing.is_empty() {
        return new_content.to_owned();
    }

    // Seed seen-set from all content lines in existing (skip marker lines).
    let mut seen: HashSet<String> = existing
        .lines()
        .filter(|l| !is_marker_line(l))
        .map(|l| l.to_owned())
        .collect();

    let mut out = existing.to_owned();
    if !out.ends_with('\n') {
        out.push('\n');
    }

    let mut last_marker_secs = extract_last_marker_secs(existing);
    // Buffer the most recent marker from new_content until we confirm there is
    // new content after it — avoids emitting a marker with no following lines.
    let mut pending_marker: Option<&str> = None;

    for line in new_content.lines() {
        if is_marker_line(line) {
            let new_secs = ts_hms_to_secs(&line[1..9]);
            let should_queue = match (last_marker_secs, new_secs) {
                (None, _) => true,
                (Some(last), Some(current)) => (current - last).abs() >= MARKER_GAP_SECS,
                _ => true,
            };
            if should_queue {
                pending_marker = Some(line);
            }
        } else if !line.is_empty() && seen.insert(line.to_owned()) {
            if let Some(marker) = pending_marker.take() {
                out.push_str(marker);
                out.push('\n');
                last_marker_secs = ts_hms_to_secs(&marker[1..9]);
            }
            out.push_str(line);
            out.push('\n');
        }
    }

    out
}

// ---------------------------------------------------------------------------
// Internals
// ---------------------------------------------------------------------------

fn process_frame(
    frame: &FrameText,
    seen: &mut HashSet<String>,
    out: &mut String,
    last_marker_secs: &mut Option<i64>,
    chrome: &HashSet<String>,
) {
    let frame_secs = ts_to_secs(&frame.timestamp);
    let new_lines = split_lines(&frame.full_text, chrome);
    let mut any_new = false;
    let mut fresh: Vec<String> = Vec::new();

    for line in new_lines {
        if seen.insert(line.clone()) {
            fresh.push(line);
            any_new = true;
        }
    }

    if !any_new {
        return;
    }

    // Emit a timestamp marker if enough time has elapsed.
    let emit_marker = match (*last_marker_secs, frame_secs) {
        (None, _) => true,
        (Some(last), Some(current)) => (current - last).abs() >= MARKER_GAP_SECS,
        (Some(_), None) => false,
    };

    if emit_marker {
        if let Some(ts) = format_ts(&frame.timestamp) {
            if !out.is_empty() && !out.ends_with('\n') {
                out.push('\n');
            }
            out.push('[');
            out.push_str(&ts);
            out.push_str("]\n");
            *last_marker_secs = frame_secs;
        }
    }

    for line in fresh {
        out.push_str(&line);
        out.push('\n');
    }
}

/// Split full_text into trimmed, non-empty lines, dropping OCR noise.
///
/// Noise removed at this layer:
///  - Lines in `chrome`: persistent UI elements (sidebars, toolbars, status bars)
///    that appear in ≥4 frames across the block but are not landmarks.
///  - Lines that don't pass `is_quality_line` AND are not `is_landmark`:
///    short fragments (< 15 chars), log noise, low alpha-ratio symbol runs.
///
/// Landmarks (URLs, shell prompts, errors, code signatures, SQL, git refs,
/// commit hashes) always survive regardless of length or alpha ratio.
///
/// Terminal OCR pathology: a single long string with no newlines is split
/// on 2+ consecutive spaces to recover phrase boundaries.
fn split_lines(full_text: &str, chrome: &HashSet<String>) -> Vec<String> {
    let mut result: Vec<String> = Vec::new();
    for raw_line in full_text.split('\n') {
        let trimmed = raw_line.trim();
        if trimmed.is_empty() {
            continue;
        }
        // Drop persistent-UI chrome before any further work.
        if chrome.contains(trimmed) {
            continue;
        }
        if trimmed.len() > 200 && trimmed.contains("  ") {
            // Long concatenated OCR line (Electron sidebars, browser tab bars) —
            // split into fragments then filter each one individually.
            for part in trimmed.split("  ") {
                let p = part.trim();
                if p.is_empty() {
                    continue;
                }
                if is_landmark(p) || is_quality_line(p) {
                    result.push(p.to_owned());
                }
            }
        } else if is_landmark(trimmed) || is_quality_line(trimmed) {
            result.push(trimmed.to_owned());
        }
    }
    result
}

/// Format an ISO 8601 timestamp to "HH:MM:SS" for marker output.
/// "2024-01-01T10:05:30.123Z" → "10:05:30"
fn format_ts(iso: &str) -> Option<String> {
    let t = iso.split('T').nth(1)?;
    if t.len() >= 8 {
        Some(t[..8].to_owned())
    } else {
        None
    }
}

/// Convert an ISO 8601 timestamp to seconds-since-midnight for threshold comparison.
fn ts_to_secs(iso: &str) -> Option<i64> {
    let t = iso.split('T').nth(1)?;
    if t.len() < 8 {
        return None;
    }
    let h: i64 = t[0..2].parse().ok()?;
    let m: i64 = t[3..5].parse().ok()?;
    let s: i64 = t[6..8].parse().ok()?;
    Some(h * 3600 + m * 60 + s)
}

/// Convert "HH:MM:SS" to seconds-since-midnight.
fn ts_hms_to_secs(hms: &str) -> Option<i64> {
    if hms.len() < 8 {
        return None;
    }
    let h: i64 = hms[0..2].parse().ok()?;
    let m: i64 = hms[3..5].parse().ok()?;
    let s: i64 = hms[6..8].parse().ok()?;
    Some(h * 3600 + m * 60 + s)
}

/// Returns true for "[HH:MM:SS]" marker lines (exactly 10 chars with correct structure).
fn is_marker_line(line: &str) -> bool {
    if line.len() != 10 {
        return false;
    }
    let b = line.as_bytes();
    b[0] == b'[' && b[9] == b']' && b[3] == b':' && b[6] == b':'
}

/// Extract the last marker's seconds-since-midnight from an existing session_text string.
fn extract_last_marker_secs(text: &str) -> Option<i64> {
    for line in text.lines().rev() {
        if is_marker_line(line) {
            return ts_hms_to_secs(&line[1..9]);
        }
    }
    None
}

/// Remove `❯ `-prefixed lines that are proper prefixes of a LATER `❯ `-prefixed line.
///
/// When screenpipe captures frames while the user is typing in a Claude Code
/// terminal, each frame may show the input mid-keystroke:
///
///   ❯ hello w
///   ❯ hello world
///
/// Only the final, complete version is useful. This pass removes all earlier
/// partial versions while leaving non-`❯ ` lines untouched.
///
/// A bounded lookahead (MAX_LOOKAHEAD cursor lines) prevents false positives
/// from coincidental prefix matches across unrelated commands later in the
/// session (e.g. "❯ git" is not removed just because "❯ git status" appears
/// 50 cursor lines later after other work).
fn dedup_cursor_prefixes(text: String) -> String {
    const CURSOR_PREFIX: &str = "❯ ";
    // Only remove A if B appears within the next N cursor lines — tight enough
    // to cover a single typing sequence (~10s at 1fps), loose enough for fast typists.
    const MAX_LOOKAHEAD: usize = 10;

    let lines: Vec<&str> = text.lines().collect();
    if lines.len() < 2 {
        return text;
    }

    // Collect (line_index, content_after_prefix) for every ❯-prefixed line.
    let cursor_lines: Vec<(usize, &str)> = lines
        .iter()
        .enumerate()
        .filter_map(|(i, l)| l.strip_prefix(CURSOR_PREFIX).map(|c| (i, c)))
        .collect();

    if cursor_lines.len() < 2 {
        return text;
    }

    let mut superseded = vec![false; lines.len()];

    for i in 0..cursor_lines.len() {
        let (idx_a, content_a) = cursor_lines[i];
        let end = (i + 1 + MAX_LOOKAHEAD).min(cursor_lines.len());
        for (_, content_b) in &cursor_lines[i + 1..end] {
            // B must strictly extend A (longer AND starts with A's content).
            if content_b.len() > content_a.len() && content_b.starts_with(content_a) {
                superseded[idx_a] = true;
                break;
            }
        }
    }

    if !superseded.iter().any(|&s| s) {
        return text;
    }

    let mut out = String::with_capacity(text.len());
    for (i, line) in lines.iter().enumerate() {
        if !superseded[i] {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(ts: &str, text: &str) -> FrameText {
        FrameText {
            frame_id: 0,
            timestamp: ts.to_owned(),
            full_text: text.to_owned(),
            text_source: "ocr".to_owned(),
        }
    }

    // ── Helper: build an empty chrome set for unit tests that don't need chrome filtering
    fn no_chrome() -> HashSet<String> {
        HashSet::new()
    }

    #[test]
    fn build_deduplicates_lines() {
        let frames = vec![
            frame("2024-01-01T10:00:00Z", "running cargo build release\nborrowing mutable reference\ncompiler diagnostics output"),
            frame("2024-01-01T10:00:01Z", "borrowing mutable reference\ncompiler diagnostics output\nno errors found in source"),
            frame("2024-01-01T10:00:02Z", "running cargo build release\nno errors found in source\ntests passed successfully"),
        ];
        let text = build_session_text(&frames);
        let lines: Vec<&str> = text.lines().filter(|l| !is_marker_line(l)).collect();
        assert_eq!(
            lines,
            [
                "running cargo build release",
                "borrowing mutable reference",
                "compiler diagnostics output",
                "no errors found in source",
                "tests passed successfully",
            ]
        );
    }

    #[test]
    fn build_emits_marker_on_first_frame() {
        let frames = vec![frame(
            "2024-01-01T10:05:30Z",
            "cargo build completed successfully",
        )];
        let text = build_session_text(&frames);
        assert!(text.contains("[10:05:30]"), "expected marker; got:\n{text}");
    }

    #[test]
    fn build_suppresses_marker_within_threshold() {
        let frames = vec![
            frame("2024-01-01T10:00:00Z", "first content line of session"),
            frame("2024-01-01T10:00:20Z", "second content line of session"), // 20s < 30s threshold
        ];
        let text = build_session_text(&frames);
        let markers: Vec<&str> = text.lines().filter(|l| is_marker_line(l)).collect();
        assert_eq!(markers.len(), 1, "expected only one marker; got:\n{text}");
    }

    #[test]
    fn build_emits_marker_beyond_threshold() {
        let frames = vec![
            frame("2024-01-01T10:00:00Z", "first content line of session"),
            frame("2024-01-01T10:01:00Z", "second content line of session"), // 60s > 30s threshold
        ];
        let text = build_session_text(&frames);
        let markers: Vec<&str> = text.lines().filter(|l| is_marker_line(l)).collect();
        assert_eq!(markers.len(), 2, "expected two markers; got:\n{text}");
    }

    #[test]
    fn merge_no_duplicates() {
        let existing = "[10:00:00]\nalpha\nbeta\n";
        let new_content = "[10:01:00]\nbeta\ngamma\ndelta\n";
        let merged = merge_session_texts(existing, new_content);
        let content: Vec<&str> = merged.lines().filter(|l| !is_marker_line(l)).collect();
        assert_eq!(content, ["alpha", "beta", "gamma", "delta"]);
    }

    #[test]
    fn merge_suppresses_close_marker() {
        let existing = "[10:00:00]\nalpha\n";
        // new_content marker is 15s after existing — below threshold
        let new_content = "[10:00:15]\nbeta\n";
        let merged = merge_session_texts(existing, new_content);
        let markers: Vec<&str> = merged.lines().filter(|l| is_marker_line(l)).collect();
        assert_eq!(
            markers.len(),
            1,
            "close marker should be suppressed; got:\n{merged}"
        );
    }

    #[test]
    fn merge_empty_new_returns_existing() {
        let existing = "[10:00:00]\nalpha\n";
        assert_eq!(merge_session_texts(existing, ""), existing);
    }

    #[test]
    fn terminal_pathology_split() {
        // No newlines, long string with 2-space separators
        let long = "cmd  output line one  output line two  output line three ".repeat(5);
        let frames = vec![frame("2024-01-01T10:00:00Z", &long)];
        let text = build_session_text(&frames);
        let content_lines: Vec<&str> = text.lines().filter(|l| !is_marker_line(l)).collect();
        assert!(
            content_lines.len() > 1,
            "terminal long line should be split; got {} line(s)",
            content_lines.len()
        );
    }

    // ── split_lines edge cases ────────────────────────────────────────────────

    #[test]
    fn split_lines_filters_ui_noise() {
        // VS Code sidebar icons, short OCR fragments — all dropped.
        // "fn main() {" is a code-signature landmark and must survive.
        let noisy = "•\n›\nCa\nLa\n@\nfn main() {\n";
        let result = split_lines(noisy, &no_chrome());
        assert_eq!(result, ["fn main() {"]);
    }

    #[test]
    fn split_lines_drops_short_non_landmark_words() {
        // "ok" (len 2) and "•" (len 1) are always dropped.
        // "cmd" (len 3, not a landmark) is dropped by MIN_LINE_LEN=15.
        // Only landmark lines would survive — none present here.
        let result = split_lines("ok\ncmd\n•\n", &no_chrome());
        assert!(
            result.is_empty(),
            "short non-landmark words must be dropped; got {result:?}"
        );
    }

    #[test]
    fn split_lines_keeps_quality_content() {
        // Long enough, high alpha ratio, not log noise.
        let result = split_lines(
            "cargo build released the binary\nrunning unit tests now",
            &no_chrome(),
        );
        assert_eq!(
            result,
            ["cargo build released the binary", "running unit tests now"]
        );
    }

    #[test]
    fn split_lines_drops_log_noise() {
        let noisy =
            "INFO:agents.server: request received from the client\ncargo build released the binary";
        let result = split_lines(noisy, &no_chrome());
        assert_eq!(result, ["cargo build released the binary"]);
    }

    #[test]
    fn split_lines_keeps_landmark_error() {
        // Error line is a landmark — survives regardless of alpha or length.
        let result = split_lines("error: unused variable in function body", &no_chrome());
        assert_eq!(result, ["error: unused variable in function body"]);
    }

    #[test]
    fn split_lines_chrome_dropped() {
        let mut chrome = HashSet::new();
        chrome.insert("File  Edit  View  Window".to_owned());
        let text = "File  Edit  View  Window\ncargo build completed successfully";
        let result = split_lines(text, &chrome);
        assert_eq!(result, ["cargo build completed successfully"]);
    }

    #[test]
    fn split_lines_empty_string() {
        assert_eq!(split_lines("", &no_chrome()), Vec::<String>::new());
    }

    #[test]
    fn split_lines_whitespace_only() {
        assert_eq!(
            split_lines("  \n  \n  ", &no_chrome()),
            Vec::<String>::new()
        );
    }

    #[test]
    fn split_lines_short_line_with_spaces_not_split() {
        let line = "word1  word2  word3"; // 19 chars — well under 200
        let result = split_lines(line, &no_chrome());
        assert_eq!(
            result.len(),
            1,
            "short line with two-space separator must not split; got {result:?}"
        );
        assert_eq!(result[0], line);
    }

    #[test]
    fn split_lines_long_line_splits_on_two_spaces() {
        let part_a = "a".repeat(100);
        let part_b = "b".repeat(100);
        let long = format!("{part_a}  {part_b}"); // 202 chars with "  " in the middle
        let result = split_lines(&long, &no_chrome());
        assert_eq!(
            result.len(),
            2,
            "long line with two-space separator should split; got {result:?}"
        );
        assert_eq!(result[0], "a".repeat(100));
        assert_eq!(result[1], "b".repeat(100));
    }

    #[test]
    fn split_lines_long_line_no_two_spaces_stays_whole() {
        // 240 chars of meaningful content, single spaces only — must not split on spaces
        let long = "ab cd ".repeat(40);
        let trimmed = long.trim();
        let result = split_lines(trimmed, &no_chrome());
        assert_eq!(
            result.len(),
            1,
            "long line with only single spaces must not split; got {result:?}"
        );
    }

    // ── ts_to_secs edge cases ─────────────────────────────────────────────────

    #[test]
    fn ts_to_secs_midnight() {
        assert_eq!(ts_to_secs("2026-01-01T00:00:00+00:00"), Some(0));
    }

    #[test]
    fn ts_to_secs_end_of_day() {
        assert_eq!(ts_to_secs("2026-01-01T23:59:59+00:00"), Some(86399));
    }

    #[test]
    fn ts_to_secs_invalid() {
        assert_eq!(ts_to_secs("not-a-timestamp"), None);
        assert_eq!(ts_to_secs(""), None);
    }

    // ── is_marker_line edge cases ─────────────────────────────────────────────

    #[test]
    fn is_marker_line_valid() {
        assert!(is_marker_line("[10:05:30]"));
        assert!(is_marker_line("[00:00:00]"));
        assert!(is_marker_line("[23:59:59]"));
    }

    #[test]
    fn is_marker_line_wrong_length() {
        assert!(!is_marker_line("[1:05:30]")); // 9 chars
        assert!(!is_marker_line("[10:05:300]")); // 11 chars
        assert!(!is_marker_line("")); // 0 chars
    }

    #[test]
    fn is_marker_line_no_colons() {
        assert!(!is_marker_line("[10-05-30]")); // dashes not colons
        assert!(!is_marker_line("[10:05-30]")); // mixed separators
    }

    // ── extract_last_marker_secs edge cases ───────────────────────────────────

    #[test]
    fn extract_last_marker_multiple() {
        let text = "[10:00:00]\nalpha\n[10:01:00]\nbeta\n[10:02:00]\ngamma\n";
        let secs = extract_last_marker_secs(text);
        assert_eq!(
            secs,
            Some(10 * 3600 + 2 * 60),
            "should return the LAST marker (10:02:00)"
        );
    }

    #[test]
    fn extract_last_marker_none() {
        assert_eq!(
            extract_last_marker_secs("no markers here\njust lines\n"),
            None
        );
        assert_eq!(extract_last_marker_secs(""), None);
    }

    // ── dedup_cursor_prefixes ─────────────────────────────────────────────────

    #[test]
    fn dedup_cursor_removes_typing_increments() {
        // Simulates mid-keystroke captures of the same message.
        let text = "[10:00:00]\n❯ wait s\n❯ wait s how\n❯ wait s how thsi\n❯ wait s how thsi work\nsome output\n";
        let result = dedup_cursor_prefixes(text.to_owned());
        let cursor: Vec<&str> = result.lines().filter(|l| l.starts_with("❯ ")).collect();
        assert_eq!(
            cursor,
            ["❯ wait s how thsi work"],
            "only the final version should survive; got:\n{result}"
        );
        // Non-cursor lines must be preserved.
        assert!(result.contains("some output"));
        assert!(result.contains("[10:00:00]"));
    }

    #[test]
    fn dedup_cursor_preserves_unrelated_short_command() {
        // "❯ git" is more than MAX_LOOKAHEAD cursor lines away from "❯ git status".
        let mut text = String::from("[10:00:00]\n❯ git\n");
        for i in 0..10 {
            text.push_str(&format!("❯ unrelated_{i}\n"));
        }
        text.push_str("❯ git status\n");
        let result = dedup_cursor_prefixes(text);
        assert!(
            result.contains("❯ git\n"),
            "❯ git must not be removed when superseder is beyond lookahead:\n{result}"
        );
        assert!(result.contains("❯ git status\n"));
    }

    #[test]
    fn dedup_cursor_no_cursor_lines_unchanged() {
        let text = "[10:00:00]\nalpha\nbeta\ngamma\n";
        let result = dedup_cursor_prefixes(text.to_owned());
        assert_eq!(result, text);
    }

    #[test]
    fn dedup_cursor_single_cursor_line_unchanged() {
        let text = "[10:00:00]\n❯ hello world\noutput\n";
        let result = dedup_cursor_prefixes(text.to_owned());
        assert_eq!(result, text);
    }

    #[test]
    fn dedup_cursor_equal_lines_not_removed() {
        // Same content twice (exact dupe) — neither supersedes the other.
        let text = "❯ hello\n❯ hello\n";
        let result = dedup_cursor_prefixes(text.to_owned());
        // Both survive (exact dedup is the seen-set's job, not ours).
        assert_eq!(result.lines().filter(|l| *l == "❯ hello").count(), 2);
    }

    #[test]
    fn dedup_cursor_build_session_text_integration() {
        // build_session_text must emit only the final version of a typed message.
        // ❯ lines are landmarks and always survive quality filtering.
        let frames = vec![
            frame("2024-01-01T10:00:00Z", "❯ fix\noutput line from process"),
            frame(
                "2024-01-01T10:00:01Z",
                "❯ fix bug\noutput line from process",
            ),
            frame(
                "2024-01-01T10:00:02Z",
                "❯ fix bug now\noutput line from process",
            ),
        ];
        let text = build_session_text(&frames);
        let cursor: Vec<&str> = text.lines().filter(|l| l.starts_with("❯ ")).collect();
        assert_eq!(
            cursor,
            ["❯ fix bug now"],
            "only the final typed version should appear; got:\n{text}"
        );
    }

    // ── chrome filtering integration ──────────────────────────────────────────

    #[test]
    fn build_drops_chrome_ui_lines() {
        // "File  Edit  View  Window  Help" appears in all 5 frames → chrome.
        // Content lines vary per frame so they appear in fewer than 4 frames → kept.
        let frames = vec![
            frame(
                "2024-01-01T10:00:00Z",
                "File  Edit  View  Window  Help\ncargo build completed successfully",
            ),
            frame(
                "2024-01-01T10:00:01Z",
                "File  Edit  View  Window  Help\nrunning unit tests for the module",
            ),
            frame(
                "2024-01-01T10:00:02Z",
                "File  Edit  View  Window  Help\ncompiler finished without any errors",
            ),
            frame(
                "2024-01-01T10:00:03Z",
                "File  Edit  View  Window  Help\ndeploying release binary to target",
            ),
            frame(
                "2024-01-01T10:00:04Z",
                "File  Edit  View  Window  Help\ndependency graph resolved successfully",
            ),
        ];
        let text = build_session_text(&frames);
        let content: Vec<&str> = text.lines().filter(|l| !is_marker_line(l)).collect();
        assert!(
            !content.contains(&"File  Edit  View  Window  Help"),
            "chrome line must be dropped; got content:\n{content:?}"
        );
        assert!(
            content.contains(&"cargo build completed successfully"),
            "real content must survive; got content:\n{content:?}"
        );
    }

    #[test]
    fn build_keeps_landmark_that_repeats_across_frames() {
        // An error message persisting across 5 frames is landmark — must NOT be chrome
        let frames: Vec<FrameText> = (0..5)
            .map(|i| {
                frame(
                    &format!("2024-01-01T10:00:{i:02}Z"),
                    "error: borrow checker failed on line 42\nFile  Edit  View  Window  Help",
                )
            })
            .collect();
        let text = build_session_text(&frames);
        let content: Vec<&str> = text.lines().filter(|l| !is_marker_line(l)).collect();
        assert!(
            content.contains(&"error: borrow checker failed on line 42"),
            "landmark repeated across frames must be kept; got content:\n{content:?}"
        );
    }

    // ── merge — pending_marker and all-duplicate cases ────────────────────────

    #[test]
    fn merge_only_marker_in_new_not_flushed() {
        let existing = "[10:00:00]\nalpha\n";
        // 60s gap qualifies, but new_content has no body lines after the marker
        let new_content = "[10:01:00]\n";
        let merged = merge_session_texts(existing, new_content);
        let markers: Vec<&str> = merged.lines().filter(|l| is_marker_line(l)).collect();
        assert_eq!(
            markers.len(),
            1,
            "pending marker without body must not be emitted; got:\n{merged}"
        );
        let content: Vec<&str> = merged.lines().filter(|l| !is_marker_line(l)).collect();
        assert_eq!(content, ["alpha"]);
    }

    #[test]
    fn merge_all_lines_already_seen_no_change() {
        let existing = "[10:00:00]\nalpha\nbeta\n";
        // 60s gap would qualify for a new marker, but all lines are already seen
        let new_content = "[10:01:00]\nalpha\nbeta\n";
        let merged = merge_session_texts(existing, new_content);
        let content: Vec<&str> = merged.lines().filter(|l| !is_marker_line(l)).collect();
        assert_eq!(
            content,
            ["alpha", "beta"],
            "duplicate lines must not be added; got:\n{merged}"
        );
        let markers: Vec<&str> = merged.lines().filter(|l| is_marker_line(l)).collect();
        assert_eq!(
            markers.len(),
            1,
            "no new content means no new marker; got:\n{merged}"
        );
    }

    // ── merge-path chrome limitation ──────────────────────────────────────────

    /// Documents the known limitation: chrome that leaks in a first ETL batch
    /// whose frame count is below CHROME_FREQ_THRESHOLD persists permanently in
    /// merged text, because `merge_session_texts` is additive and does not re-run
    /// the chrome pre-pass over the full combined frame history.
    ///
    /// This test is intentionally "asserting the bug" so that any future fix
    /// (e.g. a full re-build on session close) does not go unnoticed.
    #[test]
    fn merge_path_chrome_leaks_when_first_batch_below_threshold() {
        // "File  Edit  View" appears only 3 times in batch 1 → below threshold → NOT chrome.
        // It appears in all 4 frames of batch 2, but merge_session_texts never re-checks.
        let batch1_text = build_session_text(&[
            frame(
                "2024-01-01T10:00:00Z",
                "File  Edit  View\nreal work content line",
            ),
            frame(
                "2024-01-01T10:00:01Z",
                "File  Edit  View\nmore real work content",
            ),
            frame(
                "2024-01-01T10:00:02Z",
                "File  Edit  View\nthird real work content",
            ),
        ]);
        // batch1_text now contains "File  Edit  View" because 3 < CHROME_FREQ_THRESHOLD (4)

        let batch2_text = build_session_text(&[
            frame(
                "2024-01-01T10:01:00Z",
                "File  Edit  View\nfourth real work content",
            ),
            frame(
                "2024-01-01T10:01:01Z",
                "File  Edit  View\nfifth real work content",
            ),
            frame(
                "2024-01-01T10:01:02Z",
                "File  Edit  View\nsixth real work content",
            ),
            frame(
                "2024-01-01T10:01:03Z",
                "File  Edit  View\nseventh real work content",
            ),
        ]);
        // batch2_text does NOT contain "File  Edit  View" — 4 frames ≥ threshold

        let merged = merge_session_texts(&batch1_text, &batch2_text);

        // Document the limitation: the chrome line survives from batch 1.
        // If this assertion fails, the limitation has been fixed — update the test
        // to assert that "File  Edit  View" is absent and remove this comment.
        let content: Vec<&str> = merged.lines().filter(|l| !is_marker_line(l)).collect();
        assert!(
            content.contains(&"File  Edit  View"),
            "known limitation: chrome leaking from a sub-threshold first batch persists in merged text"
        );
        // Real content must not be lost regardless.
        assert!(content.contains(&"real work content line"));
        assert!(content.contains(&"fourth real work content"));
    }
}
