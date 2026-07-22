//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! A day's evidence: what was planned, what was actually done, and the aggregate
//! shape of the day, gathered into the named datasets its summary is composed from.
//!
//! # Three kinds of output, on purpose
//! - **Datasets** ([`Evidence::datasets`]) are aggregate rows — time by app, by
//!   category, by hour, and the day's sittings. They are the deterministic shape of
//!   the day, and the model only ever names them, never their values.
//! - **Prose evidence** (workstream log lines, the hourly reports) is what the
//!   insight cards are written FROM.
//! - **The plan** ([`Evidence::plan`]) and the resolved [`Evidence::ledger`] —
//!   the planned-vs-actual side, settled deterministically from the worklog match
//!   map ([`adherence::resolve_deterministic`]), no model involved.
//!
//! # Why this is in meridian-core and not with the rest of day_summary
//! It is a pure DB read with no LLM in it, and BOTH sides need it: the daemon to
//! build the prompt, and the tray to serve the screen's deterministic half. Keeping
//! it here means the tray reads it directly instead of spawning the CLI on every
//! screen open, and there is exactly one definition of "what happened that day".
//!
//! # Reuse, don't re-query
//! Everything comes from the existing readers ([`crate::day_tasks`],
//! [`crate::today`], [`crate::coding_agents`], [`crate::hour_text`],
//! [`crate::plan`]). A second implementation of "what happened today" would drift
//! from the timeline, and a summary that disagrees with the screen beside it is
//! worse than no summary.
//!
//! # The plan used to be deliberately absent
//! It was, on the grounds that mixing intent into a review turns it into a
//! scorecard. That was right about the risk and wrong about the fix: the question a
//! person actually has at the end of a day is whether it went the way they meant it
//! to, and refusing to answer it does not make the screen kinder, only less useful.
//! The scorecard risk is handled where it belongs — in the prompt's tone contract,
//! which forbids grading the person.
//!
//! # Related
//! - [`datasets`] — declares the names and fields this must produce.
//! - [`adherence`] — resolves the deterministic ledger from the worklog match map.
//! - [`crate::day_task_worklogs::targets`] — the match map's source.
//! - [`crate::day_summaries`] — where the composed summary is persisted.

pub mod adherence;
pub mod datasets;

use crate::intervals::{intersect_seconds, Interval};
use crate::plan::{DayPlan, PlanItem};
use crate::SqlitePool;
use anyhow::Context;
use serde::Serialize;
use serde_json::{json, Value};

/// How long a workstream must run to count as something you *did*.
///
/// Below this it is a detour, a glance, or a context switch that happened to earn
/// a title. It stays in the datasets and on the timeline - it really happened -
/// but it is excluded from `task_count`, the one number the summary says out loud.
/// A count that includes every three-minute glance is a number the reader can see
/// through immediately, and an inflated compliment reads as no compliment at all.
pub const TASK_MIN_MINUTES: i64 = 30;

/// Everything the model is shown about a day.
#[derive(Debug, Clone, Serialize)]
pub struct Evidence {
    pub day: String,
    /// name → array of row objects. Keys match [`datasets::DATASETS`].
    pub datasets: serde_json::Map<String, Value>,
    /// Day-level totals that need no chart to be interesting.
    pub scalars: Value,
    /// Per-workstream running log lines — the prose the narrative is built from.
    /// **Substantial workstreams only** (see [`TASK_MIN_MINUTES`]), so the model
    /// cannot name a piece of work the screen will not show.
    pub workstream_logs: Vec<WorkstreamLog>,
    /// `(hour, report)` for hours that reached the report stage.
    pub hour_reports: Vec<(i64, String)>,
    /// The day's committed plan, in plan order. Empty when there was none.
    #[serde(skip)]
    pub plan: Vec<PlanItem>,
    /// The deterministic plan ledger: one verdict per committed ticket and the
    /// achievement arithmetic, resolved from the worklog match map with no LLM (see
    /// [`adherence::resolve_deterministic`]). Empty verdicts + zeroed adherence when
    /// the day had no plan. This is the source of truth for the ring and the
    /// checklist; the model only writes the prose beside it.
    #[serde(skip)]
    pub ledger: adherence::DayLedger,
    /// Whether that plan was actually committed — `confirmed && !skipped && !empty`.
    /// The one flag callers should branch on; see [`DayPlan::is_planned`].
    pub planned: bool,
    /// Every day-task, unfiltered. The ledger needs the brief ones too: a matched
    /// day-task credits its ticket however few minutes it ran, and dropping it here
    /// would drop that credit.
    #[serde(skip)]
    pub tasks: Vec<crate::day_tasks::DayTask>,
    /// The newest tracked activity in the day, RFC3339, or empty when the day is
    /// empty. Stamped onto the summary so a later open can tell whether the day has
    /// moved on since it was composed.
    pub evidence_at: String,
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

/// `minutes` past local midnight on `day`, as an RFC3339 instant.
///
/// Used to stamp the newest activity a summary was composed from. Deriving it from
/// the workstream segments rather than a `MAX(timestamp)` query keeps this a pure
/// function of what the model was actually shown - the point of the stamp is "has
/// the thing I summarised changed", and a clock reading that moves while the day's
/// content does not would make every reopen look stale.
fn day_instant(day: &str, minutes: i64) -> Option<String> {
    use chrono::{Duration, Local, NaiveDate, TimeZone};
    let date = NaiveDate::parse_from_str(day, "%Y-%m-%d").ok()?;
    let midnight = Local
        .from_local_datetime(&date.and_hms_opt(0, 0, 0)?)
        .single()?;
    Some(
        (midnight + Duration::minutes(minutes))
            .to_utc()
            .to_rfc3339(),
    )
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
    // The plan is read with `plan_for_day`, not the planner's own `get_plan`: that
    // one also scores the whole board to build suggestions, which is real work for
    // an answer nobody here reads.
    let plan: DayPlan = crate::plan::plan_for_day(pool, day, chrono::Local::now().date_naive())
        .await
        .context("day_evidence: plan_for_day")?;
    // The deterministic plan-adherence input: which planned tickets the day's
    // drafted/posted worklogs matched. Read here so the ledger is resolved once, in
    // this one reader, and every caller (daemon prose, tray screen) sees one shape.
    let matches = crate::day_task_worklogs::targets::matched_tickets_for_day(pool, day)
        .await
        .context("day_evidence: matched_tickets_for_day")?;
    // A confirmed-but-empty or skipped/reopened plan is a day with no plan, so it
    // scores against no tickets. Emptying the items here means the ledger, the
    // scalars, and the stored plan all branch on one decision.
    let planned = plan.is_planned();
    let plan_items: Vec<PlanItem> = if planned {
        plan.items.clone()
    } else {
        Vec::new()
    };

    // ── workstreams + segments ────────────────────────────────────────────────
    let mut workstreams: Vec<Value> = Vec::new();
    let mut segments: Vec<Value> = Vec::new();
    let mut workstream_logs: Vec<WorkstreamLog> = Vec::new();
    let mut last_min_of_day: i64 = 0;

    for t in &day_tasks.tasks {
        let mins: Vec<(i64, i64)> = t
            .segments
            .iter()
            .filter_map(|s| Some((hhmm_to_min(&s.start)?, hhmm_to_min(&s.end)?)))
            .filter(|(a, b)| b >= a)
            .collect();

        let first_min = mins.iter().map(|(a, _)| *a).min();
        let last_min = mins.iter().map(|(_, b)| *b).max();
        last_min_of_day = last_min_of_day.max(last_min.unwrap_or(0));

        for (a, b) in &mins {
            segments.push(json!({
                "task_id": t.id,
                "title": t.title,
                "start_min": a,
                "end_min": b,
                "minutes": b - a,
            }));
        }

        if t.minutes >= TASK_MIN_MINUTES {
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
        }

        // The 30-minute floor, applied to the PROSE side only. The model writes the
        // narrative and names the day's themes from these lines, and the screen
        // lists exactly the same set - so anything it is shown here it can point
        // at. The `segments` dataset above keeps every sitting, because that is
        // aggregate shape rather than a claim about what got done.
        if t.minutes >= TASK_MIN_MINUTES {
            workstream_logs.push(WorkstreamLog {
                task_id: t.id.clone(),
                title: t.title.clone(),
                minutes: t.minutes,
                lines: t.summary.clone(),
            });
        }
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

    // "Coding" exactly as the home page's stat card shows it: read back out of the
    // SAME merged rows the category chart renders rather than recomputed, which is
    // the move `OverviewPanel` makes for the same reason — two independently
    // derived numbers agree today and drift apart the next time either side
    // changes, and a summary that contradicts the screen behind it is worse than
    // no summary.
    let coding_s = datasets["categories"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|v| v["category"] == "coding")
        .and_then(|v| v["seconds"].as_i64())
        .unwrap_or(0);

    // The count worth saying out loud. A workstream under half an hour is a
    // detour, a glance, or a context switch that happened to get a title - real,
    // and visible on the timeline, but counting it as a thing you "did" inflates
    // the number until it means nothing. Better a true "you did 3 things" than a
    // flattering "you did 9" the reader can immediately see through.
    let task_count = day_tasks
        .tasks
        .iter()
        .filter(|t| t.minutes >= TASK_MIN_MINUTES)
        .count();

    let scalars = json!({
        // The two headline numbers, named and derived exactly as the home page's
        // FOCUS and CODING cards. The summary is allowed to quote them precisely
        // because they are the same values, not a second opinion.
        "focus_s": today.engaged_s,
        "coding_s": coding_s,
        // Substantial workstreams only — see TASK_MIN_MINUTES.
        "task_count": task_count,
        "task_min_minutes": TASK_MIN_MINUTES,
        // Everything below is context for the prose, not a headline.
        "workstream_count_including_brief": day_tasks.tasks.len(),
        "idle_s": today.idle_s,
        "agent_s": today.agent_s,
        "session_count": today.session_count,
        "switch_count": today.switch_count,
        // Whether the day had a plan at all. On the screen this is the branch
        // between an achievement ring and a picture of what the day turned out to
        // be about, so it belongs with the other facts the frontend reads directly.
        "planned": planned,
        "planned_count": plan_items.len(),
    });

    // Resolve the ledger from the effective plan (empty on a no-plan day → zeroed
    // adherence) and the day's measured tasks. No LLM: the ring and the checklist
    // are computed here, once.
    let ledger = adherence::resolve_deterministic(&plan_items, &day_tasks.tasks, &matches);

    // The stamp the staleness check compares against: the end of the last sitting
    // the day contains. An empty day gets an empty stamp rather than midnight,
    // which would read as "there was activity at 00:00".
    let evidence_at = if day_tasks.tasks.is_empty() {
        String::new()
    } else {
        day_instant(day, last_min_of_day).unwrap_or_default()
    };

    for (name, rows) in &datasets {
        tracing::debug!(dataset = %name, rows = rows.as_array().map(|a| a.len()).unwrap_or(0));
    }
    tracing::info!(
        day,
        workstreams = day_tasks.tasks.len(),
        substantial = workstream_logs.len(),
        segments = datasets["segments"]
            .as_array()
            .map(|a| a.len())
            .unwrap_or(0),
        apps = datasets["apps"].as_array().map(|a| a.len()).unwrap_or(0),
        hour_reports = hour_reports.len(),
        planned,
        plan_items = plan_items.len(),
        done = ledger.adherence.done,
        achievement_pct = ledger.adherence.achievement_pct,
        "day evidence collected"
    );

    Ok(Evidence {
        day: day.to_string(),
        datasets,
        scalars,
        workstream_logs,
        hour_reports,
        planned,
        // Emptied unless the plan was genuinely committed (see `plan_items` above):
        // a skipped day can still have leftover rows, and a reopened one keeps its
        // old ones - scoring either would invent a promise never made.
        plan: plan_items,
        ledger,
        tasks: day_tasks.tasks,
        evidence_at,
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
