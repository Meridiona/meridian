//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! The `~/.meridian/plan_auto_opened` marker — the shared record of when the
//! tray last auto-opened the daily planner.
//!
//! One file, two consumers with different questions:
//! - The **tray** (writer) asks "did I already open the planner today?" —
//!   [`opened_today`] — to fire at most once per local day.
//! - The **daemon** (reader) asks "how long ago did it open?" — [`opened_at`]
//!   — to hold the `plan.nudge` reminder back for a grace period after the
//!   auto-open instead of toasting over the window that just opened.
//!
//! The content is a single RFC3339 local timestamp (e.g.
//! `2026-07-13T09:18:52+05:30`); its first 10 chars are the local date, which
//! keeps [`opened_today`] a plain prefix check and stays readable in a shell.
//! This module owns the format so the two sides can never drift.
//!
//! # Who calls this
//! - `tray/src-tauri/src/poll/plan_auto_open.rs` — path + stamp + opened_today.
//! - The tray's `plan_dismissed` command — [`restamp_if_today`], restarting
//!   the hold-back clock when the user closes the planner without confirming.
//! - The daemon's `src/daily_plan.rs::maybe_nudge` — path + opened_today +
//!   opened_at (the nudge hold-back).
//!
//! # Related
//! - [`crate::plan`] — `plan_handled`, the sibling "day already planned" gate.

use chrono::{DateTime, FixedOffset, Local};
use std::path::{Path, PathBuf};

/// File name under `~/.meridian` (same convention as the `onboarded` marker).
pub const MARKER_FILE: &str = "plan_auto_opened";

/// `<meridian_dir>/plan_auto_opened` — callers pass `~/.meridian`.
pub fn marker_path(meridian_dir: &Path) -> PathBuf {
    meridian_dir.join(MARKER_FILE)
}

/// The marker content for an auto-open happening `now`: an RFC3339 local
/// timestamp whose first 10 chars are the local date.
pub fn stamp(now: &DateTime<Local>) -> String {
    now.to_rfc3339_opts(chrono::SecondsFormat::Secs, false)
}

/// True when the marker records an auto-open on `today` (local `YYYY-MM-DD`).
/// A prefix match, so it accepts both the timestamp form and a legacy bare
/// date; whitespace-tolerant; empty/missing/garbage content → `false`.
pub fn opened_today(marker_contents: &str, today: &str) -> bool {
    marker_contents.trim().starts_with(today)
}

/// The instant the auto-open happened, if the marker carries a parseable
/// timestamp. `None` for a legacy bare-date or garbage marker — callers that
/// measure elapsed time treat that as "age unknown".
pub fn opened_at(marker_contents: &str) -> Option<DateTime<FixedOffset>> {
    DateTime::parse_from_rfc3339(marker_contents.trim()).ok()
}

/// Restart the nudge hold-back clock at planner dismissal: re-stamp the
/// marker with `now`, but ONLY when it already records `now`'s local day —
/// i.e. the auto-open actually fired today. A manual planner open/close on a
/// day without the auto-open (feature off, pre-8am machine, tray restart
/// races) must not suppress the daemon's nudge. Returns whether a re-stamp
/// was written; fs errors read as `false` (the clock simply keeps running
/// from the original open — a reminder that's early beats one that's lost).
pub fn restamp_if_today(marker: &Path, now: &DateTime<Local>) -> bool {
    let today = now.format("%Y-%m-%d").to_string();
    let contents = std::fs::read_to_string(marker).unwrap_or_default();
    if !opened_today(&contents, &today) {
        return false;
    }
    std::fs::write(marker, stamp(now)).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stamp_round_trips_and_prefixes_the_local_date() {
        let now = Local::now();
        let s = stamp(&now);
        let today = now.format("%Y-%m-%d").to_string();
        assert!(opened_today(&s, &today));
        assert!(!opened_today(&s, "1999-01-01"));
        let parsed = opened_at(&s).expect("stamp must parse back");
        assert_eq!(parsed.timestamp(), now.timestamp());
    }

    #[test]
    fn opened_today_accepts_legacy_bare_date_and_rejects_garbage() {
        assert!(opened_today("2026-07-13", "2026-07-13"), "legacy bare date");
        assert!(opened_today("2026-07-13\n", "2026-07-13"));
        assert!(opened_today("  2026-07-13T09:18:52+05:30  ", "2026-07-13"));
        assert!(!opened_today("2026-07-12T23:59:59+05:30", "2026-07-13"));
        assert!(!opened_today("", "2026-07-13"));
        assert!(!opened_today("garbage", "2026-07-13"));
    }

    #[test]
    fn restamp_only_refreshes_a_today_marker() {
        let dir = std::env::temp_dir().join(format!("meridian-restamp-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let marker = marker_path(&dir);
        // Pinned to local noon, not `Local::now()`: the "today's marker" case
        // below subtracts 30 minutes and asserts it lands on the same day,
        // which flakes if the test happens to run in the first 30 minutes
        // after local midnight (real-world CI failure - not hypothetical).
        let now = chrono::Local::now()
            .date_naive()
            .and_hms_opt(12, 0, 0)
            .unwrap()
            .and_local_timezone(chrono::Local)
            .single()
            .expect("noon is unambiguous under any offset");

        // No marker (auto-open never fired today) → no re-stamp, no file.
        assert!(!restamp_if_today(&marker, &now));
        assert!(!marker.exists());

        // Stale (prior-day) marker → left untouched.
        std::fs::write(&marker, "1999-01-01T09:00:00+00:00").unwrap();
        assert!(!restamp_if_today(&marker, &now));
        assert!(std::fs::read_to_string(&marker)
            .unwrap()
            .starts_with("1999-01-01"));

        // Today's marker → refreshed to `now` (the dismissal instant).
        let earlier = now - chrono::Duration::seconds(1800);
        std::fs::write(&marker, stamp(&earlier)).unwrap();
        assert!(restamp_if_today(&marker, &now));
        let refreshed = opened_at(&std::fs::read_to_string(&marker).unwrap()).unwrap();
        assert_eq!(refreshed.timestamp(), now.timestamp());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn opened_at_is_none_for_legacy_or_garbage() {
        assert!(
            opened_at("2026-07-13").is_none(),
            "bare date has no instant"
        );
        assert!(opened_at("").is_none());
        assert!(opened_at("garbage").is_none());
        assert!(opened_at("2026-07-13T09:18:52+05:30").is_some());
    }
}
