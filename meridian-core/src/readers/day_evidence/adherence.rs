//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Planned vs actual: what can be known for certain, and the arithmetic on top.
//!
//! # Why this exists
//! Nothing in the schema links a planned ticket to the work that was done for it.
//! There is no `plan_id` on `app_sessions` or `day_tasks`, and there never was — the
//! plan is intent and the workstreams are observation, and they are joined only by
//! the developer's own head.
//!
//! So the join is rebuilt here in two layers:
//!
//! 1. **This module.** Where the database DOES know (a worklog was posted against
//!    the ticket; the workstream carries the ticket as its linked ticket; the ticket
//!    went terminal), the outcome is a fact. [`prematch`] finds those, and they are
//!    **locked** — the model is shown them and may not overturn them.
//! 2. **The model**, for everything left over, reading the day's own log lines
//!    beside the plan. Its verdicts are folded in by [`resolve`], which never lets
//!    one land on a ticket layer 1 already settled.
//!
//! The number the screen shows then comes from [`resolve`], in Rust, from the folded
//! verdicts — never from the model's prose. That is the whole point of splitting it
//! this way: the ring and the list beside it are computed from one array, so they
//! cannot disagree, and regenerating a summary cannot silently move the score while
//! the evidence stays put.
//!
//! # Who calls this
//! [`super::collect`] (to put the locked matches in the evidence) and
//! `meridian::day_summary::generate` (to fold the answer and score it).
//!
//! # Related
//! - [`crate::plan`] — where a [`crate::plan::PlanItem`] comes from.
//! - [`crate::day_tasks`] — the observed side.
//! - [`crate::day_summaries`] — the wire types this produces.

use std::collections::{HashMap, HashSet};

use crate::day_summaries::{Adherence, Outcome, PlanVerdict};
use crate::day_tasks::DayTask;
use crate::plan::PlanItem;

use super::TASK_MIN_MINUTES;

/// How a planned ticket's outcome became known without asking the model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchEvidence {
    /// A worklog was posted against this exact ticket from this day's work. The
    /// strongest signal there is: the developer read a draft about this ticket and
    /// approved it onto the real board.
    WorklogPosted,
    /// The workstream carries this ticket as its linked ticket.
    LinkedTicket,
    /// No workstream could be tied to it, but the ticket is terminal — it left the
    /// board, which on the day it was planned almost always means it was finished.
    TicketTerminal,
}

impl MatchEvidence {
    /// The line the screen shows under the ticket. Written as a fact, in the same
    /// register as the model's own evidence lines, so the two read as one list.
    pub fn reason(self) -> &'static str {
        match self {
            MatchEvidence::WorklogPosted => "a worklog was posted against it",
            MatchEvidence::LinkedTicket => "the work was linked to it",
            MatchEvidence::TicketTerminal => "the ticket was closed",
        }
    }

    /// What a match of this kind proves. All three prove completion: a posted
    /// worklog and a closed ticket both mean the developer said so, and a linked
    /// ticket means the pipeline tied real work to it end to end.
    pub fn outcome(self) -> Outcome {
        Outcome::Done
    }
}

/// One planned ticket whose outcome the database already settles.
#[derive(Debug, Clone)]
pub struct PreMatch {
    pub task_key: String,
    /// Every workstream that backs it. Usually one; a ticket worked in two
    /// separate sittings that got split into two workstreams gives two.
    pub day_task_ids: Vec<String>,
    /// Their summed measured minutes. Zero for a [`MatchEvidence::TicketTerminal`]
    /// match, where the ticket closed but no workstream could be tied to it.
    pub minutes: i64,
    pub evidence: MatchEvidence,
}

/// The outcomes the database settles on its own, strongest evidence first.
///
/// One entry per planned ticket that matched; planned tickets with no evidence at
/// all are simply absent, and are the set the model is asked about.
pub fn prematch(plan: &[PlanItem], tasks: &[DayTask]) -> Vec<PreMatch> {
    let mut out = Vec::new();

    for item in plan {
        // Posted worklog beats linked ticket: the first means a human approved a
        // draft onto the board, the second only that the pipeline tied them.
        let posted: Vec<&DayTask> = tasks
            .iter()
            .filter(|t| t.posted_target_key.as_deref() == Some(item.task_key.as_str()))
            .collect();
        let linked: Vec<&DayTask> = tasks
            .iter()
            .filter(|t| t.linked_ticket.as_deref() == Some(item.task_key.as_str()))
            .collect();

        let (matched, evidence) = if !posted.is_empty() {
            (posted, MatchEvidence::WorklogPosted)
        } else if !linked.is_empty() {
            (linked, MatchEvidence::LinkedTicket)
        } else if item.is_terminal {
            (Vec::new(), MatchEvidence::TicketTerminal)
        } else {
            continue;
        };

        out.push(PreMatch {
            task_key: item.task_key.clone(),
            day_task_ids: matched.iter().map(|t| t.id.clone()).collect(),
            minutes: matched.iter().map(|t| t.minutes).sum(),
            evidence,
        });
    }

    out
}

/// What the model said about one planned ticket, before folding.
#[derive(Debug, Clone)]
pub struct ModelVerdict {
    pub task_key: String,
    pub outcome: Outcome,
    pub evidence: String,
    /// The workstreams it judged from. **Ids, never a duration** — the model
    /// points and the code measures, the same contract `themes` uses. Without
    /// this every non-certain row shows no time and its work counts as unplanned,
    /// which on a day with no posted worklogs is the whole day.
    pub day_task_ids: Vec<String>,
}

/// Fold the locked matches and the model's verdicts into the day's plan ledger,
/// then score it.
///
/// Order is the plan's own order, and **every** planned ticket produces a row: a
/// ticket the model forgot to mention is `not_touched` with no evidence line, which
/// is the honest reading and keeps the ledger the same length as the plan.
///
/// `tasks` is used only for the unplanned tail — the substantial workstreams no
/// planned ticket accounts for.
pub fn resolve(plan: &[PlanItem], tasks: &[DayTask], model: &[ModelVerdict]) -> DayLedger {
    let prematches = prematch(plan, tasks);
    let pre: HashMap<&str, &PreMatch> = prematches
        .iter()
        .map(|p| (p.task_key.as_str(), p))
        .collect();
    let said: HashMap<&str, &ModelVerdict> =
        model.iter().map(|m| (m.task_key.as_str(), m)).collect();

    // A model verdict names the workstreams it judged from; the minutes are read
    // off those, never taken from the model. `by_id` is the lookup that turns a
    // pointed-at id into a measured figure, and silently drops an id that was
    // never in the day.
    let by_id: HashMap<&str, &DayTask> = tasks.iter().map(|t| (t.id.as_str(), t)).collect();

    let mut verdicts: Vec<PlanVerdict> = Vec::new();
    let mut accounted: HashSet<String> = HashSet::new();

    for item in plan {
        let key = item.task_key.as_str();
        let v = match pre.get(key) {
            // Locked. The model's opinion on this one is not consulted.
            Some(p) => {
                accounted.extend(p.day_task_ids.iter().cloned());
                PlanVerdict {
                    task_key: item.task_key.clone(),
                    title: item.title.clone(),
                    outcome: p.evidence.outcome(),
                    evidence: p.evidence.reason().to_string(),
                    minutes: p.minutes,
                    day_task_ids: p.day_task_ids.clone(),
                    certain: true,
                    provider: item.provider.clone(),
                    url: item.url.clone(),
                }
            }
            None => {
                let Some(m) = said.get(key) else {
                    // A ticket the model never mentioned. Not touched, no
                    // evidence, no minutes — the honest reading, and it keeps the
                    // ledger exactly as long as the plan.
                    verdicts.push(PlanVerdict {
                        task_key: item.task_key.clone(),
                        title: item.title.clone(),
                        outcome: Outcome::NotTouched,
                        evidence: String::new(),
                        minutes: 0,
                        day_task_ids: Vec::new(),
                        certain: false,
                        provider: item.provider.clone(),
                        url: item.url.clone(),
                    });
                    continue;
                };

                // Only a ticket the model says was ACTUALLY worked on claims time.
                // Crediting a `not_touched` verdict with whatever workstreams it
                // happened to cite would both invent minutes and shrink the
                // unplanned tail by work nobody did against that ticket.
                let cited: Vec<&DayTask> = if m.outcome == Outcome::NotTouched {
                    Vec::new()
                } else {
                    m.day_task_ids
                        .iter()
                        .filter_map(|id| by_id.get(id.as_str()).copied())
                        .collect()
                };
                accounted.extend(cited.iter().map(|t| t.id.clone()));

                PlanVerdict {
                    task_key: item.task_key.clone(),
                    title: item.title.clone(),
                    outcome: m.outcome,
                    evidence: m.evidence.clone(),
                    minutes: cited.iter().map(|t| t.minutes).sum(),
                    day_task_ids: cited.iter().map(|t| t.id.clone()).collect(),
                    certain: false,
                    provider: item.provider.clone(),
                    url: item.url.clone(),
                }
            }
        };
        verdicts.push(v);
    }

    let planned = verdicts.len() as i64;
    let done = verdicts
        .iter()
        .filter(|v| v.outcome == Outcome::Done)
        .count() as i64;
    let partial = verdicts
        .iter()
        .filter(|v| v.outcome == Outcome::Partial)
        .count() as i64;
    let not_touched = planned - done - partial;

    // Half-credit arithmetic in integers, rounded half-up: done counts 2, partial 1,
    // out of 2 per planned item. Floats here would put a 66.66666 on a screen whose
    // whole job is to feel considered.
    let achievement_pct = if planned == 0 {
        0
    } else {
        let earned: i64 = verdicts.iter().map(|v| v.outcome.half_credits()).sum();
        (earned * 100 + planned) / (planned * 2)
    };

    // The unplanned tail, measured the same way the headline count is: substantial
    // workstreams only. Counting a five-minute glance as "unplanned work" would
    // make every day look scattered.
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

/// The resolved plan ledger and its arithmetic, kept together so a caller cannot
/// persist one without the other.
#[derive(Debug, Clone)]
pub struct DayLedger {
    pub verdicts: Vec<PlanVerdict>,
    pub adherence: Adherence,
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

    fn task(id: &str, minutes: i64, linked: Option<&str>, posted: Option<&str>) -> DayTask {
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
            linked_ticket: linked.map(str::to_string),
            posted_provider: None,
            posted_target_key: posted.map(str::to_string),
            posted_browse_url: None,
        }
    }

    #[test]
    fn a_posted_worklog_settles_the_ticket() {
        let plan = [item("KAN-1", false)];
        let tasks = [task("T1", 130, None, Some("KAN-1"))];
        let m = prematch(&plan, &tasks);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].evidence, MatchEvidence::WorklogPosted);
        assert_eq!(m[0].minutes, 130);
        assert_eq!(m[0].day_task_ids, vec!["T1"]);
    }

    /// A posted worklog outranks a mere link, so the evidence line the screen shows
    /// is the stronger fact rather than whichever matched first.
    #[test]
    fn posting_outranks_linking() {
        let plan = [item("KAN-1", false)];
        let tasks = [
            task("T1", 40, Some("KAN-1"), None),
            task("T2", 90, None, Some("KAN-1")),
        ];
        let m = prematch(&plan, &tasks);
        assert_eq!(m[0].evidence, MatchEvidence::WorklogPosted);
        assert_eq!(m[0].minutes, 90, "only the posted workstream is counted");
    }

    #[test]
    fn a_closed_ticket_with_no_attributable_work_still_counts() {
        let plan = [item("KAN-9", true)];
        let m = prematch(&plan, &[]);
        assert_eq!(m[0].evidence, MatchEvidence::TicketTerminal);
        assert_eq!(m[0].minutes, 0);
    }

    #[test]
    fn a_ticket_with_no_evidence_is_left_for_the_model() {
        let plan = [item("KAN-2", false)];
        assert!(prematch(&plan, &[task("T1", 60, None, None)]).is_empty());
    }

    /// One ticket, two sittings that became two workstreams: both back it and both
    /// count, or the ledger under-reports time the developer actually spent.
    #[test]
    fn two_workstreams_on_one_ticket_sum() {
        let plan = [item("KAN-1", false)];
        let tasks = [
            task("T1", 60, Some("KAN-1"), None),
            task("T2", 45, Some("KAN-1"), None),
        ];
        let m = prematch(&plan, &tasks);
        assert_eq!(m[0].minutes, 105);
        assert_eq!(m[0].day_task_ids.len(), 2);
    }

    /// The load-bearing rule: a deterministic match is not up for discussion. A
    /// model that says a worklogged ticket was never touched is wrong, and letting
    /// it win would put a number on the screen the evidence contradicts.
    #[test]
    fn the_model_cannot_overturn_a_certain_match() {
        let plan = [item("KAN-1", false)];
        let tasks = [task("T1", 130, None, Some("KAN-1"))];
        let said = [ModelVerdict {
            task_key: "KAN-1".to_string(),
            outcome: Outcome::NotTouched,
            evidence: "I saw nothing".to_string(),
            day_task_ids: Vec::new(),
        }];
        let led = resolve(&plan, &tasks, &said);
        assert_eq!(led.verdicts[0].outcome, Outcome::Done);
        assert!(led.verdicts[0].certain);
        assert_eq!(led.verdicts[0].evidence, "a worklog was posted against it");
        assert_eq!(led.adherence.achievement_pct, 100);
    }

    #[test]
    fn a_planned_ticket_the_model_forgot_reads_as_not_touched() {
        let plan = [item("KAN-1", false), item("KAN-2", false)];
        let said = [ModelVerdict {
            task_key: "KAN-1".to_string(),
            outcome: Outcome::Done,
            evidence: "shipped the reader".to_string(),
            day_task_ids: Vec::new(),
        }];
        let led = resolve(&plan, &[], &said);
        assert_eq!(led.verdicts.len(), 2, "the ledger is as long as the plan");
        assert_eq!(led.verdicts[1].outcome, Outcome::NotTouched);
        assert!(led.verdicts[1].evidence.is_empty());
    }

    /// Half credit for partial, rounded half-up: 1 done + 1 partial + 1 untouched
    /// is 1.5 of 3, which is 50%.
    #[test]
    fn partial_work_earns_half_credit() {
        let plan = [
            item("KAN-1", false),
            item("KAN-2", false),
            item("KAN-3", false),
        ];
        let said = [
            ModelVerdict {
                task_key: "KAN-1".to_string(),
                outcome: Outcome::Done,
                evidence: "done".to_string(),
                day_task_ids: Vec::new(),
            },
            ModelVerdict {
                task_key: "KAN-2".to_string(),
                outcome: Outcome::Partial,
                evidence: "started".to_string(),
                day_task_ids: Vec::new(),
            },
        ];
        let led = resolve(&plan, &[], &said);
        assert_eq!(led.adherence.done, 1);
        assert_eq!(led.adherence.partial, 1);
        assert_eq!(led.adherence.not_touched, 1);
        assert_eq!(led.adherence.achievement_pct, 50);
    }

    /// Two of three done is 66.67, and the screen must say 67 rather than 66 - a
    /// truncating divide quietly shortchanges every uneven day.
    #[test]
    fn the_percentage_rounds_half_up() {
        let plan = [
            item("KAN-1", true),
            item("KAN-2", true),
            item("KAN-3", false),
        ];
        let led = resolve(&plan, &[], &[]);
        assert_eq!(led.adherence.achievement_pct, 67);
    }

    #[test]
    fn no_plan_scores_zero_rather_than_dividing_by_it() {
        let led = resolve(&[], &[task("T1", 90, None, None)], &[]);
        assert_eq!(led.adherence.planned, 0);
        assert_eq!(led.adherence.achievement_pct, 0);
        assert!(led.verdicts.is_empty());
    }

    /// The unplanned tail counts substantial workstreams only, and never one
    /// already accounted for by a planned ticket.
    #[test]
    fn unplanned_minutes_exclude_matched_and_brief_work() {
        let plan = [item("KAN-1", false)];
        let tasks = [
            task("T1", 130, None, Some("KAN-1")), // accounted for
            task("T2", 95, None, None),           // genuinely unplanned
            task("T3", 12, None, None),           // below the floor
        ];
        let led = resolve(&plan, &tasks, &[]);
        assert_eq!(led.adherence.unplanned_minutes, 95);
    }

    fn said(key: &str, outcome: Outcome, ids: &[&str]) -> ModelVerdict {
        ModelVerdict {
            task_key: key.to_string(),
            outcome,
            evidence: "e".to_string(),
            day_task_ids: ids.iter().map(|s| s.to_string()).collect(),
        }
    }

    /// The model points at workstreams and WE measure them. Without this, a day
    /// with no posted worklogs shows every ledger row at zero minutes and counts
    /// the entire day as unplanned - which is what the first cut actually did.
    #[test]
    fn a_model_verdict_claims_the_minutes_of_the_work_it_cites() {
        let plan = [item("KAN-1", false)];
        let tasks = [task("T1", 120, None, None), task("T2", 60, None, None)];
        let led = resolve(&plan, &tasks, &[said("KAN-1", Outcome::Done, &["T1"])]);
        assert_eq!(led.verdicts[0].minutes, 120);
        assert!(!led.verdicts[0].certain, "still a judgement, not a fact");
        assert_eq!(
            led.adherence.unplanned_minutes, 60,
            "only T2 is left unaccounted"
        );
    }

    /// An id the day never contained is dropped rather than trusted - the model
    /// naming a workstream that does not exist must not invent minutes.
    #[test]
    fn an_unknown_workstream_id_contributes_nothing() {
        let plan = [item("KAN-1", false)];
        let tasks = [task("T1", 120, None, None)];
        let led = resolve(&plan, &tasks, &[said("KAN-1", Outcome::Done, &["T9"])]);
        assert_eq!(led.verdicts[0].minutes, 0);
        assert_eq!(led.adherence.unplanned_minutes, 120);
    }

    /// A ticket the model says was never touched claims nothing, whatever ids it
    /// cited - otherwise an untouched row would both show time and shrink the
    /// unplanned tail by work done against something else entirely.
    #[test]
    fn an_untouched_ticket_claims_no_time_even_if_it_cites_work() {
        let plan = [item("KAN-1", false)];
        let tasks = [task("T1", 120, None, None)];
        let led = resolve(
            &plan,
            &tasks,
            &[said("KAN-1", Outcome::NotTouched, &["T1"])],
        );
        assert_eq!(led.verdicts[0].minutes, 0);
        assert_eq!(led.adherence.unplanned_minutes, 120);
    }

    /// One workstream can genuinely advance two planned tickets. Each row reports
    /// it honestly, and the unplanned tail subtracts it once.
    #[test]
    fn one_workstream_may_back_two_tickets_without_double_subtracting() {
        let plan = [item("KAN-1", false), item("KAN-2", false)];
        let tasks = [task("T1", 100, None, None), task("T2", 40, None, None)];
        let led = resolve(
            &plan,
            &tasks,
            &[
                said("KAN-1", Outcome::Done, &["T1"]),
                said("KAN-2", Outcome::Partial, &["T1"]),
            ],
        );
        assert_eq!(led.verdicts[0].minutes, 100);
        assert_eq!(led.verdicts[1].minutes, 100);
        assert_eq!(led.adherence.unplanned_minutes, 40);
    }
}
