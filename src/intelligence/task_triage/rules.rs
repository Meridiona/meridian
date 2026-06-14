//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Pure, side-effect-free predicates for the deterministic ticket triage. Every
// function here takes primitives and returns a plain value so each rule is unit
// testable in isolation and the orchestrator (`mod.rs`) only has to compose them.
//
// Two hard-won edges live in this file:
//   1. Jira stamps `updated`/dates with a numeric offset and NO colon
//      ("2026-06-11T11:34:51.105+0530"), which is NOT valid RFC3339. A naive
//      `parse_from_rfc3339` returns Err on it, so age would read as "unknown" for
//      every Jira ticket and nothing would ever look stale. We fall back to a
//      `%z` format that accepts the colon-less offset.
//   2. Status names are arbitrary, user-defined strings ("In Review", "QA",
//      "Backlog"). We never substring-match (migration 036's lesson — "Incomplete"
//      contains "complete"); we split on word boundaries like the shared status
//      resolver does.

use chrono::{DateTime, NaiveDate, Utc};

/// How far along a ticket's status implies the work is. Deliberately three-valued:
/// `Unknown` (an unrecognised custom column) must never be read as "started",
/// because that would wrongly rescue a stale ticket from exclusion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Startedness {
    Started,
    NotStarted,
    Unknown,
}

/// Words that mean active work is underway. Word-boundary matched, never substring.
const STARTED_KEYWORDS: &[&str] = &[
    "progress",
    "doing",
    "wip",
    "review",
    "reviewing",
    "qa",
    "testing",
    "dev",
    "development",
    "implementing",
    "started",
    "active",
    "building",
    "ongoing",
    "inprogress",
];

/// Words that mean the work has not begun (it sits in a queue / backlog column).
const NOT_STARTED_KEYWORDS: &[&str] = &[
    "backlog",
    "todo",
    "open",
    "triage",
    "new",
    "icebox",
    "selected",
    "planned",
    "proposed",
    "draft",
    "upcoming",
    "scheduled",
];

/// Multi-word not-started columns that share a word with a started keyword
/// ("Selected for Development", "Ready for Dev") or split into non-keyword tokens
/// ("To Do"). Matched as whole phrases BEFORE the keyword pass so the queue meaning
/// wins over the incidental active word.
const NOT_STARTED_PHRASES: &[&str] = &[
    "to do",
    "selected for development",
    "ready for dev",
    "ready for development",
    "ready to start",
    "ready for work",
];

/// Generic one-word titles that carry no information to attribute work against.
const VAGUE_TITLE_WORDS: &[&str] = &[
    "fix", "fixes", "bug", "bugs", "update", "updates", "wip", "tmp", "temp", "test", "misc",
    "stuff", "todo", "task", "changes", "cleanup", "tweak", "tweaks", "various",
];

/// Classify a raw status name into how-started-it-is. Empty / unrecognised → Unknown.
pub(crate) fn startedness(status_raw: &str) -> Startedness {
    let lower = status_raw.to_ascii_lowercase();
    let words: Vec<&str> = lower
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect();
    if words.is_empty() {
        return Startedness::Unknown;
    }
    // 1. Known not-started phrases first — they share words with active columns
    //    ("Selected for Development") so must win over the keyword pass.
    let normalized = words.join(" ");
    if NOT_STARTED_PHRASES.iter().any(|p| normalized == *p) {
        return Startedness::NotStarted;
    }
    // 2. Any active word ("In Progress", "Ready for Review") ⇒ started.
    if words.iter().any(|w| STARTED_KEYWORDS.contains(w)) {
        return Startedness::Started;
    }
    // 3. Any queue word ⇒ not started.
    if words.iter().any(|w| NOT_STARTED_KEYWORDS.contains(w)) {
        return Startedness::NotStarted;
    }
    Startedness::Unknown
}

/// A title is vague if it is one or two generic words ("Fix bug", "Updates") with
/// no specific noun. A title of 3+ words is assumed specific enough to match.
pub(crate) fn is_vague_title(title: &str) -> bool {
    let words: Vec<&str> = title.split_whitespace().collect();
    if words.is_empty() {
        return true;
    }
    if words.len() >= 3 {
        return false;
    }
    // 1–2 words: vague only if every word is a generic filler word.
    words.iter().all(|w| {
        let clean: String = w
            .chars()
            .filter(|c| c.is_alphanumeric())
            .collect::<String>()
            .to_ascii_lowercase();
        clean.is_empty() || VAGUE_TITLE_WORDS.contains(&clean.as_str())
    })
}

/// Whole days between `updated_at` and `now`. `None` when the timestamp can't be
/// parsed — callers must treat unknown age as "not provably stale", never as old.
/// Clamps a future timestamp (clock skew) to 0 rather than returning a negative age.
pub(crate) fn age_days(updated_at: &str, now: DateTime<Utc>) -> Option<i64> {
    let ts = parse_ts(updated_at)?;
    Some((now - ts).num_days().max(0))
}

/// Days from `now` until `due` (date-only). Negative ⇒ overdue. `None` if unparseable.
pub(crate) fn days_until_due(due: &str, now: DateTime<Utc>) -> Option<i64> {
    let due_date = parse_date(due)?;
    Some((due_date - now.date_naive()).num_days())
}

/// Parse a timestamp that may be RFC3339 (`...Z` / `+05:30`) or the colon-less
/// numeric-offset variant Jira emits (`+0530`), with or without fractional seconds.
fn parse_ts(s: &str) -> Option<DateTime<Utc>> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Some(dt.with_timezone(&Utc));
    }
    // Colon-less offset fallback, e.g. "2026-06-11T11:34:51.105+0530".
    for fmt in ["%Y-%m-%dT%H:%M:%S%.f%z", "%Y-%m-%dT%H:%M:%S%z"] {
        if let Ok(dt) = DateTime::parse_from_str(s, fmt) {
            return Some(dt.with_timezone(&Utc));
        }
    }
    None
}

/// Parse a due/start value as a date. Accepts a bare `YYYY-MM-DD` or a full
/// timestamp (taking its date component).
fn parse_date(s: &str) -> Option<NaiveDate> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    if let Ok(d) = NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        return Some(d);
    }
    parse_ts(s).map(|dt| dt.date_naive())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 6, 12, 12, 0, 0).unwrap()
    }

    #[test]
    fn parses_jira_colonless_offset() {
        // The real format from the live DB — must not be unparseable.
        let age = age_days("2026-06-10T11:34:51.105+0530", now());
        assert_eq!(age, Some(2));
    }

    #[test]
    fn parses_rfc3339_z() {
        assert_eq!(age_days("2026-06-12T00:00:00Z", now()), Some(0));
        assert_eq!(age_days("2026-05-13T12:00:00Z", now()), Some(30));
    }

    #[test]
    fn unparseable_timestamp_is_none_not_old() {
        assert_eq!(age_days("not-a-date", now()), None);
        assert_eq!(age_days("", now()), None);
    }

    #[test]
    fn future_timestamp_clamps_to_zero() {
        assert_eq!(age_days("2026-07-01T00:00:00Z", now()), Some(0));
    }

    #[test]
    fn due_in_days_handles_overdue_and_future() {
        assert_eq!(days_until_due("2026-06-20", now()), Some(8));
        assert_eq!(days_until_due("2026-06-01", now()), Some(-11));
        assert_eq!(days_until_due("2026-06-12", now()), Some(0));
        assert_eq!(days_until_due("garbage", now()), None);
    }

    #[test]
    fn startedness_recognises_active_columns() {
        for s in [
            "In Progress",
            "Doing",
            "In Review",
            "QA",
            "Ready for Review",
        ] {
            assert_eq!(startedness(s), Startedness::Started, "{s}");
        }
    }

    #[test]
    fn startedness_recognises_queue_columns() {
        for s in [
            "Backlog",
            "To Do",
            "Open",
            "Triage",
            "Selected for Development",
        ] {
            assert_eq!(startedness(s), Startedness::NotStarted, "{s}");
        }
    }

    #[test]
    fn startedness_unknown_for_exotic_or_empty() {
        assert_eq!(startedness(""), Startedness::Unknown);
        assert_eq!(startedness("Blocked"), Startedness::Unknown);
    }

    #[test]
    fn vague_titles_flagged() {
        for t in ["Fix bug", "Updates", "WIP", "misc", "test task"] {
            assert!(is_vague_title(t), "{t} should be vague");
        }
    }

    #[test]
    fn specific_titles_not_flagged() {
        for t in [
            "Add dark-mode toggle to settings",
            "Integrate Stripe Checkout",
            "OAuth callback",
        ] {
            assert!(!is_vague_title(t), "{t} should be specific");
        }
    }
}
