//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! A day's evidence, gathered into the named datasets its summary is composed from.
//!
//! # Two kinds of output, on purpose
//! - **Datasets** ([`Evidence::datasets`]) are chartable rows. They are what a
//!   summary panel's spec binds to by name, and they are injected into the chart at
//!   render — the model only ever names them, never their values.
//! - **Prose evidence** (workstream log lines, the hourly reports) is what the
//!   narrative is written FROM. It is not chartable and never becomes a dataset:
//!   putting free text on an axis is how you get a chart nobody can read.
//!
//! # Why this is in meridian-core and not with the rest of day_summary
//! It is a pure DB read with no LLM in it, and BOTH sides need it: the daemon to
//! build the prompt, and the tray to inject the real rows at render (a stored spec
//! carries form only — see migration 064). Keeping it here means the tray reads it
//! directly instead of spawning the CLI on every screen open, and there is exactly
//! one definition of "what happened that day".
//!
//! # Reuse, don't re-query
//! Everything comes from the existing readers ([`crate::day_tasks`],
//! [`crate::today`], [`crate::coding_agents`], [`crate::hour_text`]). A second
//! implementation of "what happened today" would drift from the timeline, and a
//! summary that disagrees with the screen beside it is worse than no summary.
//!
//! # The daily plan is deliberately absent
//! This is what you DID, not what you promised. The plan is a different question
//! and mixing it in would turn a review into a scorecard.
//!
//! # Related
//! - [`datasets`] — declares the names and fields this must produce.
//! - [`crate::day_summaries`] — where the composed summary is persisted.

pub mod datasets;

use crate::intervals::{intersect_seconds, Interval};
use crate::SqlitePool;
use anyhow::Context;
use serde::Serialize;
use serde_json::{json, Value};

/// Everything the model is shown about a day.
#[derive(Debug, Clone, Serialize)]
pub struct Evidence {
    pub day: String,
    /// name → array of row objects. Keys match [`datasets::DATASETS`].
    pub datasets: serde_json::Map<String, Value>,
    /// Day-level totals that need no chart to be interesting.
    pub scalars: Value,
    /// Per-workstream running log lines — the prose the narrative is built from.
    pub workstream_logs: Vec<WorkstreamLog>,
    /// `(hour, report)` for hours that reached the report stage.
    pub hour_reports: Vec<(i64, String)>,
}

/// One workstream's title and its running log, for the prose side.
#[derive(Debug, Clone, Serialize)]
pub struct WorkstreamLog {
    pub task_id: String,
    pub title: String,
    pub minutes: i64,
    pub lines: Vec<String>,
}

/// `"HH:MM"` → minutes past local midnight. `"24:00"` (the readers' end-of-day
/// marker) lands on 1440, which is what a chart wants.
fn hhmm_to_min(s: &str) -> Option<i64> {
    let (h, m) = s.split_once(':')?;
    let h: i64 = h.trim().parse().ok()?;
    let m: i64 = m.trim().parse().ok()?;
    if !(0..=24).contains(&h) || !(0..60).contains(&m) {
        return None;
    }
    Some(h * 60 + m)
}

/// `cat`, defensively normalised.
///
/// [`crate::today`] already maps `fm_parse_error`/`fm_skip` →
/// `idle_personal` for CLOSED sessions, but not for the live/active block. That is
/// harmless in the timeline, which never renders the raw value; here it would put
/// a literal `fm_parse_error` on a chart axis. `today`'s own `normalize_cat` is
/// private to that module, so the rule is restated rather than imported — if it
/// ever gains a case, this needs the same one.
fn normalize_cat(cat: &str) -> &str {
    match cat {
        "fm_parse_error" | "fm_skip" | "" => "idle_personal",
        c => c,
    }
}

/// The UTC instant range of local hour `h` on `day`, as an [`Interval`].
fn hour_interval(day: &str, h: i64) -> Option<Interval> {
    use chrono::{Duration, Local, NaiveDate, TimeZone};
    let date = NaiveDate::parse_from_str(day, "%Y-%m-%d").ok()?;
    let midnight = Local
        .from_local_datetime(&date.and_hms_opt(0, 0, 0)?)
        .single()?;
    let start = (midnight + Duration::hours(h)).to_utc();
    let end = (midnight + Duration::hours(h + 1)).to_utc();
    Some(Interval {
        started_at: start.to_rfc3339(),
        ended_at: end.to_rfc3339(),
    })
}

/// Build the day's evidence.
// Named explicitly: `#[instrument]` defaults to the fn name ("collect"), which
// says nothing in a trace tree. The dotted form matches the house convention and
// is what the daily-summary dashboard groups on.
#[tracing::instrument(name = "day_evidence.collect", skip(pool))]
pub async fn collect(pool: &SqlitePool, day: &str) -> anyhow::Result<Evidence> {
    let now = chrono::Utc::now().to_rfc3339();
    let today = crate::today::get_today(pool, day, &now)
        .await
        .context("day_evidence: get_today")?;
    let day_tasks = crate::day_tasks::get_day_tasks(pool, day)
        .await
        .context("day_evidence: get_day_tasks")?;
    let agents = crate::coding_agents::get_coding_agents(pool, day)
        .await
        .context("day_evidence: get_coding_agents")?;
    let reports = crate::hour_text::get_hour_reports(pool, day)
        .await
        .context("day_evidence: get_hour_reports")?;

    // ── workstreams + segments ────────────────────────────────────────────────
    let mut workstreams: Vec<Value> = Vec::new();
    let mut segments: Vec<Value> = Vec::new();
    let mut workstream_logs: Vec<WorkstreamLog> = Vec::new();

    for t in &day_tasks.tasks {
        let mins: Vec<(i64, i64)> = t
            .segments
            .iter()
            .filter_map(|s| Some((hhmm_to_min(&s.start)?, hhmm_to_min(&s.end)?)))
            .filter(|(a, b)| b >= a)
            .collect();

        let first_min = mins.iter().map(|(a, _)| *a).min();
        let last_min = mins.iter().map(|(_, b)| *b).max();

        for (a, b) in &mins {
            segments.push(json!({
                "task_id": t.id,
                "title": t.title,
                "start_min": a,
                "end_min": b,
                "minutes": b - a,
            }));
        }

        workstreams.push(json!({
            "task_id": t.id,
            "title": t.title,
            // `minutes` is the reader's own summed-segment figure — never
            // recomputed here, so the summary and the timeline can't disagree.
            "minutes": t.minutes,
            "segment_count": mins.len(),
            "first_min": first_min.unwrap_or(0),
            "last_min": last_min.unwrap_or(0),
        }));

        workstream_logs.push(WorkstreamLog {
            task_id: t.id.clone(),
            title: t.title.clone(),
            minutes: t.minutes,
            lines: t.summary.clone(),
        });
    }

    // ── apps ──────────────────────────────────────────────────────────────────
    // Coding-agent sessions are excluded from `today.sessions` server-side (they
    // are a separate overlay stream), so their time is folded back in from
    // get_coding_agents — the same move TimeByApp makes. Skip it and every agent
    // hour silently vanishes from the chart.
    let mut app_totals: std::collections::BTreeMap<String, i64> = Default::default();
    for s in &today.sessions {
        *app_totals.entry(s.app.clone()).or_default() += s.dur;
    }
    for a in &agents.agents {
        *app_totals.entry(a.app.clone()).or_default() += a.total_s;
    }
    let mut apps: Vec<Value> = app_totals
        .into_iter()
        .filter(|(_, s)| *s > 0)
        .map(|(app, seconds)| json!({"app": app, "seconds": seconds}))
        .collect();
    apps.sort_by_key(|v| -v["seconds"].as_i64().unwrap_or(0));

    // ── categories ────────────────────────────────────────────────────────────
    // Agent time is coding, just delegated — folded into the coding slice exactly
    // as TimeByCategory's `categoryRows` does.
    let mut cat_totals: std::collections::BTreeMap<String, i64> = Default::default();
    for s in &today.sessions {
        *cat_totals
            .entry(normalize_cat(&s.cat).to_string())
            .or_default() += s.dur;
    }
    if today.agent_s > 0 {
        *cat_totals.entry("coding".to_string()).or_default() += today.agent_s;
    }
    let mut categories: Vec<Value> = cat_totals
        .into_iter()
        .filter(|(_, s)| *s > 0)
        .map(|(category, seconds)| json!({"category": category, "seconds": seconds}))
        .collect();
    categories.sort_by_key(|v| -v["seconds"].as_i64().unwrap_or(0));

    // ── hours ─────────────────────────────────────────────────────────────────
    // Every hour 0..23 is emitted, zeros included: a chart of only the busy hours
    // hides the shape of the day, which is usually the interesting part.
    let hours: Vec<Value> = (0..24)
        .map(|h| {
            let (focus_s, agent_s) = match hour_interval(day, h) {
                Some(iv) => {
                    let win = [iv];
                    (
                        intersect_seconds(&today.presence_segments, &win),
                        intersect_seconds(&today.agent_segments, &win),
                    )
                }
                // A DST spring-forward gap has no such local hour.
                None => (0, 0),
            };
            json!({"hour": h, "focus_s": focus_s, "agent_s": agent_s})
        })
        .collect();

    let mut datasets = serde_json::Map::new();
    datasets.insert("workstreams".into(), Value::Array(workstreams));
    datasets.insert("segments".into(), Value::Array(segments));
    datasets.insert("apps".into(), Value::Array(apps));
    datasets.insert("categories".into(), Value::Array(categories));
    datasets.insert("hours".into(), Value::Array(hours));

    let hour_reports: Vec<(i64, String)> = reports
        .hours
        .into_iter()
        .filter_map(|h| h.report.map(|r| (h.hour, r)))
        .collect();

    let scalars = json!({
        "focus_s": today.focus_s,
        "idle_s": today.idle_s,
        "engaged_s": today.engaged_s,
        "agent_s": today.agent_s,
        "session_count": today.session_count,
        "switch_count": today.switch_count,
        "workstream_count": day_tasks.tasks.len(),
    });

    for (name, rows) in &datasets {
        tracing::debug!(dataset = %name, rows = rows.as_array().map(|a| a.len()).unwrap_or(0));
    }
    tracing::info!(
        day,
        workstreams = day_tasks.tasks.len(),
        segments = datasets["segments"]
            .as_array()
            .map(|a| a.len())
            .unwrap_or(0),
        apps = datasets["apps"].as_array().map(|a| a.len()).unwrap_or(0),
        hour_reports = hour_reports.len(),
        "day evidence collected"
    );

    Ok(Evidence {
        day: day.to_string(),
        datasets,
        scalars,
        workstream_logs,
        hour_reports,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_clock_strings_including_the_end_of_day_marker() {
        assert_eq!(hhmm_to_min("00:27"), Some(27));
        assert_eq!(hhmm_to_min("13:56"), Some(836));
        // The readers use "24:00" for end-of-day; it must land at 1440, not fail.
        assert_eq!(hhmm_to_min("24:00"), Some(1440));
        assert_eq!(hhmm_to_min("nope"), None);
        assert_eq!(hhmm_to_min("25:00"), None);
        assert_eq!(hhmm_to_min("10:75"), None);
    }

    /// The live/active block can still carry a raw `fm_*` category, and that value
    /// would otherwise be rendered as a literal axis label.
    #[test]
    fn normalises_the_categories_the_active_block_can_leak() {
        assert_eq!(normalize_cat("fm_parse_error"), "idle_personal");
        assert_eq!(normalize_cat("fm_skip"), "idle_personal");
        assert_eq!(normalize_cat(""), "idle_personal");
        assert_eq!(normalize_cat("coding"), "coding");
    }

    #[test]
    fn an_hour_interval_spans_exactly_one_hour() {
        let iv = hour_interval("2026-07-16", 9).unwrap();
        let a = chrono::DateTime::parse_from_rfc3339(&iv.started_at).unwrap();
        let b = chrono::DateTime::parse_from_rfc3339(&iv.ended_at).unwrap();
        assert_eq!((b - a).num_seconds(), 3600);
    }

    #[test]
    fn a_bad_day_string_yields_no_hour_interval() {
        assert!(hour_interval("not-a-day", 9).is_none());
    }
}
