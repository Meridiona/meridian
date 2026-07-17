//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! A day-task time **segment** — one approximate `HH:MM`-`HH:MM` local range a task was
//! worked in, plus the small, deterministic math over a set of them.
//!
//! # Why this exists
//! The Workstream Builder assigns each hour's work activities (which carry real
//! `HH:MM-HH:MM` ranges from the activity report) to their task and stores the task's
//! segments. A task can hold several non-contiguous segments — worked 08:15-08:45, then
//! 11:43-16:15 = two segments (a break in between). Segments are what let the timeline
//! draw a task spanning its real approximate start-end instead of snapping to whole hours.
//!
//! # What is (and isn't) code's job
//! **Grouping is the model's job, not code's.** Whether a short gap between two stretches
//! of the same work is "one continuous span" or "a real break" is a judgement the prompt
//! asks the model to make — there is deliberately **no gap-merge threshold here**. Code
//! only does correctness plumbing: parse `HH:MM`, drop invalid/zero-length, and
//! **coalesce ranges that actually overlap or touch** (so a carried-forward segment and
//! the new hour's segment for the same task don't draw as two bars over the same minutes,
//! and per-hour pieces that meet at an hour boundary read as one). Time totals
//! ([`union_minutes`]) count overlapping minutes once.
//!
//! # Who uses this
//! [`crate::worklog_pipeline::workstream`] (parse / sanitize / state) and
//! [`crate::worklog_pipeline::task_db`] (storage). Defined once here, shared by both.

use std::collections::BTreeSet;

use serde_json::{json, Value};

/// One approximate local time range, stored as minutes-from-midnight `[start_min, end_min)`.
/// `end_min` may be `1440` (24:00). Always `end_min > start_min` for a live segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Segment {
    pub start_min: i64,
    pub end_min: i64,
}

/// `"HH:MM"` → minutes-from-midnight (`"08:15"` → 495). Accepts `"24:00"` (→ 1440) as an
/// end-of-day marker. `None` for anything malformed or out of range.
pub fn parse_hhmm(s: &str) -> Option<i64> {
    let (h, m) = s.trim().split_once(':')?;
    let h: i64 = h.trim().parse().ok()?;
    let m: i64 = m.trim().parse().ok()?;
    if !(0..60).contains(&m) {
        return None;
    }
    if h == 24 && m == 0 {
        return Some(1440);
    }
    if !(0..24).contains(&h) {
        return None;
    }
    Some(h * 60 + m)
}

/// Minutes-from-midnight → `"HH:MM"` (495 → `"08:15"`, 1440 → `"24:00"`).
pub fn fmt_hhmm(min: i64) -> String {
    let min = min.clamp(0, 1440);
    format!("{:02}:{:02}", min / 60, min % 60)
}

/// Parse the model's `segments` array (`[{"start":"HH:MM","end":"HH:MM"}, …]`) into
/// [`Segment`]s, dropping any entry that is malformed or zero/negative length.
pub fn parse_segments(v: Option<&Value>) -> Vec<Segment> {
    let Some(arr) = v.and_then(Value::as_array) else {
        return Vec::new();
    };
    let raw = arr.iter().filter_map(|it| {
        let start = parse_hhmm(it.get("start")?.as_str()?)?;
        let end = parse_hhmm(it.get("end")?.as_str()?)?;
        (end > start).then_some(Segment {
            start_min: start,
            end_min: end,
        })
    });
    normalize(raw.collect())
}

/// Read a stored `segments_json` blob back into [`Segment`]s (same shape as the model's).
pub fn from_json_str(s: &str) -> Vec<Segment> {
    serde_json::from_str::<Value>(s)
        .ok()
        .map(|v| parse_segments(Some(&v)))
        .unwrap_or_default()
}

/// Render segments as the `[{"start":"HH:MM","end":"HH:MM"}, …]` JSON value the model
/// reads back (in the state prompt) and the DB stores.
pub fn to_value(segs: &[Segment]) -> Value {
    Value::Array(
        segs.iter()
            .map(|s| json!({"start": fmt_hhmm(s.start_min), "end": fmt_hhmm(s.end_min)}))
            .collect(),
    )
}

/// Render segments as a `segments_json` string for storage.
pub fn to_json_string(segs: &[Segment]) -> String {
    to_value(segs).to_string()
}

/// Sort ascending and coalesce ranges that overlap or exactly touch — correctness only,
/// **not** gap-grouping (that stays the model's job). Drops zero/negative-length ranges.
pub fn normalize(mut segs: Vec<Segment>) -> Vec<Segment> {
    segs.retain(|s| s.end_min > s.start_min);
    segs.sort_by_key(|s| s.start_min);
    let mut out: Vec<Segment> = Vec::new();
    for s in segs {
        match out.last_mut() {
            Some(last) if s.start_min <= last.end_min => last.end_min = last.end_min.max(s.end_min),
            _ => out.push(s),
        }
    }
    out
}

/// Total minutes covered by a task's segments, overlap counted once (segments are already
/// normalized by [`normalize`], so this is a plain sum, but it re-unions defensively).
pub fn union_minutes(segs: &[Segment]) -> i64 {
    normalize(segs.to_vec())
        .iter()
        .map(|s| s.end_min - s.start_min)
        .sum()
}

/// The local `"YYYY-MM-DDTHH"` hour labels a task's segments touch — the idempotency key
/// (was this hour folded?) and the source of the interim hour-block UI's span. A segment
/// `[start, end)` touches every hour from `start/60` to `(end-1)/60`.
pub fn hours_touched(segs: &[Segment], day_local: &str) -> Vec<String> {
    let mut hrs: BTreeSet<i64> = BTreeSet::new();
    for s in segs {
        if s.end_min <= s.start_min {
            continue;
        }
        let first = s.start_min / 60;
        let last = (s.end_min - 1) / 60;
        for h in first..=last {
            if (0..24).contains(&h) {
                hrs.insert(h);
            }
        }
    }
    hrs.into_iter()
        .map(|h| format!("{day_local}T{h:02}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(a: i64, b: i64) -> Segment {
        Segment {
            start_min: a,
            end_min: b,
        }
    }

    #[test]
    fn parse_and_format_hhmm_round_trip() {
        assert_eq!(parse_hhmm("08:15"), Some(495));
        assert_eq!(parse_hhmm("00:00"), Some(0));
        assert_eq!(parse_hhmm("24:00"), Some(1440));
        assert_eq!(fmt_hhmm(495), "08:15");
        assert_eq!(fmt_hhmm(1440), "24:00");
        // malformed / out of range
        assert_eq!(parse_hhmm("8h15"), None);
        assert_eq!(parse_hhmm("25:00"), None);
        assert_eq!(parse_hhmm("08:75"), None);
    }

    #[test]
    fn parse_segments_drops_invalid_and_coalesces_touching() {
        let v = json!([
            {"start":"08:15","end":"08:45"},
            {"start":"08:45","end":"09:00"},   // touches previous → coalesced
            {"start":"10:00","end":"09:00"},   // end < start → dropped
            {"start":"11:43","end":"12:00"},   // real gap → kept separate
        ]);
        let got = parse_segments(Some(&v));
        assert_eq!(got, vec![seg(495, 540), seg(703, 720)]);
    }

    #[test]
    fn normalize_counts_overlap_once() {
        // 08:15-08:45 ∪ 08:40-09:10 → 08:15-09:10 (carried-forward + new hour overlap)
        let got = normalize(vec![seg(495, 525), seg(520, 550)]);
        assert_eq!(got, vec![seg(495, 550)]);
        assert_eq!(union_minutes(&[seg(495, 525), seg(520, 550)]), 55);
    }

    #[test]
    fn union_keeps_real_breaks_separate() {
        // worked 08:15-08:45 (30) then 11:43-12:15 (32) → 62 min, two blocks
        let segs = vec![seg(495, 525), seg(703, 735)];
        assert_eq!(union_minutes(&segs), 62);
        assert_eq!(normalize(segs).len(), 2);
    }

    #[test]
    fn hours_touched_spans_the_covered_hours() {
        // 08:15-08:45 → [08]; a cross-hour segment 11:43-13:10 → [11,12,13]
        assert_eq!(
            hours_touched(&[seg(495, 525)], "2026-07-15"),
            vec!["2026-07-15T08"]
        );
        assert_eq!(
            hours_touched(&[seg(703, 790)], "2026-07-15"),
            vec!["2026-07-15T11", "2026-07-15T12", "2026-07-15T13"]
        );
    }

    #[test]
    fn json_round_trip() {
        let segs = vec![seg(495, 525), seg(703, 735)];
        let s = to_json_string(&segs);
        assert_eq!(from_json_str(&s), segs);
    }
}
