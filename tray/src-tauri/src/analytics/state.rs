//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Persisted per-device analytics bookkeeping, plus the pure cursor rules that
//! decide what (if anything) each poll tick should send.
//!
//! # Why this is its own file
//! Split out of `analytics/mod.rs` when that file passed the repo's 500-line
//! cap. The seam is a real one rather than an arbitrary cut: everything here
//! is either on-disk state or a PURE function of it, so all of it is testable
//! without a Tauri app, a SQLite pool, or a network — which is exactly what
//! the test modules at the bottom do.
//!
//! # Who calls this
//! [`super::maybe_send_daily_tick`] — loads the state once per tick, consults
//! the two decision rules, and persists only after a confirmed send.
//!
//! # Related
//! - [`super::daily`] — assembles the `daily_usage` event whose cursor
//!   ([`AnalyticsState::last_sent_day_by_email`]) lives here.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

/// Persisted analytics bookkeeping. Deliberately its own file — never merged
/// into `settings.json` (which the dashboard reads/writes/displays).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct AnalyticsState {
    /// A random per-device id, generated once and never reused as a PostHog
    /// `distinct_id` (see the module doc for why). Kept only as an event
    /// PROPERTY so cross-device activity for one email is still analyzable.
    /// `#[serde(alias)]` reads an older install's `distinct_id` field under
    /// its old name, so an existing device keeps the same id across the
    /// upgrade rather than silently minting a second one.
    #[serde(alias = "distinct_id")]
    pub(super) device_id: String,
    /// Emails this device has already sent `app_installed` for. A SET, not a
    /// bool: the same device can see more than one signed-in email over its
    /// lifetime (sign-out, sign back in as someone else), and each first
    /// sighting of a given email on THIS device should fire once — see the
    /// module doc's "Install dedup" section. `#[serde(default)]` so an older
    /// install's file (no such field) just starts with an empty set rather
    /// than failing to parse.
    #[serde(default)]
    pub(super) install_events_sent: HashSet<String>,
    /// The last LOCAL calendar day ("YYYY-MM-DD") a `daily_usage` event was
    /// sent for, keyed by the signed-in account email it was accrued under —
    /// NOT a single device-wide cursor. Scoped per email for the same reason
    /// `install_events_sent` is a set: one device can see more than one
    /// signed-in email over its lifetime (sign-out, sign back in as someone
    /// else). A shared cursor would let account A's still-pending day be sent
    /// under account B's `distinct_id` after a switch, and could suppress
    /// account B's own first eligible day as "already sent" — so each email
    /// carries its own cursor. An email absent from the map has observed no
    /// day boundary yet (which never happens before that email's first
    /// sign-in — see [`maybe_send_daily_tick`]). `#[serde(default)]` so an
    /// older install's file (no such field, or the old scalar `last_sent_day`)
    /// just starts empty rather than failing to parse — the currently
    /// signed-in email simply re-arms today with no send, never a double-send.
    #[serde(default)]
    pub(super) last_sent_day_by_email: HashMap<String, String>,
    /// The last LOCAL calendar day an `app_active` heartbeat was sent for, per
    /// signed-in email. Scoped per email for the same reason
    /// [`AnalyticsState::last_sent_day_by_email`] is — one device can see more
    /// than one account over its lifetime, and a shared cursor would let one
    /// account's heartbeat suppress another's.
    ///
    /// Deliberately a SEPARATE cursor from `last_sent_day_by_email`: the two
    /// events answer different questions on different schedules. The heartbeat
    /// is about TODAY ("they are here now") and fires on the first tick of the
    /// day; `daily_usage` is about a day that has CLOSED and fires the day
    /// after. Sharing one cursor would couple them and make the heartbeat
    /// inherit the no-backfill rule that makes `daily_usage` unusable for
    /// retention — see [`day_active_action`].
    #[serde(default)]
    pub(super) last_active_day_by_email: HashMap<String, String>,
}

pub(super) fn analytics_state_path() -> Option<PathBuf> {
    meridian_core::paths::home_dir().map(|h| h.join(".meridian/analytics_state.json"))
}

/// Load the state file, creating a fresh one (new random `device_id`) if
/// absent or unparseable. Never errors — a corrupt/missing file just starts a
/// new device id, same as a genuine first run.
pub(super) fn load_or_init_state(path: &std::path::Path) -> AnalyticsState {
    if let Ok(s) = std::fs::read_to_string(path) {
        if let Ok(state) = serde_json::from_str::<AnalyticsState>(&s) {
            return state;
        }
    }
    AnalyticsState {
        device_id: uuid::Uuid::new_v4().to_string(),
        install_events_sent: HashSet::new(),
        last_sent_day_by_email: HashMap::new(),
        last_active_day_by_email: HashMap::new(),
    }
}

/// Crash-safely persist the state file via the shared atomic-write helper
/// ([`meridian_core::fs_utils::atomic_write_json`]) — best-effort, logs on
/// failure like the rest of this module.
pub(super) fn save_state(path: &std::path::Path, state: &AnalyticsState) {
    if let Err(e) = meridian_core::fs_utils::atomic_write_json(path, state) {
        tracing::warn!(error = %e, "analytics: could not persist state file");
    }
}

/// Pure day-rollover decision, split out from [`maybe_send_daily_tick`] so it
/// can be unit-tested without a live DB/HTTP client (see tests below).
/// `None` → nothing to do this tick. `Some(None)` → arm the first-observed
/// day with no send (nothing has closed yet). `Some(Some(day))` → `day` just
/// closed; send its usage, then the caller advances past it on success.
///
/// NOTE: only the single most-recently-closed day is ever reported. If the
/// tray isn't running across more than one local-day boundary (closed, then
/// reopened days later), the intervening day(s) are skipped, not backfilled —
/// an accepted "today/yesterday-only, no backfill" simplification consistent
/// with the rest of the daemon's daily-cadence jobs.
pub(super) fn day_rollover_action(
    last_sent_day: Option<&str>,
    today: &str,
) -> Option<Option<String>> {
    match last_sent_day {
        None => Some(None),
        // The closed day to report is always `today`'s own yesterday, never the stale
        // cursor - `last_sent_day` only marks "the tick that last ran", not "the day
        // whose usage is outstanding" (the caller advances it to `today` on send, see
        // `maybe_send_daily_tick`). Using the cursor value directly here used to
        // report an arbitrarily stale day after a multi-day gap instead of the one
        // day this function's own doc promises - see `yesterday`'s doc.
        Some(prev) if prev != today => Some(Some(yesterday(today))),
        _ => None,
    }
}

/// `today - 1 day`, as a local `'YYYY-MM-DD'` string. Falls back to `today` itself
/// on an unparseable input (should be unreachable — `today` always comes from
/// [`meridian_core::date::today_string`] — but this must never panic on a tick).
fn yesterday(today: &str) -> String {
    chrono::NaiveDate::parse_from_str(today, "%Y-%m-%d")
        .ok()
        .and_then(|d| d.pred_opt())
        .map(|d| d.format("%Y-%m-%d").to_string())
        .unwrap_or_else(|| today.to_string())
}

/// Whether to send an `app_active` heartbeat this tick: true on the first tick
/// of a local day this email hasn't been seen active on yet.
///
/// # Why this exists at all
/// `daily_usage` looks like it should answer "do people come back every day",
/// and it does not. Read [`day_rollover_action`] again: it reports the
/// most-recently-CLOSED day, only when the tray happens to be running after
/// the boundary, and it never backfills. So a `daily_usage` for 2026-05-14
/// means "the tray was alive at some point on 2026-05-15", the first day after
/// sign-in is structurally unreportable, and any day the user was active but
/// the tray didn't survive the following midnight vanishes entirely. Retention
/// computed from it is systematically low — and, worse, plausible-looking.
///
/// The heartbeat has none of that shape: it is about TODAY, it fires the
/// moment the tray is running on a new local day, and it is independent of
/// whether any usage has accumulated yet. That makes it the one honest
/// return-rate signal, at a cost of exactly one extra event per user per
/// active day.
///
/// Note this still measures "the tray was running", not "the user interacted"
/// — the tray launches at login and stays resident, so an unattended machine
/// still emits a heartbeat. It is an availability signal; pair it with the
/// `daily_usage` action counters to tell present-and-working from merely on.
pub(super) fn day_active_action(last_active_day: Option<&str>, today: &str) -> bool {
    last_active_day != Some(today)
}

#[cfg(test)]
mod day_active_tests {
    //! Coverage for the `app_active` heartbeat's decision rule. These pin the
    //! properties that make it usable for retention where `daily_usage` is not
    //! — see [`super::day_active_action`]'s doc.
    use super::day_active_action;

    #[test]
    fn fires_on_the_first_tick_of_a_new_day() {
        assert!(day_active_action(Some("2026-07-08"), "2026-07-09"));
    }

    #[test]
    fn fires_on_the_very_first_tick_ever() {
        // Unlike `day_rollover_action`, which arms silently on first sight,
        // the heartbeat must fire immediately: the day a user signs in IS a
        // day they were active, and dropping it would understate day-0
        // retention for every single user.
        assert!(day_active_action(None, "2026-07-09"));
    }

    #[test]
    fn does_not_repeat_within_the_same_day() {
        // The poll loop ticks ~every 60s; without this the heartbeat would be
        // ~1440 events per user per day instead of 1.
        assert!(!day_active_action(Some("2026-07-09"), "2026-07-09"));
    }

    #[test]
    fn a_multi_day_gap_still_fires_once_on_return() {
        // The tray was closed for a week. The heartbeat reports TODAY (it
        // never backfills the missed days either) — the point is that the
        // return itself is recorded, which is exactly what retention needs.
        assert!(day_active_action(Some("2026-07-01"), "2026-07-09"));
    }
}

#[cfg(test)]
mod day_rollover_tests {
    use super::day_rollover_action;

    #[test]
    fn first_observation_arms_without_sending() {
        assert_eq!(day_rollover_action(None, "2026-07-09"), Some(None));
    }

    #[test]
    fn same_day_is_a_no_op() {
        assert_eq!(day_rollover_action(Some("2026-07-09"), "2026-07-09"), None);
    }

    #[test]
    fn day_boundary_reports_the_closed_day() {
        assert_eq!(
            day_rollover_action(Some("2026-07-08"), "2026-07-09"),
            Some(Some("2026-07-08".to_string()))
        );
    }

    #[test]
    fn multi_day_gap_reports_only_the_last_closed_day() {
        // A gap of several days (tray closed then reopened) still reports only
        // the SINGLE most recently closed day - 2026-07-08, one day before
        // `today` - never the stale cursor (2026-07-01) and never a backfill of
        // every day in between (see the doc comment on `day_rollover_action`).
        assert_eq!(
            day_rollover_action(Some("2026-07-01"), "2026-07-09"),
            Some(Some("2026-07-08".to_string()))
        );
    }

    #[test]
    fn yesterday_crosses_a_month_and_year_boundary() {
        assert_eq!(super::yesterday("2026-03-01"), "2026-02-28");
        assert_eq!(super::yesterday("2027-01-01"), "2026-12-31");
    }
}

#[cfg(test)]
mod per_email_cursor_tests {
    //! Regression coverage for the per-email daily-usage cursor
    //! ([`AnalyticsState::last_sent_day_by_email`]). These mirror the exact
    //! cursor read/advance that [`maybe_send_daily_tick`] performs for the
    //! signed-in email, minus the live DB/HTTP send, so they assert the
    //! scoping decision without a Tauri app or SQLite pool.
    use super::{day_rollover_action, AnalyticsState};
    use std::collections::{HashMap, HashSet};

    fn state_with_cursors(cursors: &[(&str, &str)]) -> AnalyticsState {
        AnalyticsState {
            device_id: "test-device".to_string(),
            install_events_sent: HashSet::new(),
            last_sent_day_by_email: cursors
                .iter()
                .map(|(email, day)| (email.to_string(), day.to_string()))
                .collect::<HashMap<_, _>>(),
            // Not exercised by these tests — the heartbeat cursor is
            // deliberately independent of the daily_usage one, and is covered
            // separately in `day_active_tests`.
            last_active_day_by_email: HashMap::new(),
        }
    }

    /// The day (if any) that `maybe_send_daily_tick` would actually SEND a
    /// `daily_usage` for, for `email` on `today` — reading only that email's
    /// own cursor, exactly as the tick does. `None` = arm-only / no-op.
    fn day_to_send_for(state: &AnalyticsState, email: &str, today: &str) -> Option<String> {
        match day_rollover_action(
            state.last_sent_day_by_email.get(email).map(|s| s.as_str()),
            today,
        ) {
            Some(Some(day)) => Some(day),
            _ => None,
        }
    }

    #[test]
    fn account_switch_does_not_misattribute_pending_day() {
        // Account A armed its cursor at 2026-07-08 while signed in; a local-day
        // boundary then passed (today is 2026-07-09) with A's 2026-07-08 usage
        // still pending. A signs out and B signs in before the next tick fires.
        let state = state_with_cursors(&[("a@example.com", "2026-07-08")]);

        // B has no cursor of its own, so the tick arms B at today WITHOUT
        // sending anything — A's pending 2026-07-08 day is never attributed to
        // B. (The pre-fix shared cursor would have sent 2026-07-08 under B's
        // email, since prev != today.)
        assert_eq!(day_to_send_for(&state, "b@example.com", "2026-07-09"), None);

        // And A's own cursor is untouched: when A signs back in, its pending
        // 2026-07-08 day is still sent — under A's own email, where it belongs.
        assert_eq!(
            day_to_send_for(&state, "a@example.com", "2026-07-09"),
            Some("2026-07-08".to_string())
        );
    }

    #[test]
    fn newly_signed_in_account_still_gets_its_first_full_day() {
        // A device that has already reported days for account A. Account B
        // signs in fresh — a shared cursor sitting at A's latest day could
        // suppress B's first eligible day as "already sent".
        let mut state = state_with_cursors(&[("a@example.com", "2026-07-09")]);

        // First tick after B signs in: nothing has closed for B yet → arm
        // only, no send.
        assert_eq!(day_to_send_for(&state, "b@example.com", "2026-07-09"), None);
        state
            .last_sent_day_by_email
            .insert("b@example.com".to_string(), "2026-07-09".to_string());

        // Next local day: B's own first full day (2026-07-09) is now sent,
        // independent of A's cursor.
        assert_eq!(
            day_to_send_for(&state, "b@example.com", "2026-07-10"),
            Some("2026-07-09".to_string())
        );
    }
}
