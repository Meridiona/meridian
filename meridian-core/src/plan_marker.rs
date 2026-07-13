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
