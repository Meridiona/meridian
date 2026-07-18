//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Is this endpoint's quota big enough to actually run Meridian?
//!
//! A free-tier key can be perfectly valid, authenticate fine, answer a test prompt —
//! and still be unable to serve a working day. The user has no way to know that from
//! the provider's dashboard, because the number that matters is not "requests per
//! minute" but "requests per day measured against what this app actually asks for".
//! This module owns that arithmetic so the answer is computed once, from measured
//! demand, rather than guessed at in UI copy.
//!
//! # The demand figures are measured, not estimated
//!
//! Every constant below was counted from the call sites, not assumed:
//!
//! * [`CALLS_PER_ACTIVE_HOUR`] — `worklog_pipeline::hour::build_report` makes one call
//!   and `workstream::run` makes one, sequentially, per hour that clears the activity
//!   gate. Quiet hours are marked done without any call at all, and the local
//!   `/distill_hour` step is on-device, so it costs an endpoint nothing.
//! * [`PROBE_MAX_REQUESTS`] — `llm::probe` walks 4 pipeline schemas × a 4-rung ladder,
//!   stopping at the first rung that answers. Typically 4 requests, 16 worst case.
//! * [`DISCRETIONARY_CALLS_PER_DAY`] — the on-demand buttons (day summary, PM worklog
//!   draft, plan-task draft) are one call each. This is the one genuinely soft number:
//!   it is a usage guess, not a code-derived count, which is why the verdict leans on
//!   the automatic figure and treats this as headroom.
//!
//! # Why RPM is not the headline
//!
//! Pacing (`meridian::llm::rate_limit`) spaces requests to whatever RPM is configured,
//! so a low RPM costs TIME, not correctness — a 5-RPM key just makes the setup probe
//! take about three minutes. RPD has no such remedy: when a daily quota is exhausted
//! the work simply does not happen that day. So the daily figure drives the verdict
//! and RPM is reported only when it is low enough to be worth mentioning.
//!
//! # Who calls this
//!
//! `tray/src-tauri/src/commands/custom_llm.rs` — attached to each endpoint's view so
//! the provider card can warn, and consulted when adding one.
//!
//! # Related
//! - [`crate::settings::CustomLlmProvider`] — where `rpm` / `rpd` are stored.

use serde::{Deserialize, Serialize};

/// LLM calls one hour of real activity costs: the activity report, then the
/// workstream fold. Both are skipped entirely for an hour that fails the activity
/// gate, so this is a per-ACTIVE-hour figure.
pub const CALLS_PER_ACTIVE_HOUR: u32 = 2;

/// Active hours in the working day the verdict is sized for.
///
/// A deliberate assumption, and the one most likely to be wrong for a given user —
/// so the copy built from this states it rather than presenting the verdict as fact.
pub const ASSUMED_ACTIVE_HOURS: u32 = 8;

/// Worst-case metered requests to measure one endpoint: 4 pipeline schemas × 4
/// ladder rungs. Typically 4, because most endpoints answer on the first rung.
///
/// This is the sharpest test of a daily quota. An endpoint whose whole day is
/// smaller than its own setup cost cannot be onboarded at all - and unlike the
/// hourly load it is not spread out, so there is nothing to wait for.
pub const PROBE_MAX_REQUESTS: u32 = 16;

/// Allowance for the on-demand buttons over a day. A usage guess, unlike the others.
pub const DISCRETIONARY_CALLS_PER_DAY: u32 = 7;

/// Requests a normal working day needs, excluding one-time setup.
pub const fn daily_demand() -> u32 {
    CALLS_PER_ACTIVE_HOUR * ASSUMED_ACTIVE_HOURS + DISCRETIONARY_CALLS_PER_DAY
}

/// How serious the shortfall is. Ordered from "say nothing" upward.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapacityVerdict {
    /// Comfortably enough for a working day.
    Sufficient,
    /// No limits were entered, so nothing can be judged. Not a problem with the key.
    Unknown,
    /// Will run, but the daily quota is expected to run out before the day does.
    Tight,
    /// Cannot cover a normal day - hours will be dropped once the quota is spent.
    Insufficient,
    /// The daily quota is smaller than the one-time setup probe, so the endpoint
    /// cannot even be measured, and an unmeasured endpoint is refused by the
    /// production gate. This key is unusable regardless of how it is paced.
    CannotOnboard,
}

/// The full assessment of one endpoint's configured limits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapacityAssessment {
    pub verdict: CapacityVerdict,
    /// Active hours per day this quota covers, once the setup probe is paid for.
    /// `None` when RPD is unknown.
    pub covered_active_hours: Option<u32>,
    /// Requests a normal day needs - what `covered_active_hours` is measured against.
    pub daily_demand: u32,
    /// Set when RPM is low enough that the setup probe will visibly pace. Not a
    /// failure: pacing makes it slow, not broken.
    pub probe_seconds_at_rpm: Option<u32>,
}

/// Judge an endpoint's configured limits against what the app actually asks for.
///
/// `rpm` / `rpd` of `0` mean "not known" (the settings default), not "zero allowed" —
/// a user who leaves the fields blank gets [`CapacityVerdict::Unknown`], never a false
/// alarm. Judging is deliberately based on RPD alone: see the module docs on why a low
/// RPM is a delay rather than a defect.
pub fn assess(rpm: u32, rpd: u32) -> CapacityAssessment {
    let demand = daily_demand();

    // Below this many requests per minute the probe's 16-request burst is paced out
    // far enough that a user watching the spinner deserves to be told why.
    let probe_seconds_at_rpm = if rpm > 0 && rpm < PROBE_MAX_REQUESTS {
        // Even spacing: the first request goes immediately, so it is the GAPS that
        // cost time - one fewer than the request count.
        Some((PROBE_MAX_REQUESTS - 1) * 60 / rpm)
    } else {
        None
    };

    if rpd == 0 {
        return CapacityAssessment {
            verdict: CapacityVerdict::Unknown,
            covered_active_hours: None,
            daily_demand: demand,
            probe_seconds_at_rpm,
        };
    }

    // Setup is paid out of the same daily bucket, so it comes off the top before
    // asking how much of the working day is left.
    let after_probe = rpd.saturating_sub(PROBE_MAX_REQUESTS);
    let covered = after_probe / CALLS_PER_ACTIVE_HOUR;

    let verdict = if rpd <= PROBE_MAX_REQUESTS {
        CapacityVerdict::CannotOnboard
    } else if covered >= ASSUMED_ACTIVE_HOURS {
        CapacityVerdict::Sufficient
    } else if covered >= ASSUMED_ACTIVE_HOURS / 2 {
        CapacityVerdict::Tight
    } else {
        CapacityVerdict::Insufficient
    };

    CapacityAssessment {
        verdict,
        covered_active_hours: Some(covered),
        daily_demand: demand,
        probe_seconds_at_rpm,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The case that prompted this: a real free tier at 5 RPM / 20 RPD. The daily
    /// quota barely exceeds the setup probe, leaving 2 requests - one active hour.
    #[test]
    fn a_twenty_a_day_free_tier_is_insufficient() {
        let a = assess(5, 20);
        assert_eq!(a.verdict, CapacityVerdict::Insufficient);
        assert_eq!(a.covered_active_hours, Some(2));
    }

    /// A daily quota at or under the probe cost cannot be measured at all, and the
    /// production gate refuses an unmeasured endpoint - so this is unusable, not slow.
    #[test]
    fn a_quota_smaller_than_the_probe_cannot_onboard() {
        assert_eq!(assess(5, 16).verdict, CapacityVerdict::CannotOnboard);
        assert_eq!(assess(5, 10).verdict, CapacityVerdict::CannotOnboard);
        // One request clear of the probe is survivable-in-principle, so it must NOT
        // claim the endpoint cannot be set up - the honest verdict is "not enough".
        assert_eq!(assess(5, 17).verdict, CapacityVerdict::Insufficient);
    }

    /// The other real free tier tested against: 15 RPM / 500 RPD comfortably covers
    /// a working day, and must not nag.
    #[test]
    fn a_five_hundred_a_day_free_tier_is_sufficient() {
        let a = assess(15, 500);
        assert_eq!(a.verdict, CapacityVerdict::Sufficient);
        assert!(a.covered_active_hours.unwrap() >= ASSUMED_ACTIVE_HOURS);
    }

    /// Blank fields are the settings default. A user who does not know their key's
    /// limits must get "we cannot tell", never a warning about a key that may be fine.
    #[test]
    fn unknown_limits_never_raise_a_false_alarm() {
        let a = assess(0, 0);
        assert_eq!(a.verdict, CapacityVerdict::Unknown);
        assert_eq!(a.covered_active_hours, None);
        assert_eq!(a.probe_seconds_at_rpm, None);
        // RPM alone still cannot judge daily capacity.
        assert_eq!(assess(40, 0).verdict, CapacityVerdict::Unknown);
    }

    /// RPM affects how long setup TAKES, never whether the key is adequate - pacing
    /// handles it. A 40-RPM key clears the probe burst outright, so there is nothing
    /// to report.
    #[test]
    fn low_rpm_reports_probe_time_without_changing_the_verdict() {
        let slow = assess(5, 500);
        let fast = assess(40, 500);
        assert_eq!(slow.verdict, fast.verdict);
        assert_eq!(slow.probe_seconds_at_rpm, Some(180)); // 15 gaps at 12s
        assert_eq!(fast.probe_seconds_at_rpm, None);
    }

    /// The boundary the copy hangs on: exactly a full working day is sufficient, one
    /// request short of it is not.
    #[test]
    fn the_sufficiency_boundary_is_a_full_working_day() {
        let need = PROBE_MAX_REQUESTS + CALLS_PER_ACTIVE_HOUR * ASSUMED_ACTIVE_HOURS;
        assert_eq!(assess(0, need).verdict, CapacityVerdict::Sufficient);
        assert_eq!(
            assess(0, need - CALLS_PER_ACTIVE_HOUR).verdict,
            CapacityVerdict::Tight
        );
    }
}
