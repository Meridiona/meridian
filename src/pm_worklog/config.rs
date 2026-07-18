//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Env-driven config for the pm-worklog stage — routing/readiness/post thresholds.

/// Tunables for one pm-worklog run/cycle.
#[derive(Debug, Clone)]
pub struct PmWorklogConfig {
    /// Hours between scheduled driver passes (informational — the driver also
    /// runs on the daemon poll tick).
    pub interval_hours: f64,

    /// Routing thresholds.
    pub min_confidence: f64,
    pub min_coverage: f64,

    /// A session that has been waiting longer than this many minutes for an
    /// upstream stage is treated as "settled" for readiness, so one stuck row
    /// can never deadlock an hour (the aging escape).
    pub readiness_aging_minutes: i64,

    /// Jira's hard minimum — worklogs below this many real seconds are not
    /// posted (Jira rejects < 60s).
    pub min_post_seconds: i64,
}

fn env_parse<T: std::str::FromStr>(key: &str, default: T) -> T {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

impl PmWorklogConfig {
    pub fn from_env() -> Self {
        Self {
            interval_hours: env_parse("PM_WORKLOG_INTERVAL_HOURS", 1.0),
            min_confidence: env_parse("PM_WORKLOG_MIN_CONFIDENCE", 0.65),
            min_coverage: env_parse("PM_WORKLOG_MIN_COVERAGE", 0.80),
            readiness_aging_minutes: env_parse("PM_WORKLOG_READINESS_AGING_MIN", 90),
            min_post_seconds: env_parse("PM_WORKLOG_MIN_POST_SECONDS", 60),
        }
    }
}

impl Default for PmWorklogConfig {
    fn default() -> Self {
        Self::from_env()
    }
}
