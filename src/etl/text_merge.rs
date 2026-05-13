// meridian — normalises screenpipe activity into structured app sessions

use std::collections::HashSet;

use crate::db::screenpipe::FrameText;

/// Minimum elapsed seconds between consecutive [HH:MM:SS] markers.
const MARKER_GAP_SECS: i64 = 30;

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Build session_text from scratch from an ordered sequence of frames.
/// Every unique line across all frames appears exactly once, in the order
/// it first appeared.  Timestamp markers ([HH:MM:SS]) are injected when
/// new content appears more than MARKER_GAP_SECS after the previous marker.
pub fn build_session_text(frames: &[FrameText]) -> String {
    let mut seen: HashSet<String> = HashSet::with_capacity(4096);
    let mut out = String::with_capacity(8192);
    let mut last_marker_secs: Option<i64> = None;

    for frame in frames {
        process_frame(frame, &mut seen, &mut out, &mut last_marker_secs);
    }
    out
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
) {
    let frame_secs = ts_to_secs(&frame.timestamp);
    let new_lines = split_lines(&frame.full_text);
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
/// Terminal OCR pathology: a single long string with no newlines is split
/// on 2+ consecutive spaces to recover phrase boundaries.
fn split_lines(full_text: &str) -> Vec<String> {
    let mut result: Vec<String> = Vec::new();
    for raw_line in full_text.split('\n') {
        let trimmed = raw_line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.len() > 200 && trimmed.contains("  ") {
            for part in trimmed.split("  ") {
                let p = part.trim();
                if !p.is_empty() && is_meaningful_line(p) {
                    result.push(p.to_owned());
                }
            }
        } else if is_meaningful_line(trimmed) {
            result.push(trimmed.to_owned());
        }
    }
    result
}

/// Returns true if a line contains real content worth storing.
/// Filters out VS Code sidebar icons (•, ›), short OCR fragments (Ca, La),
/// and other screenpipe UI-chrome noise.
///
/// A line is meaningful when it is at least 3 chars long AND contains a run
/// of 2+ consecutive alphanumeric/underscore characters (i.e., a real word).
fn is_meaningful_line(line: &str) -> bool {
    if line.len() < 3 {
        return false;
    }
    let mut run = 0u32;
    for c in line.chars() {
        if c.is_alphanumeric() || c == '_' {
            run += 1;
            if run >= 2 {
                return true;
            }
        } else {
            run = 0;
        }
    }
    false
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

    #[test]
    fn build_deduplicates_lines() {
        let frames = vec![
            frame("2024-01-01T10:00:00Z", "alpha\nbeta\ngamma"),
            frame("2024-01-01T10:00:01Z", "beta\ngamma\ndelta"),
            frame("2024-01-01T10:00:02Z", "alpha\ndelta\nepsilon"),
        ];
        let text = build_session_text(&frames);
        let lines: Vec<&str> = text.lines().filter(|l| !is_marker_line(l)).collect();
        assert_eq!(lines, ["alpha", "beta", "gamma", "delta", "epsilon"]);
    }

    #[test]
    fn build_emits_marker_on_first_frame() {
        let frames = vec![frame("2024-01-01T10:05:30Z", "hello")];
        let text = build_session_text(&frames);
        assert!(text.contains("[10:05:30]"), "expected marker; got:\n{text}");
    }

    #[test]
    fn build_suppresses_marker_within_threshold() {
        let frames = vec![
            frame("2024-01-01T10:00:00Z", "line1"),
            frame("2024-01-01T10:00:20Z", "line2"), // 20s < 30s threshold
        ];
        let text = build_session_text(&frames);
        let markers: Vec<&str> = text.lines().filter(|l| is_marker_line(l)).collect();
        assert_eq!(markers.len(), 1, "expected only one marker; got:\n{text}");
    }

    #[test]
    fn build_emits_marker_beyond_threshold() {
        let frames = vec![
            frame("2024-01-01T10:00:00Z", "line1"),
            frame("2024-01-01T10:01:00Z", "line2"), // 60s > 30s threshold
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

    // -------------------------------------------------------------------------
    // split_lines edge cases
    // -------------------------------------------------------------------------

    #[test]
    fn split_lines_filters_ui_noise() {
        // VS Code sidebar icons, short OCR fragments — all dropped
        let noisy = "•\n›\nCa\nLa\n@\nfn main() {\n";
        let result = split_lines(noisy);
        assert_eq!(result, ["fn main() {"]);
    }

    #[test]
    fn split_lines_keeps_short_meaningful_words() {
        // "cmd" (len 3, word run 3) must survive; "ok" (len 2) and "•" must not
        let result = split_lines("ok\ncmd\n•\n");
        assert_eq!(result, ["cmd"]);
    }

    #[test]
    fn split_lines_empty_string() {
        assert_eq!(split_lines(""), Vec::<String>::new());
    }

    #[test]
    fn split_lines_whitespace_only() {
        assert_eq!(split_lines("  \n  \n  "), Vec::<String>::new());
    }

    #[test]
    fn split_lines_short_line_with_spaces_not_split() {
        let line = "word1  word2  word3"; // 19 chars — well under 200
        let result = split_lines(line);
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
        let result = split_lines(&long);
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
        let result = split_lines(trimmed);
        assert_eq!(
            result.len(),
            1,
            "long line with only single spaces must not split; got {result:?}"
        );
    }

    // -------------------------------------------------------------------------
    // ts_to_secs edge cases
    // -------------------------------------------------------------------------

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

    // -------------------------------------------------------------------------
    // is_marker_line edge cases
    // -------------------------------------------------------------------------

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

    // -------------------------------------------------------------------------
    // extract_last_marker_secs edge cases
    // -------------------------------------------------------------------------

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

    // -------------------------------------------------------------------------
    // merge — pending_marker and all-duplicate cases
    // -------------------------------------------------------------------------

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
}
