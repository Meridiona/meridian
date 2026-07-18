//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! In-memory day-task state for the fold replays — snapshot serde + rendering.
//!
//! The fold experiments never write `day_tasks`; instead the same pure fold
//! pipeline the daemon runs (`parse_placements` → `apply_placements` → `to_rows`)
//! is applied to an in-memory [`DayTaskRow`] working set, and the result is
//! rendered as the **exact `DayTasksResponse` JSON the dashboard's
//! `get_day_tasks` serves** — so the LLM Lab UI can feed it straight into the
//! real `DayTaskColumn` timeline and show "what the day would look like under
//! this model".
//!
//! # Who calls this
//! [`super::runner`] (hour-fold rendering + the day-fold chain) and
//! [`super::request`] (snapshotting the hour-fold's prior state into `render_ctx`).
//!
//! # Related
//! - [`meridian_core::day_tasks`] — the reader whose response shape
//!   [`day_tasks_json`] mirrors byte-for-byte (keep them in sync).
//! - [`crate::worklog_pipeline::workstream_sanitize`] /
//!   [`crate::worklog_pipeline::workstream_state`] — the reused fold pipeline.

use serde_json::{json, Value};

use crate::worklog_pipeline::{
    segment::{self, Segment},
    task_db::DayTaskRow,
    workstream_parse::parse_placements,
    workstream_sanitize::apply_placements,
    workstream_state::{summary_lines, to_rows},
};

/// Serialise a working set for a `render_ctx` / `input_json` snapshot. Segments
/// ride as `[start_min, end_min]` integer pairs — lossless, unlike the `HH:MM`
/// display form.
pub fn rows_to_json(rows: &[DayTaskRow]) -> Value {
    Value::Array(
        rows.iter()
            .map(|r| {
                json!({
                    "task_id": r.task_id,
                    "title": r.title,
                    "summary": r.summary,
                    "hours": r.hours,
                    "segs": r.segments.iter().map(|s| json!([s.start_min, s.end_min])).collect::<Vec<_>>(),
                    "minutes": r.minutes,
                    "status": r.status,
                    "linked_ticket": r.linked_ticket,
                    "created_at": r.created_at,
                })
            })
            .collect(),
    )
}

/// Rebuild a working set from a [`rows_to_json`] snapshot. Tolerant: a missing /
/// malformed field degrades to its default rather than failing the replay.
pub fn rows_from_json(v: &Value) -> Vec<DayTaskRow> {
    let Some(arr) = v.as_array() else {
        return Vec::new();
    };
    arr.iter()
        .map(|t| {
            let s = |k: &str| t.get(k).and_then(Value::as_str).unwrap_or("").to_string();
            let segments: Vec<Segment> = t
                .get("segs")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|p| {
                            let pair = p.as_array()?;
                            Some(Segment {
                                start_min: pair.first()?.as_i64()?,
                                end_min: pair.get(1)?.as_i64()?,
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();
            DayTaskRow {
                task_id: s("task_id"),
                title: s("title"),
                summary: s("summary"),
                hours: t
                    .get("hours")
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(|h| h.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default(),
                segments,
                minutes: t.get("minutes").and_then(Value::as_i64).unwrap_or(0),
                status: {
                    let st = s("status");
                    if st.is_empty() {
                        "active".to_string()
                    } else {
                        st
                    }
                },
                linked_ticket: t
                    .get("linked_ticket")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                created_at: s("created_at"),
            }
        })
        .collect()
}

/// Fold one model answer onto `state` — the same parse → sanitize → derive-time
/// pipeline [`crate::worklog_pipeline::workstream::run`] applies, minus
/// persistence. An unparseable or empty answer preserves the state untouched
/// (the fold's own safety rule).
pub fn fold_answer(
    state: &[DayTaskRow],
    answer: &str,
    day_local: &str,
    now: &str,
) -> Vec<DayTaskRow> {
    let placements = parse_placements(answer).unwrap_or_default();
    if placements.is_empty() {
        return state.to_vec();
    }
    let sanitized = apply_placements(placements, state);
    to_rows(&sanitized, day_local, state, now)
}

/// Local hour-of-day from a `"YYYY-MM-DDTHH"` label (mirrors the reader's private
/// `label_hour`).
fn label_hour(label: &str) -> Option<i64> {
    label.get(11..13)?.parse::<i64>().ok()
}

/// Render a working set as the dashboard's `DayTasksResponse` JSON — the exact
/// shape `meridian_core::day_tasks::get_day_tasks` serves, so the Lab UI feeds it
/// straight into the real `DayTaskColumn`.
pub fn day_tasks_json(day: &str, rows: &[DayTaskRow]) -> Value {
    let tasks: Vec<Value> = rows
        .iter()
        .map(|r| {
            let mut hours = r.hours.clone();
            hours.sort();
            let hour_nums: Vec<i64> = hours.iter().filter_map(|h| label_hour(h)).collect();
            json!({
                "id": r.task_id,
                "title": r.title,
                "summary": summary_lines(&r.summary),
                "minutes": r.minutes,
                "hours": hours,
                "segments": segment::to_value(&r.segments),
                "first_hour": hour_nums.iter().min().copied().unwrap_or(-1),
                "last_hour": hour_nums.iter().max().copied().unwrap_or(-1),
                "status": r.status,
                "linked_ticket": r.linked_ticket,
            })
        })
        .collect();
    json!({ "day": day, "tasks": tasks })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row() -> DayTaskRow {
        DayTaskRow {
            task_id: "T1".into(),
            title: "A task".into(),
            summary: "did a\ndid b".into(),
            hours: vec!["2026-07-16T09".into(), "2026-07-16T08".into()],
            segments: vec![Segment {
                start_min: 495,
                end_min: 585,
            }],
            minutes: 90,
            status: "active".into(),
            linked_ticket: Some("KAN-1".into()),
            created_at: "t0".into(),
        }
    }

    #[test]
    fn snapshot_round_trips_losslessly() {
        let rows = vec![row()];
        let back = rows_from_json(&rows_to_json(&rows));
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].task_id, "T1");
        assert_eq!(back[0].summary, "did a\ndid b");
        assert_eq!(back[0].segments[0].start_min, 495);
        assert_eq!(back[0].linked_ticket.as_deref(), Some("KAN-1"));
        assert_eq!(back[0].created_at, "t0");
    }

    #[test]
    fn day_tasks_json_matches_the_reader_shape() {
        let v = day_tasks_json("2026-07-16", &[row()]);
        assert_eq!(v["day"], "2026-07-16");
        let t = &v["tasks"][0];
        assert_eq!(t["id"], "T1");
        assert_eq!(t["summary"], json!(["did a", "did b"]));
        // hours sorted ascending; first/last derived from labels.
        assert_eq!(t["hours"][0], "2026-07-16T08");
        assert_eq!(t["first_hour"], 8);
        assert_eq!(t["last_hour"], 9);
        // Segments in the HH:MM display form the timeline parses.
        assert_eq!(t["segments"][0]["start"], "08:15");
        assert_eq!(t["segments"][0]["end"], "09:45");
    }

    #[test]
    fn fold_answer_preserves_state_on_garbage_and_folds_on_placements() {
        let state = vec![row()];
        // Garbage answer: state unchanged.
        let kept = fold_answer(&state, "no json here", "2026-07-16", "now");
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].title, "A task");

        // A real placement extends the day (exact fold semantics are the
        // sanitizer's own tested domain - here we just prove the wiring).
        let answer = r#"{"placements":[{"id":"T2","title":"New work",
            "summary":["started it"],"segments":[{"start":"10:00","end":"10:30"}]}]}"#;
        let folded = fold_answer(&state, answer, "2026-07-16", "now");
        assert!(folded.iter().any(|t| t.task_id == "T2"), "{folded:?}");
    }
}
