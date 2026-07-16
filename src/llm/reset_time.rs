//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Turns a rate-limit message into "how long to actually wait" — instead of guessing.
//!
//! Claude Code, Codex and friends print their own reset hint in the error text: an
//! absolute clock time ("resets 3pm", "resets at 3:00 PM") or a relative window ("try
//! again in 5 hours", "resets in: 3 hours"). [`parse_backoff`] pulls whichever shape is
//! present and returns exactly how long until then; [`super::resolver`]'s backoff falls
//! back to its own flat default when nothing parses (a weekly-limit message that names a
//! full date, or a shape we haven't seen yet).
//!
//! # Ambiguity
//!
//! A bare hour with no am/pm ("resets 3") could mean 3am or 3pm. We pick whichever is
//! sooner in the future — a provider's session window is never more than ~12h out when it
//! prints a bare hour, so the near reading is always the right one.
//!
//! # Related
//! - [`super::resolver`] — the only caller; owns the in-memory backoff state and the flat
//!   fallback duration.

use std::time::Duration;

use chrono::{DateTime, Duration as ChronoDuration, Local, NaiveTime, TimeZone};

/// Cushion added to every parsed wait so a retry doesn't land in the same second the
/// window opens — some providers are strict about the boundary.
const MARGIN: Duration = Duration::from_secs(60);

/// Upper bound on a parsed wait. Guards against a garbled relative parse ("in 999 hours")
/// producing an absurd sleep; every window we actually recognise (5-hour session, daily)
/// fits well inside this. A weekly-limit message names a full date, which we don't parse
/// at all (see [`parse_absolute`]'s doc), so this cap never fires on a legitimate case.
const MAX_BACKOFF: Duration = Duration::from_secs(6 * 3600);

/// How long to wait before retrying, extracted from a provider's own rate-limit message.
/// `None` means the message didn't carry a shape we recognise — the caller should use its
/// own flat backoff instead.
pub fn parse_backoff(message: &str, now: DateTime<Local>) -> Option<Duration> {
    let low = message.to_lowercase();
    let raw = parse_relative(&low).or_else(|| parse_absolute(&low, now))?;
    Some((raw + MARGIN).min(MAX_BACKOFF))
}

const RELATIVE_ANCHORS: &[&str] = &["try again in", "resets in", "reset in", "wait "];

fn parse_relative(low: &str) -> Option<Duration> {
    for anchor in RELATIVE_ANCHORS {
        if let Some(pos) = low.find(anchor) {
            if let Some(d) = parse_number_unit(&low[pos + anchor.len()..]) {
                return Some(d);
            }
        }
    }
    None
}

/// Parses a leading "[in ]<N> (hour|hours|h|minute|minutes|min|mins|m)" off `s`, tolerant
/// of a leading colon/whitespace (the "resets in" anchor leaves ": 3 hours"). When the unit
/// is hours, also consumes a trailing minutes component if one follows — Codex renders
/// "Try again in 3h 42m." and Copilot renders "...reset in 2 hours 15 minutes.", both
/// observed live; dropping the minutes there means retrying up to 59 minutes early.
fn parse_number_unit(s: &str) -> Option<Duration> {
    let s = s.trim_start_matches(|c: char| c == ':' || c.is_whitespace());
    let s = s.strip_prefix("in ").map(str::trim_start).unwrap_or(s);
    let (amount, rest) = take_number(s)?;
    let rest = rest.trim_start();
    let unit_end = rest
        .find(|c: char| !c.is_ascii_alphabetic())
        .unwrap_or(rest.len());
    let unit = &rest[..unit_end];

    if unit.starts_with('m') {
        Some(Duration::from_secs(amount * 60))
    } else if unit.starts_with('h') {
        let mut secs = amount * 3600;
        let after_unit = rest[unit_end..].trim_start();
        if let Some((minutes, mrest)) = take_number(after_unit) {
            let mrest = mrest.trim_start();
            let munit_end = mrest
                .find(|c: char| !c.is_ascii_alphabetic())
                .unwrap_or(mrest.len());
            if mrest[..munit_end].starts_with('m') {
                secs += minutes * 60;
            }
        }
        Some(Duration::from_secs(secs))
    } else {
        None
    }
}

/// Pulls a leading run of ASCII digits off `s`, returning the parsed number and the
/// remainder. `None` if `s` doesn't start with a digit.
fn take_number(s: &str) -> Option<(u64, &str)> {
    let digits_end = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    if digits_end == 0 {
        return None;
    }
    Some((s[..digits_end].parse().ok()?, &s[digits_end..]))
}

/// Parses "reset(s) [at] H[:MM] [am|pm]" and resolves it against `now`, rolling to
/// tomorrow if the time already passed today. A bare hour with no am/pm picks whichever of
/// the two 12h readings is sooner.
///
/// Deliberately does NOT handle a full date ("resets Jul 5th, 2026 1:16 PM") — Codex's
/// weekly-limit shape. That's out of scope here; the caller falls back to its flat backoff
/// rather than this function guessing at a date format that varies by locale.
fn parse_absolute(low: &str, now: DateTime<Local>) -> Option<Duration> {
    let pos = low.find("reset")?;
    let rest = low[pos..].trim_start_matches(|c: char| c.is_alphabetic());
    let rest = rest.trim_start();
    let rest = rest.strip_prefix("at ").unwrap_or(rest).trim_start();

    let digits_end = rest
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(rest.len());
    if digits_end == 0 || digits_end > 2 {
        return None;
    }
    let hour: u32 = rest[..digits_end].parse().ok()?;
    let mut cursor = &rest[digits_end..];

    let mut minute: u32 = 0;
    if let Some(after_colon) = cursor.strip_prefix(':') {
        let mend = after_colon
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(after_colon.len());
        if mend > 0 {
            minute = after_colon[..mend].parse().ok()?;
            cursor = &after_colon[mend..];
        }
    }
    cursor = cursor.trim_start();
    let ampm_pm = if cursor.starts_with("am") {
        Some(false)
    } else if cursor.starts_with("pm") {
        Some(true)
    } else {
        None
    };

    if hour > 23 || minute > 59 {
        return None;
    }

    let candidate_hours: Vec<u32> = match ampm_pm {
        Some(pm) => vec![to_24h(hour, pm)],
        None if (1..=12).contains(&hour) => vec![to_24h(hour, false), to_24h(hour, true)],
        None => vec![hour],
    };

    let today = now.date_naive();
    let mut best: Option<DateTime<Local>> = None;
    for h in candidate_hours {
        let t = NaiveTime::from_hms_opt(h, minute, 0)?;
        let mut dt = Local.from_local_datetime(&today.and_time(t)).single()?;
        if dt <= now {
            dt += ChronoDuration::days(1);
        }
        best = Some(match best {
            Some(b) if b <= dt => b,
            _ => dt,
        });
    }
    best?.signed_duration_since(now).to_std().ok()
}

fn to_24h(hour: u32, pm: bool) -> u32 {
    match (hour, pm) {
        (12, true) => 12,
        (12, false) => 0,
        (h, true) => h + 12,
        (h, false) => h,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(h: u32, m: u32) -> DateTime<Local> {
        let today = Local::now().date_naive();
        Local
            .from_local_datetime(&today.and_hms_opt(h, m, 0).unwrap())
            .single()
            .unwrap()
    }

    #[test]
    fn parses_bare_pm_reset_later_today() {
        let now = at(13, 0); // 1pm
        let wait = parse_backoff("5-hour limit reached ∙ resets 3pm", now).unwrap();
        assert_eq!(wait, Duration::from_secs(2 * 3600 + 60));
    }

    #[test]
    fn parses_reset_with_minutes_and_am() {
        let now = at(0, 30); // 12:30am
        let wait = parse_backoff("resets at 2:15am", now).unwrap();
        assert_eq!(wait, Duration::from_secs(105 * 60 + 60));
    }

    #[test]
    fn a_past_time_rolls_to_tomorrow() {
        let now = at(14, 0); // 2pm
        let wait = parse_backoff("resets 10am", now).unwrap();
        assert_eq!(wait, Duration::from_secs(20 * 3600 + 60).min(MAX_BACKOFF));
    }

    #[test]
    fn ambiguous_bare_hour_picks_the_sooner_reading() {
        // At 1pm, a bare "resets 3" could mean 3pm (2h away) or 3am tomorrow (14h away) —
        // 3pm is sooner.
        let now = at(13, 0);
        let wait = parse_backoff("usage limit — resets 3", now).unwrap();
        assert_eq!(wait, Duration::from_secs(2 * 3600 + 60));
    }

    #[test]
    fn parses_relative_hours() {
        let now = at(9, 0);
        let wait =
            parse_backoff("You've hit your usage limit. Try again in 5 hours.", now).unwrap();
        assert_eq!(wait, Duration::from_secs(5 * 3600 + 60));
    }

    #[test]
    fn parses_compact_hours_and_minutes() {
        // Observed live from Codex: "You've reached your 5-hour message limit. Try again
        // in 3h 42m."
        let now = at(9, 0);
        let wait = parse_backoff(
            "You've reached your 5-hour message limit. Try again in 3h 42m.",
            now,
        )
        .unwrap();
        assert_eq!(wait, Duration::from_secs(3 * 3600 + 42 * 60 + 60));
    }

    #[test]
    fn parses_spelled_out_hours_and_minutes() {
        // Observed live from Copilot: "...your limit to reset in 2 hours 15 minutes."
        let now = at(9, 0);
        let wait = parse_backoff(
            "You've hit your rate limit. Please wait for your limit to reset in 2 hours 15 minutes.",
            now,
        )
        .unwrap();
        assert_eq!(wait, Duration::from_secs(2 * 3600 + 15 * 60 + 60));
    }

    #[test]
    fn parses_relative_with_colon_and_minutes() {
        let now = at(9, 0);
        let wait = parse_backoff("rate limited, resets in: 45 minutes", now).unwrap();
        assert_eq!(wait, Duration::from_secs(45 * 60 + 60));
    }

    #[test]
    fn no_recognisable_shape_returns_none() {
        assert!(parse_backoff(
            "This request would exceed your account's rate limit.",
            at(9, 0)
        )
        .is_none());
    }

    #[test]
    fn a_full_date_weekly_limit_is_not_parsed() {
        // Codex's weekly-limit shape names a date, not a bare time — out of scope, caller
        // falls back to the flat backoff.
        let now = at(9, 0);
        assert!(parse_backoff(
            "You've hit your usage limit. Upgrade to Plus, or try again at Jul 5th, 2026 1:16 PM.",
            now
        )
        .is_none());
    }

    #[test]
    fn a_wildly_long_relative_wait_is_capped() {
        let now = at(9, 0);
        let wait = parse_backoff("rate limit — try again in 999 hours", now).unwrap();
        assert_eq!(wait, MAX_BACKOFF);
    }
}
