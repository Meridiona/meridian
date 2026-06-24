//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Wall-clock interval math — a faithful Rust port of `ui/lib/intervals.ts`,
//! the single source of the dashboard's "total time" math. Meridian stores two
//! overlapping recordings of the same time in `app_sessions` (foreground screen
//! capture + coding-agent transcript), so any total must UNION intervals, never
//! sum durations (which would double-count every overlapping second).

use chrono::{DateTime, SecondsFormat, TimeZone, Utc};
use serde::{Deserialize, Serialize};

/// A wall-clock span; timestamps are RFC3339 strings exactly as stored in the DB.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Interval {
    pub started_at: String,
    pub ended_at: String,
}

/// RFC3339 → epoch milliseconds (mirrors JS `new Date(s).getTime()`).
/// `None` for unparseable timestamps, so those rows are dropped — the Rust
/// equivalent of the TS `Number.isFinite` filter.
///
/// Shared by `week`, `today`, and `active` — declared here to avoid
/// re-implementing the same three-liner in each module.
pub(crate) fn parse_ms(s: &str) -> Option<i64> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.timestamp_millis())
}

/// epoch-ms → `2026-06-16T10:00:00.000Z` (mirrors JS `new Date(ms).toISOString()`).
fn ms_to_iso(ms: i64) -> String {
    Utc.timestamp_millis_opt(ms)
        .single()
        .map(|dt| dt.to_rfc3339_opts(SecondsFormat::Millis, true))
        .unwrap_or_default()
}

/// Parse → drop invalid (`end <= start` / unparseable) → sort by start → merge
/// overlapping or touching spans into a disjoint, ascending set of `(start,end)` ms.
fn normalize(intervals: &[Interval]) -> Vec<(i64, i64)> {
    let mut ivs: Vec<(i64, i64)> = intervals
        .iter()
        .filter_map(|r| {
            let s = parse_ms(&r.started_at)?;
            let e = parse_ms(&r.ended_at)?;
            (e > s).then_some((s, e))
        })
        .collect();
    ivs.sort_by_key(|&(s, _)| s);

    let mut out: Vec<(i64, i64)> = Vec::new();
    for (s, e) in ivs {
        match out.last_mut() {
            // overlaps/touches the previous span → extend it
            Some(last) if s <= last.1 => last.1 = last.1.max(e),
            _ => out.push((s, e)),
        }
    }
    out
}

/// Total wall-clock seconds covered by a set of intervals, overlap counted once.
pub fn union_seconds(intervals: &[Interval]) -> i64 {
    let total_ms: i64 = normalize(intervals).iter().map(|&(s, e)| e - s).sum();
    round_ms_to_s(total_ms)
}

/// Clamp every interval to the window `[lo, hi]` (RFC3339), keeping only the
/// portion that falls inside it and dropping any interval entirely outside.
///
/// Used to bound a daily total to `[start_of_day, min(now, end_of_day)]` so a
/// stale or cross-midnight block can't inflate "today" — e.g. an `active_session`
/// left open by a stopped daemon would otherwise span days. Unparseable bounds
/// degrade to a no-op (returns the input unchanged) rather than zeroing totals.
pub fn clamp_intervals(intervals: &[Interval], lo: &str, hi: &str) -> Vec<Interval> {
    let (Some(lo), Some(hi)) = (parse_ms(lo), parse_ms(hi)) else {
        return intervals.to_vec();
    };
    intervals
        .iter()
        .filter_map(|r| {
            let s = parse_ms(&r.started_at)?.max(lo);
            let e = parse_ms(&r.ended_at)?.min(hi);
            (e > s).then(|| Interval {
                started_at: ms_to_iso(s),
                ended_at: ms_to_iso(e),
            })
        })
        .collect()
}

/// Merge a set of intervals into a disjoint, ascending list — the timeline's
/// presence/agent bands are drawn from these.
pub fn merge_intervals(intervals: &[Interval]) -> Vec<Interval> {
    normalize(intervals)
        .into_iter()
        .map(|(s, e)| Interval {
            started_at: ms_to_iso(s),
            ended_at: ms_to_iso(e),
        })
        .collect()
}

/// Wall-clock interval for one `app_sessions` row, normalised across the two
/// streams. Foreground rows (`coding_agent_session_uuid` IS NULL) use their real span;
/// coding-agent rows are capped to their engaged `duration_s` anchored at the
/// start, so a parked-open agent window can't masquerade as hours of activity.
pub fn session_interval(
    started_at: &str,
    ended_at: &str,
    duration_s: i64,
    coding_agent_session_uuid: Option<&str>,
) -> Interval {
    if coding_agent_session_uuid.is_some() {
        let start_ms = parse_ms(started_at).unwrap_or(0);
        let end_ms = start_ms + duration_s.max(0) * 1000;
        return Interval {
            started_at: started_at.to_string(),
            ended_at: ms_to_iso(end_ms),
        };
    }
    Interval {
        started_at: started_at.to_string(),
        ended_at: ended_at.to_string(),
    }
}

/// Total seconds where set `a` and set `b` overlap (e.g. agent time that ran
/// while you were foreground-active → "supervised"). Both sides are normalized,
/// then swept together in linear time.
pub fn intersect_seconds(a: &[Interval], b: &[Interval]) -> i64 {
    let a = normalize(a);
    let b = normalize(b);
    let (mut i, mut j, mut total_ms) = (0usize, 0usize, 0i64);
    while i < a.len() && j < b.len() {
        let lo = a[i].0.max(b[j].0);
        let hi = a[i].1.min(b[j].1);
        if hi > lo {
            total_ms += hi - lo;
        }
        // advance whichever interval ends first; advance both on a tie so
        // neither re-enters against a following interval starting right there
        if a[i].1 < b[j].1 {
            i += 1;
        } else if a[i].1 > b[j].1 {
            j += 1;
        } else {
            i += 1;
            j += 1;
        }
    }
    round_ms_to_s(total_ms)
}

/// One foreground row for switch counting.
pub struct SwitchSession {
    pub app: String,
    pub started_at: String,
    pub dur: i64,
}

/// Count genuine context switches in a time-ordered foreground stream: app
/// changes between consecutive sessions, after dropping sub-`min_duration_s`
/// focus jitter (screenpipe's rapid focus flicker is not a real switch).
pub fn count_switches(sessions: &[SwitchSession], min_duration_s: i64) -> i64 {
    let mut ordered: Vec<&SwitchSession> = sessions
        .iter()
        .filter(|s| s.dur >= min_duration_s)
        .collect();
    ordered.sort_by_key(|s| parse_ms(&s.started_at).unwrap_or(0));

    ordered.windows(2).filter(|w| w[0].app != w[1].app).count() as i64
}

/// `Math.round(ms / 1000)` — half-up for the non-negative totals we deal with.
fn round_ms_to_s(ms: i64) -> i64 {
    (ms as f64 / 1000.0).round() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn iv(s: &str, e: &str) -> Interval {
        Interval {
            started_at: s.into(),
            ended_at: e.into(),
        }
    }

    #[test]
    fn union_counts_overlap_once() {
        // 10:00–10:10 ∪ 10:05–10:15 → 10:00–10:15 = 900 s
        let ivs = vec![
            iv("2026-06-16T10:00:00+00:00", "2026-06-16T10:10:00+00:00"),
            iv("2026-06-16T10:05:00+00:00", "2026-06-16T10:15:00+00:00"),
        ];
        assert_eq!(union_seconds(&ivs), 900);
    }

    #[test]
    fn union_drops_invalid_rows() {
        let ivs = vec![
            iv("2026-06-16T10:00:00+00:00", "2026-06-16T09:00:00+00:00"), // end < start
            iv("not-a-date", "2026-06-16T10:10:00+00:00"),                // unparseable
            iv("2026-06-16T10:00:00+00:00", "2026-06-16T10:01:00+00:00"), // 60 s
        ];
        assert_eq!(union_seconds(&ivs), 60);
    }

    #[test]
    fn merge_outputs_utc_z_millis() {
        let ivs = vec![iv(
            "2026-06-16T10:00:00.000+00:00",
            "2026-06-16T10:01:00.000+00:00",
        )];
        let m = merge_intervals(&ivs);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].started_at, "2026-06-16T10:00:00.000Z");
        assert_eq!(m[0].ended_at, "2026-06-16T10:01:00.000Z");
    }

    #[test]
    fn intersect_counts_overlap_only() {
        // a 10:00–10:10, b 10:05–10:20 → overlap 10:05–10:10 = 300 s
        let a = vec![iv("2026-06-16T10:00:00+00:00", "2026-06-16T10:10:00+00:00")];
        let b = vec![iv("2026-06-16T10:05:00+00:00", "2026-06-16T10:20:00+00:00")];
        assert_eq!(intersect_seconds(&a, &b), 300);
    }

    #[test]
    fn intersect_equal_endpoints_counted_once() {
        // Both intervals end at the same point — advance-both-on-tie means the
        // endpoint is not double-counted if subsequent intervals start there.
        let a = vec![iv("2026-06-16T10:00:00+00:00", "2026-06-16T10:10:00+00:00")];
        let b = vec![iv("2026-06-16T10:05:00+00:00", "2026-06-16T10:10:00+00:00")];
        assert_eq!(intersect_seconds(&a, &b), 300);
    }

    #[test]
    fn session_interval_caps_agent_but_not_foreground() {
        // agent row parked open 8 h but engaged only 120 s → ends at +120 s
        let agent = session_interval(
            "2026-06-16T10:00:00+00:00",
            "2026-06-16T18:00:00+00:00",
            120,
            Some("uuid"),
        );
        assert_eq!(agent.ended_at, "2026-06-16T10:02:00.000Z");
        // foreground row keeps its real ended_at
        let fg = session_interval(
            "2026-06-16T10:00:00+00:00",
            "2026-06-16T10:30:00+00:00",
            0,
            None,
        );
        assert_eq!(fg.ended_at, "2026-06-16T10:30:00+00:00");
    }

    #[test]
    fn clamp_bounds_drops_outside_and_trims_overlap() {
        let lo = "2026-06-18T00:00:00+00:00";
        let hi = "2026-06-18T12:00:00+00:00";
        let ivs = vec![
            // entirely before the window (a stale block from a prior day) → dropped
            iv("2026-06-16T04:33:00+00:00", "2026-06-16T05:00:00+00:00"),
            // straddles the lower bound → trimmed to [00:00, 01:00) = 3600 s
            iv("2026-06-17T23:00:00+00:00", "2026-06-18T01:00:00+00:00"),
            // runs past the upper bound → trimmed to [11:00, 12:00) = 3600 s
            iv("2026-06-18T11:00:00+00:00", "2026-06-18T20:00:00+00:00"),
        ];
        let clamped = clamp_intervals(&ivs, lo, hi);
        assert_eq!(clamped.len(), 2);
        assert_eq!(union_seconds(&clamped), 7200);
    }

    #[test]
    fn switches_ignore_short_jitter() {
        let s = |app: &str, at: &str, dur: i64| SwitchSession {
            app: app.into(),
            started_at: at.into(),
            dur,
        };
        // 5 s Chrome flicker dropped → Code → Code → 0 switches
        let jittery = vec![
            s("Code", "2026-06-16T10:00:00+00:00", 600),
            s("Chrome", "2026-06-16T10:10:00+00:00", 5),
            s("Code", "2026-06-16T10:11:00+00:00", 600),
        ];
        assert_eq!(count_switches(&jittery, 15), 0);
        // real Chrome session in the middle → 2 switches
        let real = vec![
            s("Code", "2026-06-16T10:00:00+00:00", 600),
            s("Chrome", "2026-06-16T10:10:00+00:00", 600),
            s("Code", "2026-06-16T10:20:00+00:00", 600),
        ];
        assert_eq!(count_switches(&real, 15), 2);
    }
}
