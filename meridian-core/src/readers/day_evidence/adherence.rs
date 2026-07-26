//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Planned vs actual: which committed tickets were worked on, and the arithmetic
//! on top. **Fully deterministic - no model involved.**
//!
//! # Why this exists, and why it is no longer the model's job
//! Nothing in the schema linked a planned ticket to the work done for it, so this
//! used to be a two-layer join: DB facts where they existed, and the LLM's verdict
//! for the rest. But the worklog matcher ALREADY makes that join. When a worklog is
//! drafted for a day-task, the ticket(s) it advances are written to
//! `day_task_worklog_targets` (migration 062) - the moment it is drafted, before any
//! approval. So "which planned ticket did this day-task's work go to" is a fact on
//! disk, and asking the model to re-derive it was redundant and occasionally wrong.
//!
//! [`resolve_deterministic`] reads that match map (built by
//! [`crate::day_task_worklogs::targets::matched_tickets_for_day`]) and settles every
//! planned ticket without an LLM call:
//!
//! - a worklog was matched to it (drafted or posted) → **done**, crediting the
//!   matched day-tasks' measured minutes;
//! - else the ticket is terminal (it left the board, almost always by being closed)
//!   → **done**, no attributable time;
//! - else → **not started**.
//!
//! Binary on purpose: touching the task (a worklog matched it) is enough to count it
//! done, and there is no reliable deterministic signal for a "half done" middle
//! state. [`Outcome::Partial`] stays in the type for wire back-compat but is never
//! emitted here.
//!
//! The number the screen shows comes from here, in Rust, from one array - so the
//! ring and the list beside it cannot disagree, and it holds even when the model
//! call for the prose fails.
//!
//! # Who calls this
//! [`super::collect`], which puts the resolved [`DayLedger`] on the evidence so both
//! the daemon (the summary prose) and the tray (the screen) read one shape.
//!
//! # Related
//! - [`crate::plan`] — where a [`crate::plan::PlanItem`] comes from.
//! - [`crate::day_tasks`] — the observed side.
//! - [`crate::day_task_worklogs::targets`] — the match map's source.
//! - [`crate::day_summaries`] — the wire types this produces.

use std::collections::{HashMap, HashSet};

use crate::day_summaries::{Adherence, Outcome, PlanVerdict};
use crate::day_tasks::DayTask;
use crate::plan::PlanItem;

use super::TASK_MIN_MINUTES;

/// The resolved plan ledger and its arithmetic, kept together so a caller cannot
/// persist one without the other.
#[derive(Debug, Clone)]
pub struct DayLedger {
    pub verdicts: Vec<PlanVerdict>,
    pub adherence: Adherence,
}

/// Settle every planned ticket from the worklog match map, and score it.
///
/// `matches` is `task_key -> [day-task id, …]`: the day-tasks whose drafted or
/// posted worklog was matched to that ticket. Every planned ticket produces exactly
/// one verdict, in plan order, so the ledger is always the same length as the plan.
///
/// `tasks` supplies the measured minutes for the matched day-tasks (and the
/// unplanned tail). A match naming a day-task id that is not in `tasks` - a stale
/// target from a workstream that has since been re-segmented away - claims nothing.
pub fn resolve_deterministic(
    plan: &[PlanItem],
    tasks: &[DayTask],
    matches: &HashMap<String, Vec<String>>,
) -> DayLedger {
    let by_id: HashMap<&str, &DayTask> = tasks.iter().map(|t| (t.id.as_str(), t)).collect();

    let mut verdicts: Vec<PlanVerdict> = Vec::new();
    // Day-tasks already credited to a planned ticket, so they don't also count as
    // unplanned work below.
    let mut accounted: HashSet<String> = HashSet::new();

    for item in plan {
        // The matched day-tasks that actually exist in the day.
        let matched: Vec<&DayTask> = matches
            .get(item.task_key.as_str())
            .map(|ids| {
                ids.iter()
                    .filter_map(|id| by_id.get(id.as_str()).copied())
                    .collect()
            })
            .unwrap_or_default();

        // Base the verdict on the item; only the outcome-specific fields differ.
        let mut verdict = PlanVerdict {
            task_key: item.task_key.clone(),
            title: item.title.clone(),
            outcome: Outcome::NotTouched,
            evidence: String::new(),
            minutes: 0,
            day_task_ids: Vec::new(),
            // Every verdict here is a database fact, never a guess.
            certain: true,
            provider: item.provider.clone(),
            url: item.url.clone(),
        };

        if !matched.is_empty() {
            for t in &matched {
                accounted.insert(t.id.clone());
            }
            verdict.outcome = Outcome::Done;
            verdict.evidence = "you worked on it".to_string();
            verdict.minutes = matched.iter().map(|t| t.minutes).sum();
            verdict.day_task_ids = matched.iter().map(|t| t.id.clone()).collect();
        } else if item.is_terminal {
            // Closed today with no workstream tied to it - finished, no time to show.
            verdict.outcome = Outcome::Done;
            verdict.evidence = "the ticket was closed".to_string();
        }

        verdicts.push(verdict);
    }

    let planned = verdicts.len() as i64;
    let done = verdicts
        .iter()
        .filter(|v| v.outcome == Outcome::Done)
        .count() as i64;
    // No middle state is emitted, so partial is always zero and not_touched is the
    // complement of done. Both are still derived rather than assumed, so the type
    // can carry a partial verdict from an older row without the count going wrong.
    let partial = verdicts
        .iter()
        .filter(|v| v.outcome == Outcome::Partial)
        .count() as i64;
    let not_touched = planned - done - partial;

    // Half-credit arithmetic in integers, rounded half-up (done = 2, partial = 1,
    // out of 2 per planned item). With a binary ledger this is just done/planned,
    // but the general form keeps a stray partial honest.
    let achievement_pct = if planned == 0 {
        0
    } else {
        let earned: i64 = verdicts.iter().map(|v| v.outcome.half_credits()).sum();
        (earned * 100 + planned) / (planned * 2)
    };

    // The unplanned tail: substantial workstreams no planned ticket accounts for.
    // Counting a five-minute glance as "unplanned work" would make every day look
    // scattered, so the same floor the headline count uses applies here.
    let unplanned_minutes = tasks
        .iter()
        .filter(|t| t.minutes >= TASK_MIN_MINUTES && !accounted.contains(&t.id))
        .map(|t| t.minutes)
        .sum();

    DayLedger {
        adherence: Adherence {
            planned,
            done,
            partial,
            not_touched,
            achievement_pct,
            unplanned_minutes,
        },
        verdicts,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::day_tasks::DayTask;

    fn item(key: &str, terminal: bool) -> PlanItem {
        PlanItem {
            task_key: key.to_string(),
            position: 0,
            origin: "manual".to_string(),
            title: format!("{key} title"),
            provider: "jira".to_string(),
            url: String::new(),
            status: String::new(),
            is_terminal: terminal,
            due_date: None,
            due_days: None,
            description: String::new(),
            epic: None,
            priority: None,
            issue_type: "Task".to_string(),
            story_points: None,
        }
    }

    fn task(id: &str, minutes: i64) -> DayTask {
        DayTask {
            id: id.to_string(),
            title: format!("{id} work"),
            summary: Vec::new(),
            minutes,
            hours: Vec::new(),
            segments: Vec::new(),
            first_hour: -1,
            last_hour: -1,
            status: String::new(),
            linked_ticket: None,
            posted_provider: None,
            posted_target_key: None,
            posted_browse_url: None,
        }
    }

    /// task_key → [day-task ids], the shape `matched_tickets_for_day` returns.
    fn matches(pairs: &[(&str, &[&str])]) -> HashMap<String, Vec<String>> {
        pairs
            .iter()
            .map(|(k, ids)| (k.to_string(), ids.iter().map(|s| s.to_string()).collect()))
            .collect()
    }

    #[test]
    fn a_matched_ticket_is_done_and_claims_its_minutes() {
        let plan = [item("KAN-1", false)];
        let tasks = [task("T1", 130)];
        let led = resolve_deterministic(&plan, &tasks, &matches(&[("KAN-1", &["T1"])]));
        assert_eq!(led.verdicts[0].outcome, Outcome::Done);
        assert_eq!(led.verdicts[0].minutes, 130);
        assert_eq!(led.verdicts[0].day_task_ids, vec!["T1"]);
        assert!(led.verdicts[0].certain);
        assert_eq!(led.adherence.done, 1);
        assert_eq!(led.adherence.achievement_pct, 100);
    }

    #[test]
    fn a_closed_ticket_with_no_matched_work_is_still_done() {
        let plan = [item("KAN-9", true)];
        let led = resolve_deterministic(&plan, &[], &matches(&[]));
        assert_eq!(led.verdicts[0].outcome, Outcome::Done);
        assert_eq!(led.verdicts[0].minutes, 0);
        assert_eq!(led.verdicts[0].evidence, "the ticket was closed");
    }

    #[test]
    fn an_unmatched_open_ticket_is_not_touched() {
        let plan = [item("KAN-2", false)];
        let led = resolve_deterministic(&plan, &[task("T1", 60)], &matches(&[]));
        assert_eq!(led.verdicts[0].outcome, Outcome::NotTouched);
        assert!(led.verdicts[0].day_task_ids.is_empty());
        assert!(led.verdicts[0].evidence.is_empty());
    }

    #[test]
    fn the_ledger_is_always_the_length_of_the_plan() {
        let plan = [
            item("KAN-1", false),
            item("KAN-2", false),
            item("KAN-3", false),
        ];
        let tasks = [task("T1", 60)];
        let led = resolve_deterministic(&plan, &tasks, &matches(&[("KAN-1", &["T1"])]));
        assert_eq!(led.verdicts.len(), 3);
        assert_eq!(led.adherence.planned, 3);
        assert_eq!(led.adherence.done, 1);
        assert_eq!(led.adherence.not_touched, 2);
    }

    /// Four of six done rounds to 67, not 66 - half-up on the integer arithmetic.
    #[test]
    fn the_percentage_rounds_half_up() {
        let plan: Vec<PlanItem> = (1..=6).map(|n| item(&format!("KAN-{n}"), false)).collect();
        let tasks: Vec<DayTask> = (1..=4).map(|n| task(&format!("T{n}"), 30)).collect();
        let m = matches(&[
            ("KAN-1", &["T1"]),
            ("KAN-2", &["T2"]),
            ("KAN-3", &["T3"]),
            ("KAN-4", &["T4"]),
        ]);
        let led = resolve_deterministic(&plan, &tasks, &m);
        assert_eq!(led.adherence.done, 4);
        assert_eq!(led.adherence.achievement_pct, 67);
    }

    /// One workstream matched to two tickets credits both without being double-
    /// subtracted from the unplanned tail.
    #[test]
    fn one_workstream_may_back_two_tickets() {
        let plan = [item("KAN-1", false), item("KAN-2", false)];
        let tasks = [task("T1", 90)];
        let m = matches(&[("KAN-1", &["T1"]), ("KAN-2", &["T1"])]);
        let led = resolve_deterministic(&plan, &tasks, &m);
        assert_eq!(led.adherence.done, 2);
        assert_eq!(led.verdicts[0].minutes, 90);
        assert_eq!(led.verdicts[1].minutes, 90);
        // T1 is accounted, so it is not also unplanned.
        assert_eq!(led.adherence.unplanned_minutes, 0);
    }

    #[test]
    fn substantial_unmatched_work_is_the_unplanned_tail() {
        let plan = [item("KAN-1", false)];
        // T1 matched; T2 substantial but unplanned; T3 below the floor.
        let tasks = [task("T1", 60), task("T2", 45), task("T3", 10)];
        let led = resolve_deterministic(&plan, &tasks, &matches(&[("KAN-1", &["T1"])]));
        assert_eq!(led.adherence.unplanned_minutes, 45);
    }

    /// A match naming a day-task the day no longer has claims nothing and does not
    /// crash the fold.
    #[test]
    fn a_stale_match_id_is_ignored() {
        let plan = [item("KAN-1", false)];
        let led = resolve_deterministic(&plan, &[], &matches(&[("KAN-1", &["GONE"])]));
        // No real day-task, so no minutes and no match → not touched.
        assert_eq!(led.verdicts[0].outcome, Outcome::NotTouched);
        assert_eq!(led.verdicts[0].minutes, 0);
    }

    #[test]
    fn an_empty_plan_scores_zero_without_dividing_by_zero() {
        let led = resolve_deterministic(&[], &[task("T1", 60)], &matches(&[]));
        assert_eq!(led.adherence.planned, 0);
        assert_eq!(led.adherence.achievement_pct, 0);
        // Substantial work with no plan is entirely the unplanned tail.
        assert_eq!(led.adherence.unplanned_minutes, 60);
    }
}
