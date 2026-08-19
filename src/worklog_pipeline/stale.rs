//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Telling the user a worklog draft has fallen behind the work it describes.
//
// A draft is written from the work that existed at the moment it was generated.
// Keep working the same task afterwards - which is the NORMAL case, since drafts
// are generated mid-afternoon and the afternoon carries on - and it quietly
// stops describing that task. Nothing said so. The two ways that ended were both
// bad and both silent: an update went out missing half a day's work, or the user
// noticed once and stopped trusting the drafts, which costs the feature.
//
// WHY A NOTIFICATION AND NOT ONLY A BADGE. The dashboard already shows the state
// (see `stale` on the draft) - but the entire premise of the product is that the
// user is not looking at the dashboard. A badge nobody is in front of is not a
// warning, and the moment the information matters is the moment the work
// happened, not whenever they next open the window.
//
// ONE PER STEP, NOT ONE PER HOUR. The dedup key carries the staleness bucketed
// by `notify_step` - the threshold, doubling: 15, 30, 60, 120 minutes behind -
// so a task worked all afternoon escalates a few times and then goes quiet,
// rather than notifying on every fold or falling silent as the gap grows to
// something absurd. `enqueue` is idempotent on that key, so this can run on
// every tick without a further guard. The doubling is load-bearing and not a
// stylistic choice: see `notify_step`, where a linear step is what turned this
// guarantee into one notification per hour, forever.
//
// ONE NOTIFICATION PER FOLD, NOT ONE PER TASK. A day with several drafted tasks
// worked past their staleness threshold in the same hourly fold used to fire one
// toast per task - five stale drafts meant five toasts landing at once. `notify_step`
// already collapses repetition *within* a task; this collapses repetition *across*
// tasks in the same run: every draft `stale_drafts` returns this tick becomes ONE
// combined notification (`batch_message`), keyed on the full (task, step) set that
// composed it (`batch_dedup_key`) so the same set never re-fires, but a newly-stale
// task or a further escalation - which changes the set - produces a fresh one.
//
// # Who calls this
// [`crate::worklog_pipeline::workstream::run`], immediately after the hourly
// fold rewrites the day's tasks - which is the only moment measured minutes can
// have changed.
//
// # Related
// - `meridian-core/src/readers/day_task_worklogs/mod.rs` — `stale_drafts`, and
//   the `WORKLOG_STALE_MINUTES` definition both sides read
// - `src/migrations/079_worklog_stale.sql` — why the baseline is minutes
// - `docs/notifications.md` — the outbox lifecycle this produces into

use meridian_core::day_task_worklogs::{stale_drafts, WORKLOG_STALE_MINUTES};
use sqlx::SqlitePool;

use crate::notifications::{self, NewNotification};

/// Notify for every draft on `day_local` that has fallen at least
/// [`WORKLOG_STALE_MINUTES`] behind its task.
///
/// Best-effort by contract: this runs at the tail of the hourly fold, and a
/// failure to tell someone about a stale draft must never fail the fold that
/// produced the day's tasks. Every error is logged and swallowed.
#[tracing::instrument(skip(pool))]
pub async fn notify_stale_drafts(pool: &SqlitePool, day_local: &str) {
    let stale = match stale_drafts(pool, day_local).await {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(day = day_local, error = %e, "worklog: stale-draft sweep failed");
            return;
        }
    };
    if stale.is_empty() {
        return;
    }

    let dedup = batch_dedup_key(day_local, &stale);
    let (title, body) = batch_message(&stale);
    let n = NewNotification::event(&dedup, "worklog.stale", &title, &body)
        // The draft is edited in the worklog review surface, not on the
        // timeline. `/today` merely closed any open modal, which resolved but
        // landed nowhere useful — the one ALL entry whose handler was still
        // behaviourally a no-op.
        .link(meridian_core::notifications::deep_links::WORKLOGS);
    if let Err(e) = notifications::enqueue(pool, n).await {
        tracing::warn!(
            day = day_local,
            error = %e,
            "worklog: could not enqueue stale-draft notice"
        );
    }

    tracing::info!(
        day = day_local,
        n_stale = stale.len(),
        "worklog: stale drafts notified"
    );
}

/// The notification body: what went out of date, and by how much.
///
/// Names the task and the amount, because "a draft is stale" is not actionable -
/// a user with four drafts open cannot tell which one to rewrite, and "18
/// minutes" is what decides whether it is worth doing now or at the end of the
/// day. Plain hyphen only: this is user-facing app text.
///
/// Pure, and unit-tested for the unit boundaries - a body reading "60 minutes"
/// where it should read "1h" is the sort of thing that survives review and then
/// looks broken to every user at once.
fn stale_body(title: &str, minutes: i64) -> String {
    let amount = if minutes < 60 {
        format!("{minutes} more minutes")
    } else {
        let h = minutes / 60;
        let m = minutes % 60;
        let hours = if h == 1 {
            "1 hour".to_string()
        } else {
            format!("{h} hours")
        };
        if m == 0 {
            format!("{hours} more")
        } else {
            format!("{hours} {m} min more")
        }
    };
    format!("You have worked {amount} on \"{title}\" since the draft was written. Regenerate it before you post.")
}

/// The dedup key for a whole fold's worth of stale drafts, not one task.
///
/// Built from every `(task_id, step)` pair in `stale`, sorted so key order never
/// depends on `stale_drafts`'s row order. `enqueue`'s idempotency then does the
/// same job it always did - the same set of tasks at the same steps produces the
/// same key and is a no-op - but the unit of "same" is now the whole batch: any
/// task newly crossing the threshold, or escalating to the next step, changes the
/// set and earns a fresh notification, same as before.
fn batch_dedup_key(
    day_local: &str,
    stale: &[meridian_core::day_task_worklogs::StaleDraft],
) -> String {
    let mut parts: Vec<String> = stale
        .iter()
        .map(|d| format!("{}@{}", d.task_id, notify_step(d.stale_minutes)))
        .collect();
    parts.sort();
    format!("worklog.stale:{day_local}:{}", parts.join(","))
}

/// The title and body for a fold's worth of stale drafts.
///
/// A single stale draft keeps the original, specific wording - naming the task
/// and the exact amount is the whole point when there is only one to act on. Two
/// or more collapse into one notification naming every task (three or fewer) or
/// the first two plus a count (more than three), because at that point which
/// exact minute each one crossed its threshold is not the decision the user needs
/// to make - regenerating all of them is.
fn batch_message(stale: &[meridian_core::day_task_worklogs::StaleDraft]) -> (String, String) {
    if let [only] = stale {
        return (
            "Your worklog draft is out of date".to_string(),
            stale_body(&only.title, only.stale_minutes),
        );
    }

    let title = format!("{} worklog drafts are out of date", stale.len());
    // Sorted for the same reason `batch_dedup_key` sorts its parts: `stale`'s row
    // order isn't part of the contract (it's whatever the hourly fold's query
    // happened to return), but `batch_dedup_key` already treats the SET as the
    // identity, not the order - a body built from the unsorted order would
    // silently vary run to run for the identical set, and for a batch of 4+ it
    // decides which two names actually get named, not just their order.
    let mut names: Vec<&str> = stale.iter().map(|d| d.title.as_str()).collect();
    names.sort_unstable();
    let listed = match names.as_slice() {
        [a, b] => format!("\"{a}\" and \"{b}\""),
        [a, b, c] => format!("\"{a}\", \"{b}\", and \"{c}\""),
        _ => {
            let rest = names.len() - 2;
            format!("\"{}\", \"{}\", and {rest} more", names[0], names[1])
        }
    };
    let body = format!(
        "{listed} have new work since their drafts were written. Regenerate them before you post."
    );
    (title, body)
}

/// The dedup bucket a given staleness falls in: `WORKLOG_STALE_MINUTES` doubling each
/// time — 15, 30, 60, 120, 240 minutes behind.
///
/// A LINEAR step (every 15 minutes) is what this used to be, and it did not survive
/// contact with the caller's cadence. `notify_stale_drafts` runs once per hourly fold,
/// and an hour spent on the task adds about sixty minutes of staleness — four whole
/// linear steps — so the key had ALWAYS moved by the time the next fold looked at it.
/// The documented "one per step, not one per hour" guarantee therefore delivered
/// exactly one per hour, every hour, for as long as the draft went unregenerated: the
/// nagging the dedup key exists to prevent, arriving reliably.
///
/// Doubling fixes it without giving up the early warning, because it matches how the
/// news actually decays. The first 15 minutes behind is worth saying once; going from
/// 15 to 30 is worth saying; going from 4 hours to 4 hours 15 is not news, it is the
/// same fact again. Over a full working day this is at most five notifications for one
/// draft instead of eight and counting, and it stays a pure function of `stale_minutes`
/// — no state, so `enqueue`'s idempotency on the key is still the only guard needed.
fn notify_step(stale_minutes: i64) -> i64 {
    let mut step = WORKLOG_STALE_MINUTES;
    while stale_minutes >= step.saturating_mul(2) {
        step = step.saturating_mul(2);
    }
    step
}

#[cfg(test)]
mod tests {
    use super::*;
    use meridian_core::day_task_worklogs::StaleDraft;

    fn draft(task_id: &str, title: &str, minutes: i64) -> StaleDraft {
        StaleDraft {
            task_id: task_id.to_string(),
            title: title.to_string(),
            stale_minutes: minutes,
        }
    }

    #[test]
    fn a_single_stale_draft_keeps_the_specific_wording() {
        let (title, body) = batch_message(&[draft("T1", "Fixing the login bug", 18)]);
        assert_eq!(title, "Your worklog draft is out of date");
        assert!(body.contains("Fixing the login bug"));
        assert!(body.contains("18 more minutes"));
    }

    #[test]
    fn two_stale_drafts_are_both_named() {
        let (title, body) = batch_message(&[
            draft("T1", "Release notes", 20),
            draft("T2", "Bug triage", 40),
        ]);
        assert_eq!(title, "2 worklog drafts are out of date");
        // Alphabetical, not input order - see the sort in `batch_message`.
        assert!(body.contains("\"Bug triage\" and \"Release notes\""));
    }

    /// The whole point of sorting: the SAME stale set, folded in a different row
    /// order (e.g. a different query plan, or a task's `stale_minutes` landing it
    /// earlier/later in whatever produced `stale`), must produce the identical
    /// notification body - not just the identical dedup key.
    #[test]
    fn the_batch_body_is_order_independent() {
        let forward = [
            draft("T1", "Release notes", 20),
            draft("T2", "Bug triage", 40),
        ];
        let backward = [
            draft("T2", "Bug triage", 40),
            draft("T1", "Release notes", 20),
        ];
        assert_eq!(batch_message(&forward), batch_message(&backward));
    }

    #[test]
    fn more_than_three_stale_drafts_are_summarised() {
        let stale = vec![
            draft("T1", "A", 20),
            draft("T2", "B", 20),
            draft("T3", "C", 20),
            draft("T4", "D", 20),
            draft("T5", "E", 20),
        ];
        let (title, body) = batch_message(&stale);
        assert_eq!(title, "5 worklog drafts are out of date");
        assert!(body.contains("\"A\", \"B\", and 3 more"));
        assert!(!body.contains('C'), "{body}");
    }

    #[test]
    fn the_batch_dedup_key_is_order_independent() {
        let forward = vec![draft("T1", "A", 20), draft("T2", "B", 45)];
        let backward = vec![draft("T2", "B", 45), draft("T1", "A", 20)];
        assert_eq!(
            batch_dedup_key("2026-08-19", &forward),
            batch_dedup_key("2026-08-19", &backward)
        );
    }

    #[test]
    fn the_batch_dedup_key_changes_when_a_task_escalates() {
        let before = vec![draft("T1", "A", 20)]; // step 15
        let after = vec![draft("T1", "A", 35)]; // step 30
        assert_ne!(
            batch_dedup_key("2026-08-19", &before),
            batch_dedup_key("2026-08-19", &after)
        );
    }

    #[test]
    fn the_batch_dedup_key_changes_when_a_task_is_added() {
        let before = vec![draft("T1", "A", 20)];
        let after = vec![draft("T1", "A", 20), draft("T2", "B", 20)];
        assert_ne!(
            batch_dedup_key("2026-08-19", &before),
            batch_dedup_key("2026-08-19", &after),
            "a newly-stale task joining the batch must produce a fresh notification"
        );
    }

    #[test]
    fn reads_as_minutes_below_the_hour() {
        let b = stale_body("Fixing the login bug", 18);
        assert!(b.contains("18 more minutes"), "{b}");
        assert!(b.contains("Fixing the login bug"), "{b}");
    }

    #[test]
    fn switches_to_hours_rather_than_saying_ninety_minutes() {
        assert!(stale_body("T", 60).contains("1 hour more"));
        assert!(stale_body("T", 90).contains("1 hour 30 min more"));
        assert!(stale_body("T", 125).contains("2 hours 5 min more"));
        assert!(stale_body("T", 120).contains("2 hours more"));
    }

    #[test]
    fn never_uses_a_dash_that_is_not_a_hyphen() {
        // User-facing app text: plain hyphen only (see CLAUDE.md's hard rules).
        for m in [5, 18, 60, 90, 125] {
            let b = stale_body("Some task", m);
            assert!(!b.contains('\u{2014}') && !b.contains('\u{2013}'), "{b}");
        }
    }

    #[test]
    fn the_dedup_step_only_moves_once_per_whole_step() {
        // The property the once-per-step guarantee rests on: everything inside
        // one step shares a key, and `enqueue` is idempotent on it.
        assert_eq!(
            notify_step(WORKLOG_STALE_MINUTES),
            notify_step(WORKLOG_STALE_MINUTES + 1)
        );
        assert_eq!(
            notify_step(WORKLOG_STALE_MINUTES * 2 - 1),
            notify_step(WORKLOG_STALE_MINUTES)
        );
        assert_ne!(
            notify_step(WORKLOG_STALE_MINUTES * 2),
            notify_step(WORKLOG_STALE_MINUTES)
        );
    }

    /// The reason the step doubles instead of stepping linearly.
    ///
    /// This function's only caller runs once an hour, and an hour of work on the task
    /// adds about an hour of staleness. Under a linear step that cleared four buckets
    /// per fold, so the key had always moved and the user was notified about the same
    /// unregenerated draft every single hour - which is precisely what the dedup key
    /// was introduced to stop.
    #[test]
    fn a_draft_worked_all_day_does_not_notify_every_hour() {
        // A full working day of continuous work on one un-regenerated draft, sampled
        // as the hourly fold would see it.
        let hourly: Vec<i64> = (1..=8).map(|h| h * 60).collect();
        let buckets: std::collections::BTreeSet<i64> =
            hourly.iter().copied().map(notify_step).collect();
        assert!(
            buckets.len() <= 4,
            "eight hourly folds must not mean eight notifications, got {buckets:?}"
        );

        // ...while the early, genuinely informative escalation still gets through:
        // first crossing, then each doubling.
        assert_ne!(notify_step(15), notify_step(30));
        assert_ne!(notify_step(30), notify_step(60));
        assert_ne!(notify_step(60), notify_step(120));

        // ...and a big gap growing slightly is not news a second time.
        assert_eq!(notify_step(240), notify_step(255));
    }

    /// Below the threshold nothing is ever enqueued (`stale_drafts` filters those out),
    /// but the bucket must still be well-defined rather than dividing by a smaller step.
    #[test]
    fn the_first_bucket_is_the_threshold_itself() {
        assert_eq!(notify_step(0), WORKLOG_STALE_MINUTES);
        assert_eq!(
            notify_step(WORKLOG_STALE_MINUTES - 1),
            WORKLOG_STALE_MINUTES
        );
    }
}
