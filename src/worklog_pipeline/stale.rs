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

    for d in &stale {
        let step = notify_step(d.stale_minutes);
        let dedup = format!("worklog.stale:{day_local}:{}:{step}", d.task_id);
        let body = stale_body(&d.title, d.stale_minutes);
        let n = NewNotification::event(
            &dedup,
            "worklog.stale",
            "Your worklog draft is out of date",
            &body,
        )
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
