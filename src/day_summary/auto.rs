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
//!
//! # Telling the user it happened ([`notify_ready`])
//! A successful auto-compose enqueues a `day_summary.ready` notification. Under
//! `docs/notifications.md`'s taxonomy this is an **Ask**, not Status, and the
//! distinction is worth stating because that document warns - correctly - that
//! Status "feels notification-worthy to the person writing it and is noise to
//! everyone else".
//!
//! Three things separate this from the `system.health` "Back online." toast that
//! warning is about. The user picked the time it fires at, so it is not
//! unsolicited. It announces an artefact they are meant to read - the day's
//! adherence, work list and standup lines - rather than an internal state
//! transition. And the header above says this whole path exists to guarantee the
//! summary EXISTS "for someone who never opens it" - which is precisely the person
//! who will not find it unless told. A summary composed for someone who never
//! learns it is there is work done for nobody.
//!
//! It carries `categories::GENERIC_LINK` (a single \[Open\]) and expires at local
//! midnight, when the day it reviews stops being today. No snooze: an answer that
//! needs a consumer arm and a re-enqueue dedup scheme is not worth inventing for a
//! notification that dies in a few hours anyway.

use chrono::{DateTime, Local, TimeZone, Timelike, Utc};
use meridian_core::day_summaries::DaySummary;
use sqlx::SqlitePool;
use tracing::field::Empty;
use tracing::Instrument;

use crate::notifications::{self, NewNotification};

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
        Ok(s) => {
            tracing::info!(
                day = day_local,
                done = s.adherence.done,
                planned = s.adherence.planned,
                fallback = s.fallback,
                "day summary: auto-composed"
            );
            notify_ready(pool, day_local, &s).await;
        }
        Err(e) => tracing::warn!(
            day = day_local, error = %e,
            "day summary: auto-compose failed - the user can still compose it manually"
        ),
    }
}

/// Tell the user today's summary is ready. See the module header for why this is
/// an Ask rather than Status.
///
/// **Only for a real summary.** A fallback is prose-less - the model was down -
/// so announcing it would send the user to a screen with nothing to read. Silence
/// is also what keeps the retry path working: `run` re-composes a fallback on its
/// next wake, and since `enqueue` is idempotent on `dedup_key`, a notification
/// sent for the fallback would dedup away the one for the real summary that
/// replaces it. The user would then be told about the only version worth reading:
/// never.
///
/// Best-effort, like everything else on this path - a failed enqueue is logged and
/// swallowed, because the summary itself was composed successfully and that is the
/// part that matters.
#[tracing::instrument(skip(pool, s))]
async fn notify_ready(pool: &SqlitePool, day_local: &str, s: &DaySummary) {
    if s.fallback {
        tracing::debug!(day = day_local, "day summary: fallback - not notifying");
        return;
    }

    let dedup = format!("day_summary.ready:{day_local}");
    let body = summary_body(s.adherence.done, s.adherence.planned);
    let mut n = NewNotification::event(
        &dedup,
        "day_summary.ready",
        "Your day summary is ready",
        &body,
    )
    .link(meridian_core::notifications::deep_links::SUMMARY)
    .interactive(meridian_core::notifications::categories::GENERIC_LINK);

    // Expiry is the next local midnight - the moment the day it reviews stops
    // being today. `None` only on a calendar edge; the enqueue still goes out
    // without one, since a summary notification that outlives its day is a far
    // smaller problem than one that never arrives.
    let expires = next_local_midnight_utc(&Local::now());
    if let Some(exp) = expires.as_deref() {
        n = n.expiring(exp);
    }

    match notifications::enqueue(pool, n).await {
        // Logged at INFO because "was the user actually told?" is the first
        // question any report about this feature raises, and the answer is
        // otherwise only visible as a row in the outbox.
        Ok(()) => tracing::info!(
            day = day_local,
            expires = expires.as_deref().unwrap_or(""),
            "day summary: ready notification enqueued"
        ),
        Err(e) => {
            tracing::warn!(day = day_local, error = %e, "day summary: ready notification failed to enqueue")
        }
    }
}

/// The toast body. Split out to be testable, and because the plan-vs-no-plan
/// wording is the sort of thing that reads fine until the day nobody planned.
///
/// A "0 of 0" line would be both useless and faintly accusing, so an unplanned day
/// gets the neutral phrasing instead - the summary still has themes and a work list
/// to show, it just has no plan to score against.
fn summary_body(done: i64, planned: i64) -> String {
    if planned <= 0 {
        return "Open it to see what your day was about.".to_string();
    }
    format!("{done} of {planned} planned tasks done. Open it to review your day.")
}

/// `t` as the UTC ISO-8601 string shape (`2026-07-05T02:30:24Z`) the notifications
/// table string-compares `expires_at` against.
///
/// The shape matters and is not interchangeable with the other UTC formatter in the
/// tree: `meridian_core::date::local_day_bounds` emits `toISOString()` millis
/// (`…:00.000Z`), and in a byte-wise compare `.` sorts BEFORE `Z`, so the same
/// instant spelled that way reads as EARLIER than a `T`-separated now and the row
/// would be treated as expired at birth - never delivered, no error anywhere. See
/// `docs/notifications.md`'s testing section, which documents the same trap for
/// hand-seeded rows.
fn utc_iso(t: &DateTime<Local>) -> String {
    t.with_timezone(&Utc)
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string()
}

/// First instant of the next local day, as a UTC ISO string. `None` only on
/// calendar edges (last representable date, a DST transition swallowing midnight).
///
/// A deliberate local twin of `daily_plan.rs`'s same-named helper rather than a
/// shared one: both producers happen to expire at midnight today, but that is a
/// per-notification policy choice, and hoisting it would make a later change to one
/// silently retime the other.
fn next_local_midnight_utc(now: &DateTime<Local>) -> Option<String> {
    let midnight = now.date_naive().succ_opt()?.and_hms_opt(0, 0, 0)?;
    let local = Local.from_local_datetime(&midnight).earliest()?;
    Some(utc_iso(&local))
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

    #[test]
    fn the_body_names_the_score_only_when_there_was_a_plan() {
        assert_eq!(
            summary_body(3, 5),
            "3 of 5 planned tasks done. Open it to review your day."
        );
        // An unplanned day scores 0 of 0, which is useless and reads as a telling-off.
        assert_eq!(
            summary_body(0, 0),
            "Open it to see what your day was about."
        );
    }

    /// The expiry format is load-bearing: the drain compares `expires_at` as a
    /// STRING against a `T`-separated UTC now. A millis-bearing spelling of the
    /// same instant (`…:00.000Z`) sorts BEFORE it, because `.` < `Z`, and the row
    /// would be dropped as expired at birth without a single error anywhere.
    #[test]
    fn the_midnight_expiry_string_sorts_after_now() {
        let now = Local::now();
        let expiry = next_local_midnight_utc(&now).expect("midnight is computable today");
        assert!(
            expiry > utc_iso(&now),
            "{expiry} must sort after {} in a byte-wise compare",
            utc_iso(&now)
        );
        assert!(expiry.ends_with('Z') && !expiry.contains('.'), "{expiry}");
        assert_eq!(expiry.len(), 20, "{expiry} is not YYYY-MM-DDTHH:MM:SSZ");
    }

    /// An in-memory stand-in for the outbox: `enqueue` needs `dedup_key` UNIQUE for
    /// its `ON CONFLICT DO NOTHING`, and every column it binds to exist.
    async fn outbox() -> SqlitePool {
        use sqlx::sqlite::SqlitePoolOptions;
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE notifications (id INTEGER PRIMARY KEY, dedup_key TEXT UNIQUE, \
             event_key TEXT, severity TEXT, title TEXT, body TEXT, deep_link TEXT, \
             channels TEXT, scheduled_for TEXT, expires_at TEXT, category TEXT, actions TEXT)",
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    fn summary(fallback: bool) -> DaySummary {
        DaySummary {
            day: "2026-08-15".to_string(),
            headline: String::new(),
            narrative: String::new(),
            insights: vec![],
            plan: vec![],
            adherence: meridian_core::day_summaries::Adherence {
                planned: 4,
                done: 3,
                ..Default::default()
            },
            themes: vec![],
            standup: vec![],
            provider: "test".to_string(),
            model: String::new(),
            fallback,
            generated_at: "2026-08-15T12:00:00Z".to_string(),
            evidence_at: String::new(),
        }
    }

    /// The one that matters. A fallback carries no prose, so announcing it would
    /// send the user to an empty screen - and worse, `enqueue` is idempotent on
    /// `dedup_key`, so that row would dedup away the notification for the real
    /// summary the retry composes an hour later. The user would be told about the
    /// only version worth reading: never.
    #[tokio::test]
    async fn notifies_for_a_real_summary_but_never_for_a_fallback() {
        let pool = outbox().await;

        notify_ready(&pool, "2026-08-15", &summary(true)).await;
        let after_fallback: i64 = sqlx::query_scalar("SELECT count(*) FROM notifications")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(after_fallback, 0, "a fallback must not notify");

        notify_ready(&pool, "2026-08-15", &summary(false)).await;
        let row: (
            String,
            String,
            Option<String>,
            Option<String>,
            String,
            Option<String>,
        ) = sqlx::query_as(
            "SELECT dedup_key, event_key, deep_link, category, channels, expires_at \
                 FROM notifications",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(row.0, "day_summary.ready:2026-08-15");
        assert_eq!(row.1, "day_summary.ready");
        assert_eq!(row.2.as_deref(), Some("/summary"));
        assert_eq!(row.3.as_deref(), Some("generic_link"));
        // Both channels, pinned rather than inherited from `event()`'s default.
        // The banner half is the one worth stating: it is what reaches a user
        // whose toast was suppressed by quiet hours (banners are not gated by
        // them) or by Windows having no action buttons at all.
        assert_eq!(row.4, "native,banner");
        // And it must carry an expiry, because the banner is a persistent
        // surface. `active_banners` filters on `expires_at`, so a row with one
        // self-clears at midnight; a row WITHOUT one sits in the dashboard until
        // someone clicks it away - which is how 22 dead `board.hygiene` banners
        // ended up needing migration 077 to remove them.
        assert!(
            row.5.is_some(),
            "a banner-channel row must expire on its own"
        );
    }

    /// Scoped per day, so the pass running every hour from the chosen time to
    /// midnight (and the self-healing catch-up after a sleep) cannot re-toast.
    #[tokio::test]
    async fn a_second_pass_on_the_same_day_is_a_no_op() {
        let pool = outbox().await;
        notify_ready(&pool, "2026-08-15", &summary(false)).await;
        notify_ready(&pool, "2026-08-15", &summary(false)).await;
        notify_ready(&pool, "2026-08-16", &summary(false)).await;
        let n: i64 = sqlx::query_scalar("SELECT count(*) FROM notifications")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(n, 2, "one row per local day, however often the pass runs");
    }
}
