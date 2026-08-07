//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Per-row throttle for warnings a repeating sweep would otherwise duplicate.
//!
//! Split out of [`super::post`] rather than added to it — that module is already
//! past the repo's 500-line cap, and this is a self-contained decision with no
//! dependency on the posting logic.
//!
//! # Who calls this
//! [`super::post::missing_provider`] — the branch that leaves an approved
//! worklog in place because its provider is not configured on this daemon.
//! That row is deliberately never cleared, so the 60 s approved-sweep
//! re-encounters it forever; unthrottled, a single stuck row produced 1440
//! identical WARNs a day. Measured on a real install it was the ONLY warning
//! the daemon emitted, 224 times in the last 3000 log lines.
//!
//! Why that matters beyond tidiness: the local telemetry spool is the daemon's
//! **sole** log sink, and WARN+ is exactly the severity that egresses to the
//! central error pipeline. One benign, permanently-stuck row would otherwise
//! crowd out every real error on both.
//!
//! # Related
//! - [`super::post`] — `MISSING_PROVIDER_WARN_INTERVAL` sets the period.
//! - `coding_agent_session_ingest::summariser`'s `MAX_ROW_ATTEMPTS` ledger — the
//!   same in-memory, per-process, per-row shape, for the same reason (a restart
//!   should re-report; a fresh log ought to state the condition).

use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Remembers when each row last warned, so repeats inside the interval are
/// suppressed.
///
/// In-memory for the process lifetime. Bounded by the number of stuck rows —
/// which is also the number the user can see and act on in the dashboard — so
/// it cannot grow without a corresponding, visible backlog.
#[derive(Default)]
pub(super) struct WarnThrottle {
    last: HashMap<i64, Instant>,
}

impl WarnThrottle {
    /// True when `row_id` should warn now; records the decision when it does.
    ///
    /// Takes `now` rather than reading the clock so the interval logic is
    /// testable without sleeping for hours.
    pub(super) fn should_warn(&mut self, row_id: i64, now: Instant, interval: Duration) -> bool {
        match self.last.get(&row_id) {
            Some(&last) if now.duration_since(last) < interval => false,
            _ => {
                self.last.insert(row_id, now);
                true
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const INTERVAL: Duration = Duration::from_secs(6 * 3600);

    /// The condition must still be reported — throttling is not silencing.
    #[test]
    fn the_first_occurrence_always_warns() {
        let mut t = WarnThrottle::default();
        assert!(t.should_warn(1, Instant::now(), INTERVAL));
    }

    /// The actual regression: the 60 s sweep re-encounters the same row forever.
    #[test]
    fn repeats_within_the_interval_are_suppressed() {
        let mut t = WarnThrottle::default();
        let t0 = Instant::now();
        assert!(t.should_warn(1, t0, INTERVAL));
        // Every sweep up to (but not including) the six-hour boundary — minute
        // 360 is exactly the interval, where warning again is correct and is
        // pinned by `the_warning_returns_once_the_interval_elapses`.
        for minute in 1..360 {
            assert!(
                !t.should_warn(1, t0 + Duration::from_secs(minute * 60), INTERVAL),
                "row warned again {minute} minutes in, inside the interval"
            );
        }
    }

    /// Throttled, not suppressed forever — a condition that is still true after
    /// the interval must resurface.
    #[test]
    fn the_warning_returns_once_the_interval_elapses() {
        let mut t = WarnThrottle::default();
        let t0 = Instant::now();
        assert!(t.should_warn(1, t0, INTERVAL));
        assert!(!t.should_warn(1, t0 + INTERVAL - Duration::from_secs(1), INTERVAL));
        assert!(t.should_warn(1, t0 + INTERVAL, INTERVAL));
    }

    /// One noisy row must not mask a different row's first report.
    #[test]
    fn rows_are_throttled_independently() {
        let mut t = WarnThrottle::default();
        let t0 = Instant::now();
        assert!(t.should_warn(1, t0, INTERVAL));
        assert!(!t.should_warn(1, t0, INTERVAL));
        assert!(
            t.should_warn(2, t0, INTERVAL),
            "row 2 was never warned about"
        );
    }

    /// After a row resurfaces, its own clock restarts — it must not then warn on
    /// every sweep (the bug this would reintroduce is an off-by-one in which
    /// timestamp gets stored).
    #[test]
    fn resurfacing_restarts_the_interval() {
        let mut t = WarnThrottle::default();
        let t0 = Instant::now();
        assert!(t.should_warn(1, t0, INTERVAL));
        assert!(t.should_warn(1, t0 + INTERVAL, INTERVAL));
        assert!(
            !t.should_warn(1, t0 + INTERVAL + Duration::from_secs(60), INTERVAL),
            "the second warn did not reset the window"
        );
    }
}
