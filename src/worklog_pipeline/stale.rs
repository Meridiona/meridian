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
// ONE PER STEP, NOT ONE PER HOUR. The dedup key carries the staleness rounded
// down to a multiple of `WORKLOG_STALE_MINUTES`, so a task worked all afternoon
// notifies at 15, 30, 45 minutes behind - not every fold, and not once and then
// never again as the gap grows to something absurd. `enqueue` is idempotent on
// that key, so this can run on every tick without a further guard.
//
// # Who calls this
// [`crate::worklog_pipeline::workstream::run`], immediately after the hourly
// fold rewrites the day's tasks - which is the only moment measured minutes can
// have changed.
//
// # Related
// - `meridian-core/src/readers/day_task_worklogs/mod.rs` — `stale_drafts`, and
//   the `WORKLOG_STALE_MINUTES` definition both sides read
// - `src/migrations/077_worklog_stale.sql` — why the baseline is minutes
// - `NOTIFICATIONS.md` — the outbox lifecycle this produces into

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
        // Rounded DOWN to the step, so the key only changes when the gap has
        // grown by another whole step's worth of work.
        let step = (d.stale_minutes / WORKLOG_STALE_MINUTES) * WORKLOG_STALE_MINUTES;
        let dedup = format!("worklog.stale:{day_local}:{}:{step}", d.task_id);
        let body = stale_body(&d.title, d.stale_minutes);
        let n = NewNotification::event(
            &dedup,
            "worklog.stale",
            "Your worklog draft is out of date",
            &body,
        )
        .link("/today");
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
        let step = |m: i64| (m / WORKLOG_STALE_MINUTES) * WORKLOG_STALE_MINUTES;
        assert_eq!(step(WORKLOG_STALE_MINUTES), step(WORKLOG_STALE_MINUTES + 1));
        assert_eq!(
            step(WORKLOG_STALE_MINUTES * 2 - 1),
            step(WORKLOG_STALE_MINUTES)
        );
        assert_ne!(step(WORKLOG_STALE_MINUTES * 2), step(WORKLOG_STALE_MINUTES));
    }
}
