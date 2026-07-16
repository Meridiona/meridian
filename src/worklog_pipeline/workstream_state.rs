//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! The two edges of the fold that touch stored rows: turning the day's prior tasks
//! into the model's input state, and turning the sanitised result back into
//! persistable [`DayTaskRow`]s.
//!
//! Time is **code-owned**: a task's `minutes` and `hours` are derived from its
//! segments here ([`segment::union_minutes`] / [`segment::hours_touched`]), never
//! read from the model. `created_at` / `status` / `linked_ticket` are carried
//! forward from the prior row of the same id.
//!
//! # Who calls this
//! [`crate::worklog_pipeline::workstream::run`].

use serde_json::{json, Value};

use super::segment;
use super::task_db::DayTaskRow;
use super::workstream_sanitize::SaneTask;

/// Serialise the current tasks as the `{"tasks":[…]}` state the model reads back
/// each hour (with their segments, so it can extend rather than restate them). The
/// stored newline-joined `summary` is split back into the line array.
pub fn build_state_json(prior: &[DayTaskRow]) -> String {
    let tasks: Vec<Value> = prior
        .iter()
        .map(|t| {
            json!({
                "id": t.task_id,
                "title": t.title,
                "summary": summary_lines(&t.summary),
                "segments": segment::to_value(&t.segments),
            })
        })
        .collect();
    json!({ "tasks": tasks }).to_string()
}

/// Split a stored `summary` blob back into its lines (drops blanks).
pub fn summary_lines(summary: &str) -> Vec<String> {
    summary
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

/// Build the persistable rows: join summary lines, derive `minutes` / `hours` from
/// each task's segments, and carry `created_at` / `status` / `linked_ticket`
/// forward from the prior row of the same id (a new task is stamped `now`,
/// status `active`, ticket `None`).
pub fn to_rows(
    tasks: &[SaneTask],
    day_local: &str,
    prior: &[DayTaskRow],
    now: &str,
) -> Vec<DayTaskRow> {
    tasks
        .iter()
        .map(|t| {
            let prev = prior.iter().find(|p| p.task_id == t.id);
            DayTaskRow {
                task_id: t.id.clone(),
                title: t.title.clone(),
                summary: t.summary.join("\n"),
                hours: segment::hours_touched(&t.segments, day_local),
                minutes: segment::union_minutes(&t.segments),
                segments: t.segments.clone(),
                status: prev
                    .map(|p| p.status.clone())
                    .unwrap_or_else(|| "active".to_string()),
                linked_ticket: prev.and_then(|p| p.linked_ticket.clone()),
                created_at: prev
                    .map(|p| p.created_at.clone())
                    .unwrap_or_else(|| now.to_string()),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::super::segment::Segment;
    use super::*;

    fn sane(id: &str, segs: &[(i64, i64)]) -> SaneTask {
        SaneTask {
            id: id.into(),
            title: format!("Title {id}"),
            summary: vec!["l1".into(), "l2".into()],
            segments: segs
                .iter()
                .map(|&(a, b)| Segment {
                    start_min: a,
                    end_min: b,
                })
                .collect(),
        }
    }

    #[test]
    fn to_rows_derives_minutes_and_hours_from_segments() {
        // 08:15-08:45 (30) + 11:43-12:15 (32) → 62 min, hours [08,11,12].
        let rows = to_rows(
            &[sane("T1", &[(495, 525), (703, 735)])],
            "2026-07-15",
            &[],
            "now",
        );
        assert_eq!(rows[0].minutes, 62);
        assert_eq!(
            rows[0].hours,
            vec!["2026-07-15T08", "2026-07-15T11", "2026-07-15T12"]
        );
        assert_eq!(rows[0].summary, "l1\nl2");
        assert_eq!(rows[0].created_at, "now");
    }

    #[test]
    fn to_rows_preserves_created_at_and_ticket_by_id() {
        let prior = vec![DayTaskRow {
            task_id: "T1".into(),
            title: "old".into(),
            summary: String::new(),
            hours: vec![],
            segments: vec![],
            minutes: 0,
            status: "active".into(),
            linked_ticket: Some("KAN-1".into()),
            created_at: "orig".into(),
        }];
        let rows = to_rows(&[sane("T1", &[(495, 525)])], "2026-07-15", &prior, "now");
        assert_eq!(rows[0].created_at, "orig");
        assert_eq!(rows[0].linked_ticket.as_deref(), Some("KAN-1"));
    }

    #[test]
    fn state_json_carries_title_summary_and_segments_as_anchors() {
        let prior = vec![DayTaskRow {
            task_id: "T1".into(),
            title: "Website".into(),
            summary: "did a\ndid b".into(),
            hours: vec![],
            segments: vec![Segment {
                start_min: 495,
                end_min: 525,
            }],
            minutes: 30,
            status: "active".into(),
            linked_ticket: None,
            created_at: "t0".into(),
        }];
        // The anchor state the model matches against carries the id, title, the full
        // summary split into lines, and the segments (times) back to the model.
        let s = build_state_json(&prior);
        assert!(s.contains("\"Website\""));
        assert!(s.contains("\"did a\""));
        assert!(s.contains("\"did b\""));
        assert!(s.contains("\"08:15\""));
    }
}
