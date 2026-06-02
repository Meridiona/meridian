// meridian — normalises screenpipe activity into structured app sessions
//
// Shared worklog-comment helpers for the trackers that have NO native worklog
// API (Linear, GitHub). Neither exposes a "log N hours" primitive — verified
// against Linear's full GraphQL schema (zero worklog/timeEntry/custom-field) and
// GitHub's docs (time tracking is an open feature request). So for those two we
// mirror a Jira worklog row as a structured comment on the issue: a human-readable
// time line + the synthesised narrative, plus a machine marker carrying the exact
// window and seconds so an entry can be reconstructed/deduplicated programmatically.
//
// Jira does NOT use this — it has a real worklog endpoint (see `jira.rs`).

use anyhow::{Context, Result};
use chrono::{DateTime, Local, NaiveDateTime, Utc};

/// Provider-neutral result of a worklog post. `id` is the backend's identifier
/// for the created entry (a Jira worklog id, or a Linear/GitHub comment id) and
/// is stored in `pm_worklogs.posted_worklog_id`. `label` is a short human string
/// for logs (e.g. the time-spent "1h 30m").
#[derive(Debug, Clone)]
pub struct PostedWorklog {
    pub id: String,
    pub label: String,
}

/// Convert seconds → a human "1h 30m" / "45m" / "2h" label, rounding to the
/// nearest minute. Worklogs below 60s are never posted (the min-post floor), so
/// the minute is always the smallest unit shown.
pub fn seconds_to_human(seconds: i64) -> String {
    let minutes_total = ((seconds.max(0) + 30) / 60).max(1);
    let hours = minutes_total / 60;
    let minutes = minutes_total % 60;
    match (hours, minutes) {
        (h, m) if h > 0 && m > 0 => format!("{h}h {m}m"),
        (h, _) if h > 0 => format!("{h}h"),
        (_, m) => format!("{m}m"),
    }
}

/// Render a UTC ISO instant as a local "YYYY-MM-DD HH:MM" label for display in a
/// comment. Falls back to the raw string if it cannot be parsed.
pub fn local_label(utc_iso: &str) -> String {
    match parse_iso_utc(utc_iso) {
        Ok(utc) => utc
            .with_timezone(&Local)
            .format("%Y-%m-%d %H:%M")
            .to_string(),
        Err(_) => utc_iso.to_string(),
    }
}

/// Build the Markdown body for a worklog comment posted to Linear or GitHub.
///
/// The trailing HTML comment is a stable machine marker: callers (and future
/// readers) can grep `meridian-worklog` to find/parse logged entries and recover
/// the window + seconds even though neither tracker stores them as structured time.
pub fn format_worklog_comment(
    summary: &str,
    time_spent_seconds: i64,
    window_start_iso: &str,
    window_end_iso: &str,
) -> String {
    let human = seconds_to_human(time_spent_seconds);
    let when = local_label(window_start_iso);
    format!(
        "**⏱ Worklog — {human}** · {when}\n\n{summary}\n\n\
         <!-- meridian-worklog v1 window={window_start_iso}/{window_end_iso} seconds={time_spent_seconds} -->",
        summary = summary.trim()
    )
}

/// Parse a `YYYY-MM-DDTHH:MM:SSZ` / RFC3339 / naive-UTC instant into a UTC datetime.
pub(crate) fn parse_iso_utc(iso: &str) -> Result<DateTime<Utc>> {
    if let Ok(dt) = DateTime::parse_from_rfc3339(&iso.replace('Z', "+00:00")) {
        return Ok(dt.with_timezone(&Utc));
    }
    let naive = NaiveDateTime::parse_from_str(iso.trim_end_matches('Z'), "%Y-%m-%dT%H:%M:%S")
        .context("unrecognised timestamp format")?;
    Ok(DateTime::from_naive_utc_and_offset(naive, Utc))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn human_time_rounds_and_floors() {
        assert_eq!(seconds_to_human(60), "1m");
        assert_eq!(seconds_to_human(89), "1m"); // 1.48 → 1
        assert_eq!(seconds_to_human(90), "2m"); // 1.5 → 2
        assert_eq!(seconds_to_human(3600), "1h");
        assert_eq!(seconds_to_human(5400), "1h 30m");
        assert_eq!(seconds_to_human(0), "1m"); // floored to a visible minute
    }

    #[test]
    fn comment_carries_marker_and_summary() {
        let body = format_worklog_comment(
            "Did the thing.",
            5400,
            "2026-06-01T09:00:00Z",
            "2026-06-01T10:00:00Z",
        );
        assert!(body.contains("Did the thing."));
        assert!(body.contains("1h 30m"));
        assert!(body.contains(
            "meridian-worklog v1 window=2026-06-01T09:00:00Z/2026-06-01T10:00:00Z seconds=5400"
        ));
    }

    #[test]
    fn local_label_parses_utc() {
        // Just assert it produces a "YYYY-MM-DD HH:MM"-shaped string, not the raw input.
        let s = local_label("2026-06-01T09:00:00Z");
        assert_eq!(s.len(), 16, "expected 'YYYY-MM-DD HH:MM': {s}");
        assert!(s.contains('-') && s.contains(':'));
    }

    #[test]
    fn local_label_passthrough_on_garbage() {
        assert_eq!(local_label("not-a-date"), "not-a-date");
    }
}
