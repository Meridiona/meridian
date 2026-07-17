//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Pure builders for the hour report — the model's input and the final rendering.
//!
//! A faithful Rust port of the pure helpers in the Python `worklog_pipeline/pipeline.py`
//! (`_local_hhmm`, `_window_label`, `_format_timeline_block`, `_format_coding_block`,
//! `parse_activities`, `hour_span_minutes`, `assemble_report`). Ported here because the
//! hour is now orchestrated in Rust so the generative stages can go through the user's
//! chosen provider ([`crate::llm`]) — the Python versions become dead once the pipeline
//! is retired.
//!
//! The one deliberate improvement over the port: the span math reuses
//! [`meridian_core::intervals`] (union over the real UTC timestamps, with a coding-agent
//! row capped to its engaged `duration_s`) instead of the Python's local-`HH:MM` minute
//! arithmetic. Same answer, but a parked-open agent window can no longer inflate the span,
//! and it is timezone-exact rather than truncated to whole local minutes.

use std::collections::BTreeMap;

use chrono::{DateTime, Local};
use meridian_core::intervals::{session_interval, union_seconds, Interval};
use serde_json::Value;

/// One measured session row for the hour (screen or coding-agent).
#[derive(Debug, Clone)]
pub struct TimelineRow {
    pub app_name: String,
    pub started_at: String,
    pub ended_at: String,
    pub duration_s: i64,
    /// Raw `app_sessions.window_titles` JSON blob (`[{"window_name":…,"count":N}]`).
    pub window_titles: String,
    pub is_coding: bool,
}

/// One coding-agent session that already carries an agent-written summary.
#[derive(Debug, Clone)]
pub struct CodingRow {
    pub app_name: String,
    pub started_at: String,
    pub ended_at: String,
    pub duration_s: i64,
    pub session_summary: String,
}

/// UTC RFC3339 timestamp → local `HH:MM`. Empty string if unparseable (mirrors the
/// Python `_local_hhmm`: the model simply gets a blank where a time can't be formed).
pub fn local_hhmm(ts: &str) -> String {
    DateTime::parse_from_rfc3339(ts)
        .map(|dt| dt.with_timezone(&Local).format("%H:%M").to_string())
        .unwrap_or_default()
}

/// Most-seen window title from an `app_sessions.window_titles` JSON blob. The title is
/// what actually names the work on screen (the repo, the doc page, the PR), so the
/// timeline reads as work rather than 40 lines of bare app names.
pub fn window_label(window_titles: &str) -> String {
    let Ok(Value::Array(titles)) = serde_json::from_str::<Value>(window_titles) else {
        return String::new();
    };
    titles
        .iter()
        .map(|t| {
            let count = t.get("count").and_then(Value::as_i64).unwrap_or(0);
            let name = t.get("window_name").and_then(Value::as_str).unwrap_or("");
            (count, name)
        })
        .max_by_key(|&(count, _)| count)
        .map(|(_, name)| name.trim().to_string())
        .unwrap_or_default()
}

/// Render the hour's MEASURED session timeline — the report model's time anchor. The
/// prompt forbids inventing clock times, so every timestamp the model emits must be
/// copied from here. Empty string when there are no rows.
pub fn format_timeline_block(rows: &[TimelineRow]) -> String {
    if rows.is_empty() {
        return String::new();
    }
    let mut parts = vec![
        "## MEASURED TIMELINE (ground truth — the ONLY source of clock times; \
         copy from here, never estimate)"
            .to_string(),
    ];
    for r in rows {
        let st = local_hhmm(&r.started_at);
        let en = local_hhmm(&r.ended_at);
        let mins = ((r.duration_s.max(0) as f64) / 60.0).round().max(1.0) as i64;
        let app = if r.app_name.is_empty() {
            "?"
        } else {
            &r.app_name
        };
        let tag = if r.is_coding { " [coding agent]" } else { "" };
        let win = window_label(&r.window_titles);
        let win = if win.is_empty() {
            String::new()
        } else {
            let w: String = win.chars().take(70).collect();
            format!(" — {w}")
        };
        parts.push(format!("{st}-{en}  {mins:>3} min  {app}{tag}{win}"));
    }
    parts.join("\n")
}

/// Render coding-agent summaries as a LABELED input section for the report model. Each
/// session's measured local span + active minutes are stamped from the DB (never asked of
/// a model — the summariser prompt is barred from mentioning time), then its agent-written
/// summary verbatim. The report prompt folds these into the same timed bullets as the
/// screen activity. Empty string when there are none.
pub fn format_coding_block(coding: &[CodingRow]) -> String {
    if coding.is_empty() {
        return String::new();
    }
    let mut parts = vec!["## CODING-AGENT SESSIONS THIS HOUR \
         (the developer drove these AI coding agents — this is their own work; \
         fold into the same timed bullets as the screen activity)"
        .to_string()];
    for c in coding {
        let st = local_hhmm(&c.started_at);
        let en = local_hhmm(&c.ended_at);
        let mins = ((c.duration_s.max(0) as f64) / 60.0).round().max(1.0) as i64;
        let app = if c.app_name.is_empty() {
            "Coding agent"
        } else {
            &c.app_name
        };
        let summary = c.session_summary.trim();
        parts.push(format!(
            "\n[{st}-{en} · {mins} min active · {app}]\n{summary}"
        ));
    }
    parts.join("\n")
}

/// Split call 1's numbered prose into an ordered list of activities. Anything that isn't a
/// numbered paragraph (a stray preamble, a trailing note) is dropped — it has no session
/// rows behind it. Mirrors the Python `parse_activities` (`^\s*(\d{1,2})[.)]\s+`).
pub fn parse_activities(prose: &str) -> Vec<String> {
    let mut out = Vec::new();
    for raw_line in prose.lines() {
        let line = raw_line.trim_start();
        // A line that opens "1. " / "12) " starts a new activity.
        let mut chars = line.char_indices();
        let mut digits = 0usize;
        let mut split_at = None;
        for (i, ch) in chars.by_ref() {
            if ch.is_ascii_digit() && digits < 2 {
                digits += 1;
            } else if (ch == '.' || ch == ')') && digits > 0 {
                split_at = Some(i + ch.len_utf8());
                break;
            } else {
                break;
            }
        }
        let Some(idx) = split_at else { continue };
        let rest = line[idx..].trim_start();
        if rest.is_empty() || line[..idx - 1].chars().any(|c| !c.is_ascii_digit()) {
            continue;
        }
        // Collapse internal whitespace, matching the Python `" ".join(body.split())`.
        out.push(rest.split_whitespace().collect::<Vec<_>>().join(" "));
    }
    out
}

/// Wall-clock minutes actually covered by the hour's sessions (union of intervals). The
/// ceiling for any single activity's estimate and the pool an unestimated activity gets a
/// share of. Uses the union (overlap counted once) with coding rows capped to their
/// engaged `duration_s` — reuses [`meridian_core::intervals`], the dashboard's own math.
pub fn hour_span_minutes(rows: &[TimelineRow]) -> i64 {
    if rows.is_empty() {
        return 0;
    }
    let intervals: Vec<Interval> = rows
        .iter()
        .map(|r| {
            session_interval(
                &r.started_at,
                &r.ended_at,
                r.duration_s,
                r.is_coding.then_some("coding"),
            )
        })
        .collect();
    ((union_seconds(&intervals) as f64) / 60.0).round() as i64
}

/// Render the final report: one line per activity, `"<HH:MM>  <minutes> min  <activity>"`.
///
/// `minutes` maps a 1-based activity number to the model's estimate; `stamps` maps the same
/// index to a rough local start time (`"HH:MM"`) so downstream knows when the activity
/// began — a line whose stamp is missing is rendered without the prefix. Two guards on the
/// minutes, both in code because the model is unreliable about either:
/// * **Clamp** each estimate to `[1, span]` — no activity can claim more time than the
///   hour's sessions cover.
/// * **Fill** an activity the model omitted by splitting the unaccounted time between the
///   omissions (≥1 min each), so an activity never silently loses its time.
///
/// `span` is [`hour_span_minutes`]; with no sessions it is 0 and estimates pass through
/// unclamped (nothing to check them against). Faithful port of Python `assemble_report`,
/// extended with the local-start-time prefix.
pub fn assemble_report(
    activities: &[String],
    minutes: &BTreeMap<usize, i64>,
    stamps: &BTreeMap<usize, String>,
    span: i64,
) -> String {
    if activities.is_empty() {
        return String::new();
    }
    let n = activities.len();

    let mut est: BTreeMap<usize, i64> = (1..=n)
        .filter_map(|i| minutes.get(&i).map(|&v| (i, v)))
        .collect();
    if span > 0 {
        for v in est.values_mut() {
            *v = (*v).clamp(1, span);
        }
    }

    let gaps: Vec<usize> = (1..=n).filter(|i| !est.contains_key(i)).collect();
    if !gaps.is_empty() {
        let used: i64 = est.values().sum();
        let left = if span > 0 { (span - used).max(0) } else { 0 };
        let share = (left / gaps.len() as i64).max(1);
        for i in gaps {
            est.insert(i, share);
        }
    }

    activities
        .iter()
        .enumerate()
        .map(|(idx, body)| {
            let m = est.get(&(idx + 1)).copied().unwrap_or(1);
            match stamps.get(&(idx + 1)) {
                Some(s) => format!("{s}  {m} min  {body}"),
                None => format!("{m} min  {body}"),
            }
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_activities_keeps_only_numbered_paragraphs() {
        let prose =
            "Here is the hour:\n1. Investigated the OAuth flow\n2.  Reviewed  PR  #12\nfooter note";
        assert_eq!(
            parse_activities(prose),
            vec![
                "Investigated the OAuth flow".to_string(),
                "Reviewed PR #12".to_string(),
            ]
        );
    }

    #[test]
    fn parse_activities_handles_paren_and_two_digit() {
        assert_eq!(
            parse_activities("10) Shipped it\n11. Tested it"),
            vec!["Shipped it".to_string(), "Tested it".to_string()]
        );
    }

    #[test]
    fn window_label_picks_the_most_seen_title() {
        let blob = r#"[{"window_name":"a","count":2},{"window_name":"  winner ","count":9}]"#;
        assert_eq!(window_label(blob), "winner");
        assert_eq!(window_label("not json"), "");
        assert_eq!(window_label("[]"), "");
    }

    #[test]
    fn assemble_clamps_and_fills() {
        let acts = vec!["A".to_string(), "B".to_string(), "C".to_string()];
        // B omitted; C over-claims the whole hour.
        let mut minutes = BTreeMap::new();
        minutes.insert(1, 10);
        minutes.insert(3, 999);
        let out = assemble_report(&acts, &minutes, &BTreeMap::new(), 60);
        let lines: Vec<&str> = out.split("\n\n").collect();
        assert_eq!(lines[0], "10 min  A");
        // C clamped to the 60-min span.
        assert!(lines[2].starts_with("60 min"), "{}", lines[2]);
        // B filled with a share of the remaining time, at least 1.
        assert!(lines[1].starts_with(|c: char| c.is_ascii_digit()));
        assert!(lines[1].ends_with("min  B") || lines[1].contains("min  B"));
    }

    #[test]
    fn assemble_all_omitted_splits_the_span_evenly() {
        let acts = vec!["A".to_string(), "B".to_string()];
        let out = assemble_report(&acts, &BTreeMap::new(), &BTreeMap::new(), 30);
        assert_eq!(out, "15 min  A\n\n15 min  B");
    }

    #[test]
    fn assemble_prefixes_the_local_start_time_when_present() {
        let acts = vec!["A".to_string(), "B".to_string()];
        let minutes = BTreeMap::from([(1, 10), (2, 20)]);
        // Only the first activity carries a stamp — the second renders without a prefix.
        let stamps = BTreeMap::from([(1, "07:45".to_string())]);
        let out = assemble_report(&acts, &minutes, &stamps, 60);
        assert_eq!(out, "07:45  10 min  A\n\n20 min  B");
    }

    #[test]
    fn assemble_empty_is_empty() {
        assert_eq!(
            assemble_report(&[], &BTreeMap::new(), &BTreeMap::new(), 60),
            ""
        );
    }

    #[test]
    fn hour_span_unions_overlap_once() {
        // Two 30-min screen sessions overlapping by 15 min → 45 min union, not 60.
        let rows = vec![
            TimelineRow {
                app_name: "A".into(),
                started_at: "2026-05-30T05:00:00+00:00".into(),
                ended_at: "2026-05-30T05:30:00+00:00".into(),
                duration_s: 1800,
                window_titles: "[]".into(),
                is_coding: false,
            },
            TimelineRow {
                app_name: "B".into(),
                started_at: "2026-05-30T05:15:00+00:00".into(),
                ended_at: "2026-05-30T05:45:00+00:00".into(),
                duration_s: 1800,
                window_titles: "[]".into(),
                is_coding: false,
            },
        ];
        assert_eq!(hour_span_minutes(&rows), 45);
    }

    #[test]
    fn hour_span_caps_a_parked_coding_row() {
        // A coding row parked open for an hour but engaged only 5 min counts as 5, not 60.
        let rows = vec![TimelineRow {
            app_name: "Claude Code".into(),
            started_at: "2026-05-30T05:00:00+00:00".into(),
            ended_at: "2026-05-30T06:00:00+00:00".into(),
            duration_s: 300,
            window_titles: "[]".into(),
            is_coding: true,
        }];
        assert_eq!(hour_span_minutes(&rows), 5);
    }

    #[test]
    fn timeline_block_empty_when_no_rows() {
        assert_eq!(format_timeline_block(&[]), "");
        assert_eq!(format_coding_block(&[]), "");
    }
}
