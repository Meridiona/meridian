//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Auto-compose the daily summary once a day, at the user's chosen end-of-day time -
//! right after the worklog auto-generate pass, so the plan matches it reads already
//! exist.
//!
//! # Why it rides the worklog clock
//! The plan ledger is deterministic and built from the worklog matches
//! ([`meridian_core::day_evidence::adherence`]). Those matches are written when a
//! worklog is drafted, and worklogs auto-draft at `worklog_auto_generate_time`
//! ([`crate::pm_worklog::auto_generate`]). So the summary composes on the SAME gate,
//! one step later in [`crate::worklog_pipeline::run_loop`]: worklogs first, then this.
//! Sharing the clock is the point - a summary composed before the day's worklogs
//! would score a plan whose matches had not landed yet.
//!
//! # Once a day, from the chosen time onward - self-healing, never crossing midnight
//! Called on every HH:03 wake but a no-op until the local hour reaches the chosen
//! hour, exactly like [`crate::pm_worklog::auto_generate`]. `>=`, not `==`: a machine
//! asleep at the chosen hour catches up on its next wake that same day, and re-runs
//! are cheap because a summary already composed today (and not a fallback) is left
//! alone. It never reaches back across local midnight.
//!
//! # Fires once, not on every tick
//! Skips when today already has a real (non-fallback) summary, whether this composed
//! it or the user did from the panel. A stale end-of-evening refresh is the on-open
//! staleness check's job ([`meridian_core::day_summaries::is_stale`]), not this - this
//! only guarantees the summary EXISTS by end of day for someone who never opens it.
//! An earlier fallback (the model was down) IS retried, since that left no prose.
//!
//! # Never fails the caller
//! A compose error is logged and swallowed - this is a background nicety, and the
//! manual Compose button always still works.

use chrono::{DateTime, Local, Timelike};
use sqlx::SqlitePool;
use tracing::field::Empty;
use tracing::Instrument;

/// Parse "HH:MM" into the hour. `None` for anything malformed - the settings write
/// path already rejects a bad value, so this is only defensive against a hand-edited
/// file. (Kept local rather than shared with `pm_worklog::auto_generate` so the two
/// gates cannot silently diverge in what they accept.)
fn chosen_hour(s: &str) -> Option<u32> {
    let (h, m) = s.split_once(':')?;
    let (h, m) = (h.parse::<u32>().ok()?, m.parse::<u32>().ok()?);
    (h < 24 && m < 60).then_some(h)
}

/// True when `generated_at` (RFC3339 UTC) falls on local day `day_local`.
///
/// The once-a-day guard: a summary stamped earlier today means the day is already
/// covered. Compared in LOCAL time because "today" is a local day - a UTC-date
/// compare would roll over at the wrong moment for anyone west of UTC.
fn generated_today(generated_at: &str, day_local: &str) -> bool {
    DateTime::parse_from_rfc3339(generated_at)
        .map(|t| t.with_timezone(&Local).format("%Y-%m-%d").to_string() == day_local)
        .unwrap_or(false)
}

/// Compose today's summary if the chosen end-of-day hour has arrived and today does
/// not already have one. A no-op when `worklog_auto_generate_time` is unset, before
/// that hour, on an empty day, or when a real summary already exists for today.
#[tracing::instrument(skip(pool))]
pub async fn maybe_auto_summarise(pool: &SqlitePool, day_local: &str) {
    let span = tracing::info_span!(
        "day_summary.auto.run",
        day = day_local,
        chosen_time = Empty,
        gate = Empty,
    );
    run(pool, day_local).instrument(span).await
}

async fn run(pool: &SqlitePool, day_local: &str) {
    let cur = tracing::Span::current();

    let settings = meridian_core::settings::load_runtime_settings();
    let Some(chosen) = settings.worklog_auto_generate_time else {
        cur.record("gate", "disabled");
        return;
    };
    cur.record("chosen_time", chosen.as_str());

    let Some(hour) = chosen_hour(&chosen) else {
        cur.record("gate", "malformed_time");
        tracing::warn!(chosen, "day summary: auto time is malformed - skipping");
        return;
    };
    if Local::now().hour() < hour {
        cur.record("gate", "before_chosen_hour");
        return;
    }

    // Already composed today (and it carries prose) - leave it. A fallback is
    // retried, because it left the screen without cards.
    match meridian_core::day_summaries::get_day_summary(pool, day_local).await {
        Ok(Some(s)) if !s.fallback && generated_today(&s.generated_at, day_local) => {
            cur.record("gate", "already_done");
            return;
        }
        Ok(_) => {}
        Err(e) => {
            tracing::warn!(day = day_local, error = %e, "day summary: auto existing-check failed - skipping");
            return;
        }
    }

    // Nothing tracked → nothing to summarise. A planned-but-idle day is left to the
    // manual button rather than auto-composing a "0 of N" review nobody asked for.
    match meridian_core::day_tasks::get_day_tasks(pool, day_local).await {
        Ok(resp) if resp.tasks.is_empty() => {
            cur.record("gate", "empty_day");
            return;
        }
        Ok(_) => {}
        Err(e) => {
            tracing::warn!(day = day_local, error = %e, "day summary: auto day-task read failed - skipping");
            return;
        }
    }

    cur.record("gate", "ran");
    match super::generate::generate(pool, day_local).await {
        Ok(s) => tracing::info!(
            day = day_local,
            done = s.adherence.done,
            planned = s.adherence.planned,
            fallback = s.fallback,
            "day summary: auto-composed"
        ),
        Err(e) => tracing::warn!(
            day = day_local, error = %e,
            "day summary: auto-compose failed - the user can still compose it manually"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_chosen_hour() {
        assert_eq!(chosen_hour("18:00"), Some(18));
        assert_eq!(chosen_hour("09:05"), Some(9));
        assert_eq!(chosen_hour("24:00"), None);
        assert_eq!(chosen_hour("nope"), None);
    }

    #[test]
    fn generated_today_compares_in_local_time() {
        // A summary stamped at 03:00 UTC on the 22nd is still the 21st in a far-west
        // zone; the compare must use the local calendar day, so we assert the shape
        // rather than a fixed offset (the test host's zone is unknown).
        let now = Local::now();
        let today = now.format("%Y-%m-%d").to_string();
        assert!(generated_today(&now.to_utc().to_rfc3339(), &today));
        assert!(!generated_today(&now.to_utc().to_rfc3339(), "1999-01-01"));
        assert!(!generated_today("not-a-time", &today));
    }
}
