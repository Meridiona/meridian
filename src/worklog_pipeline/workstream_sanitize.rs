//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Apply one hour's PLACEMENTS onto the day's prior tasks — the append step of the
//! anchored incremental fold. The model no longer returns the whole task set; it
//! returns only where this hour's work goes, and this stage folds that into the
//! running state without ever letting the model touch earlier hours.
//!
//! The working set **starts from `prior`**, so every existing task passes through
//! verbatim unless a placement names it — a bad or empty answer can only fail to
//! add, never reshuffle, rename, or drop earlier tasks. For each placement:
//!
//! * an existing `id` → **append** this hour's segments, **replace** the task's
//!   summary with the model's freshly-rewritten whole-story bullets (the summary is
//!   a rewrite, not a per-hour delta — see `workstream.md`), and adopt the
//!   placement's title only if it is non-empty (a rename),
//! * an empty/unknown `id` → **open a new task** with the next free `T<n>`,
//! * the summary is clamped to 6 bullets ([`MAX_SUMMARY_BULLETS`]) as a floor under
//!   the prompt's own cap, and an **empty** summary keeps the task's prior story
//!   (a placement that adds only time never blanks the narrative),
//! * each task's segments are coalesced ([`segment::normalize`]) so a carried-forward
//!   span and this hour's span for the same task draw as one bar,
//! * a task left with no segments (a rename that targeted nothing) is dropped.
//!
//! Time stays **code-owned** and the merge is retry-safe: re-applying the same hour
//! re-adds already-present segments (which coalesce to a no-op) and re-writes the
//! same bounded story.
//!
//! # Who calls this
//! [`crate::worklog_pipeline::workstream::run`].

use std::collections::BTreeSet;

use super::segment;
use super::task_db::DayTaskRow;
use super::workstream_parse::ParsedTask;
use super::workstream_state::summary_lines;

/// Hard cap on a task's summary — the story is a whole rewrite, never a growing
/// log. Backs up the prompt's own "never more than 6 bullets" instruction and the
/// schema's `maxItems`, so a model that ignores both still can't balloon the card.
const MAX_SUMMARY_BULLETS: usize = 6;

/// A task in the working set — a stable id, its narrative, and its full accumulated
/// segments, ready for persistence.
#[derive(Debug, Clone)]
pub struct SaneTask {
    pub id: String,
    pub title: String,
    pub summary: Vec<String>,
    pub segments: Vec<segment::Segment>,
}

/// Fold this hour's `placements` onto the day's `prior` tasks, returning the updated
/// working set (prior tasks first, in their existing order, then any new tasks).
pub fn apply_placements(placements: Vec<ParsedTask>, prior: &[DayTaskRow]) -> Vec<SaneTask> {
    // Start from prior — untouched tasks survive verbatim.
    let mut tasks: Vec<SaneTask> = prior
        .iter()
        .map(|p| SaneTask {
            id: p.task_id.clone(),
            title: p.title.clone(),
            summary: summary_lines(&p.summary),
            segments: p.segments.clone(),
        })
        .collect();

    let mut used: BTreeSet<i64> = tasks.iter().filter_map(|t| parse_tid(&t.id)).collect();

    for p in placements {
        // Resolve the target: an existing id appends; anything else opens a new task.
        let idx = match parse_tid(&p.id)
            .filter(|_| !p.id.is_empty())
            .and_then(|_| tasks.iter().position(|t| t.id == p.id))
        {
            Some(i) => i,
            None => {
                // New task — it needs an identity (a title) and time to show.
                let title = if !p.title.is_empty() {
                    p.title.clone()
                } else if let Some(first) = p.summary.first() {
                    first.clone()
                } else {
                    // No title, no story, no way to name it — only worth a slot if it
                    // carries time; give it a neutral label rather than dropping time.
                    "Untitled work".to_string()
                };
                let id = next_free_id(&mut used);
                tasks.push(SaneTask {
                    id,
                    title,
                    summary: Vec::new(),
                    segments: Vec::new(),
                });
                tasks.len() - 1
            }
        };

        let t = &mut tasks[idx];
        t.segments.extend(p.segments);
        // Summary is a whole-story REWRITE, not a delta: replace it with the model's
        // fresh bullets (clamped to the cap). An empty summary means the model chose
        // not to restate the story this hour — keep the prior one rather than blank
        // the task.
        if !p.summary.is_empty() {
            let mut story = p.summary;
            story.truncate(MAX_SUMMARY_BULLETS);
            t.summary = story;
        }
        // Adopt a matured/new title; an empty title keeps the existing one.
        if !p.title.is_empty() {
            t.title = p.title;
        }
    }

    // Coalesce each task's segments, then drop any task that ended up with no time.
    for t in tasks.iter_mut() {
        t.segments = segment::normalize(std::mem::take(&mut t.segments));
    }
    tasks.retain(|t| !t.segments.is_empty());
    tasks
}

/// The lowest unused `T<n>` id, marking it used.
fn next_free_id(used: &mut BTreeSet<i64>) -> String {
    let mut n = 1i64;
    while used.contains(&n) {
        n += 1;
    }
    used.insert(n);
    format!("T{n}")
}

/// Parse a `T<n>` id to its number, or `None` if it isn't that shape.
fn parse_tid(id: &str) -> Option<i64> {
    id.strip_prefix('T').and_then(|n| n.parse::<i64>().ok())
}

#[cfg(test)]
mod tests {
    use super::segment::Segment;
    use super::*;

    fn placement(id: &str, title: &str, summary: &[&str], segs: &[(i64, i64)]) -> ParsedTask {
        ParsedTask {
            id: id.into(),
            title: title.into(),
            summary: summary.iter().map(|s| s.to_string()).collect(),
            segments: segs
                .iter()
                .map(|&(a, b)| Segment {
                    start_min: a,
                    end_min: b,
                })
                .collect(),
        }
    }

    fn prior_row(id: &str, title: &str, summary: &str, segs: &[(i64, i64)]) -> DayTaskRow {
        DayTaskRow {
            task_id: id.into(),
            title: title.into(),
            summary: summary.into(),
            hours: vec![],
            segments: segs
                .iter()
                .map(|&(a, b)| Segment {
                    start_min: a,
                    end_min: b,
                })
                .collect(),
            minutes: 0,
            status: "active".into(),
            linked_ticket: None,
            created_at: "t0".into(),
        }
    }

    #[test]
    fn rewrites_an_existing_task_summary() {
        // Prior T1 worked 08:15-08:45 with a one-line summary; this hour adds
        // 09:00-09:30 and the model returns the WHOLE rewritten story, which
        // REPLACES the old summary (not appends). Time still accrues.
        let out = apply_placements(
            vec![placement(
                "T1",
                "",
                &[
                    "Set out to rebuild the website",
                    "Reworked the SEO and signup page",
                ],
                &[(540, 570)],
            )],
            &[prior_row("T1", "Website", "Reworked SEO", &[(495, 525)])],
        );
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].title, "Website", "empty title keeps existing");
        assert_eq!(out[0].segments.len(), 2);
        assert_eq!(
            out[0].summary,
            vec![
                "Set out to rebuild the website",
                "Reworked the SEO and signup page"
            ],
            "the rewritten story replaces the prior summary"
        );
    }

    #[test]
    fn untouched_prior_task_passes_through_verbatim() {
        // Placement touches only T1; T2 must survive unchanged (no data loss).
        let out = apply_placements(
            vec![placement("T1", "", &[], &[(540, 570)])],
            &[
                prior_row("T1", "A", "did a", &[(495, 525)]),
                prior_row("T2", "B", "did b", &[(600, 630)]),
            ],
        );
        assert_eq!(out.len(), 2);
        let t2 = out.iter().find(|t| t.id == "T2").unwrap();
        assert_eq!(t2.title, "B");
        assert_eq!(t2.summary, vec!["did b"]);
        assert_eq!(
            t2.segments,
            vec![Segment {
                start_min: 600,
                end_min: 630
            }]
        );
    }

    #[test]
    fn opens_a_new_task_for_unrelated_work() {
        let out = apply_placements(
            vec![placement(
                "",
                "Desktop sign-in",
                &["Designed the flow"],
                &[(720, 750)],
            )],
            &[prior_row("T1", "A", "did a", &[(495, 525)])],
        );
        assert_eq!(out.len(), 2);
        assert_eq!(out[1].id, "T2");
        assert_eq!(out[1].title, "Desktop sign-in");
        assert_eq!(out[1].summary, vec!["Designed the flow"]);
    }

    #[test]
    fn renames_an_existing_task_when_title_given() {
        let out = apply_placements(
            vec![placement(
                "T1",
                "Faster AI on the timeline",
                &[],
                &[(540, 570)],
            )],
            &[prior_row("T1", "AI stuff", "did a", &[(495, 525)])],
        );
        assert_eq!(out[0].title, "Faster AI on the timeline");
    }

    #[test]
    fn coalesces_prior_and_new_touching_segments() {
        // prior 08:15-08:45 + this hour 08:40-09:10 → one span 08:15-09:10.
        let out = apply_placements(
            vec![placement("T1", "", &[], &[(520, 550)])],
            &[prior_row("T1", "A", "", &[(495, 525)])],
        );
        assert_eq!(
            out[0].segments,
            vec![Segment {
                start_min: 495,
                end_min: 550
            }]
        );
    }

    #[test]
    fn empty_summary_keeps_prior_story() {
        // A placement that adds only time (empty summary — the model chose not to
        // restate the story this hour) must not blank the task's existing story.
        let out = apply_placements(
            vec![placement("T1", "", &[], &[(540, 570)])],
            &[prior_row("T1", "A", "did a\ndid b", &[(495, 525)])],
        );
        assert_eq!(out[0].summary, vec!["did a", "did b"]);
    }

    #[test]
    fn clamps_summary_to_six_bullets() {
        // A model that ignores the "never more than 6" rule is still capped.
        let out = apply_placements(
            vec![placement(
                "T1",
                "",
                &["1", "2", "3", "4", "5", "6", "7", "8"],
                &[(540, 570)],
            )],
            &[prior_row("T1", "A", "old", &[(495, 525)])],
        );
        assert_eq!(out[0].summary, vec!["1", "2", "3", "4", "5", "6"]);
    }

    #[test]
    fn reapplying_the_same_hour_keeps_one_story_and_coalesced_time() {
        // Replace semantics stay idempotent: re-folding the same hour re-writes the
        // same bounded story and re-coalesces the same time.
        let prior = &[prior_row("T1", "A", "did a", &[(495, 525)])];
        let p = || {
            vec![placement(
                "T1",
                "",
                &["Told the whole story", "up to now"],
                &[(540, 570)],
            )]
        };
        let once = apply_placements(p(), prior);
        assert_eq!(once[0].summary, vec!["Told the whole story", "up to now"]);
        // Feed the result back as prior and apply the same placement again.
        let prior2: Vec<DayTaskRow> = once
            .iter()
            .map(|t| prior_row(&t.id, &t.title, &t.summary.join("\n"), &segs_of(t)))
            .collect();
        let twice = apply_placements(p(), &prior2);
        assert_eq!(twice.len(), 1);
        assert_eq!(twice[0].summary, once[0].summary);
        assert_eq!(twice[0].segments, once[0].segments);
    }

    #[test]
    fn empty_placements_keep_the_day() {
        let out = apply_placements(vec![], &[prior_row("T1", "A", "did a", &[(495, 525)])]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].id, "T1");
    }

    fn segs_of(t: &SaneTask) -> Vec<(i64, i64)> {
        t.segments
            .iter()
            .map(|s| (s.start_min, s.end_min))
            .collect()
    }
}
