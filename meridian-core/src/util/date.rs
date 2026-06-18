//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Local-day boundary helpers — a faithful Rust port of `ui/lib/date-utils.ts`.
//!
//! "Today" is the user's LOCAL calendar day, but timestamps are stored in UTC.
//! So we compute local-midnight .. local-end-of-day and convert each to a UTC
//! RFC3339 string for the `started_at >= start AND started_at < end` query
//! bounds. JS parses a tz-less datetime as local and emits UTC via toISOString();
//! we reproduce both exactly with chrono::Local → Utc.

use chrono::{
    DateTime, Local, LocalResult, NaiveDate, NaiveDateTime, SecondsFormat, TimeZone, Utc,
};

/// `[start, end)` UTC bounds (RFC3339, millis + `Z`) for the LOCAL day `date_str`
/// ("YYYY-MM-DD"). Mirrors `localDayBounds`: local `00:00:00` .. local
/// `23:59:59.999`, each `.toISOString()`-formatted.
pub fn local_day_bounds(date_str: &str) -> (String, String) {
    // Unparseable date → fall back to today (JS would yield an Invalid Date).
    let date = NaiveDate::parse_from_str(date_str, "%Y-%m-%d")
        .unwrap_or_else(|_| Local::now().date_naive());
    let start = date.and_hms_opt(0, 0, 0).expect("00:00:00 is valid");
    let end = date
        .and_hms_milli_opt(23, 59, 59, 999)
        .expect("23:59:59.999 is valid");
    (local_naive_to_utc_iso(start), local_naive_to_utc_iso(end))
}

/// Today's date in the LOCAL timezone as "YYYY-MM-DD" (mirrors `todayString`).
pub fn today_string() -> String {
    Local::now().format("%Y-%m-%d").to_string()
}

/// Whole calendar days from `today` to a due date (`YYYY-MM-DD` or a full
/// datetime), or `None` if absent/unparseable. Mirrors the JS `dueDaysFrom`:
/// "due today" → 0, "due yesterday" → -1, overdue → negative. The single shared
/// impl behind `task_detail` and `plan` so their due math can't drift.
///
/// Known divergence from the JS source on a *datetime* due value: JS parses the
/// full timestamp as LOCAL then takes its local calendar date, whereas we take
/// the leading `YYYY-MM-DD` prefix (a UTC-naive date). For a due near midnight in
/// a far-from-UTC zone these can differ by a day. This edge is accepted (board
/// due dates are date-only in practice) — comment, don't re-solve.
pub fn due_days_from(due: Option<&str>, today: NaiveDate) -> Option<i64> {
    let due = due?;
    let date_part = if due.len() <= 10 { due } else { due.get(..10)? };
    let d = NaiveDate::parse_from_str(date_part, "%Y-%m-%d").ok()?;
    Some((d - today).num_days())
}

/// Interpret a naive datetime as LOCAL wall-clock → UTC → JS `toISOString()`
/// format (millis precision + `Z`).
fn local_naive_to_utc_iso(naive: NaiveDateTime) -> String {
    let utc: DateTime<Utc> = match Local.from_local_datetime(&naive) {
        LocalResult::Single(dt) => dt.with_timezone(&Utc),
        // DST fall-back ambiguity → earliest instant (matches JS).
        LocalResult::Ambiguous(dt, _) => dt.with_timezone(&Utc),
        // DST spring-forward gap (this wall-clock never occurs) → treat as UTC;
        // never hits a normal 00:00/23:59 day bound outside pathological zones.
        LocalResult::None => Utc.from_utc_datetime(&naive),
    };
    utc.to_rfc3339_opts(SecondsFormat::Millis, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(s: &str) -> i64 {
        DateTime::parse_from_rfc3339(s).unwrap().timestamp_millis()
    }

    #[test]
    fn bounds_span_a_full_local_day() {
        // 00:00:00.000 .. 23:59:59.999 = 86_399_999 ms, in any (non-DST-transition)
        // timezone — June 16 is not a DST boundary anywhere relevant.
        let (start, end) = local_day_bounds("2026-06-16");
        assert_eq!(ms(&end) - ms(&start), 86_399_999);
    }

    #[test]
    fn start_round_trips_to_local_midnight() {
        let (start, _) = local_day_bounds("2026-06-16");
        let back = DateTime::parse_from_rfc3339(&start)
            .unwrap()
            .with_timezone(&Local);
        assert_eq!(
            back.format("%Y-%m-%dT%H:%M:%S").to_string(),
            "2026-06-16T00:00:00"
        );
    }

    #[test]
    fn bounds_use_iso_millis_z() {
        let (start, end) = local_day_bounds("2026-06-16");
        assert!(start.ends_with('Z') && start.contains('.'));
        assert!(end.ends_with(".999Z"));
    }

    #[test]
    fn due_days_calendar_diff() {
        let today = NaiveDate::from_ymd_opt(2026, 6, 18).unwrap();
        assert_eq!(due_days_from(Some("2026-06-18"), today), Some(0));
        assert_eq!(due_days_from(Some("2026-06-20"), today), Some(2));
        assert_eq!(due_days_from(Some("2026-06-17"), today), Some(-1));
        // full datetime → date prefix used
        assert_eq!(due_days_from(Some("2026-06-21T15:00:00Z"), today), Some(3));
        // absent / unparseable → None
        assert_eq!(due_days_from(None, today), None);
        assert_eq!(due_days_from(Some("nope"), today), None);
    }

    #[test]
    fn today_string_is_local_yyyy_mm_dd() {
        let t = today_string();
        assert_eq!(t.len(), 10);
        assert_eq!(t.matches('-').count(), 2);
        assert_eq!(t, Local::now().format("%Y-%m-%d").to_string());
    }
}
