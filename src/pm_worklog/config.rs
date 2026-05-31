// meridian — normalises screenpipe activity into structured app sessions
//
// Env-driven config for the pm-worklog stage. Defaults mirror the Python
// `pm_worklog_update/config.py`; the cadence + readiness knobs are new for the
// in-daemon hourly driver.

/// Tunables for one pm-worklog run/cycle.
#[derive(Debug, Clone)]
pub struct PmWorklogConfig {
    /// MLX server host/port for the `/synthesise_worklog` endpoint.
    pub mlx_host: String,
    pub mlx_port: u16,
    /// HTTP timeout for one synth call (the agno agent can take ~60s).
    pub synth_timeout_s: u64,

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

    /// Master safety switch for the daemon driver. Default `false`: the driver
    /// runs in dry-run (drafts worklog rows but never POSTs to real Jira) until
    /// a human flips `PM_WORKLOG_POST_ENABLED=true`. The CLI `--dry-run` flag is
    /// independent of this.
    pub post_enabled: bool,
}

fn env_str(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
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
            mlx_host: env_str("MLX_SERVER_HOST", "127.0.0.1"),
            mlx_port: env_parse("MLX_SERVER_PORT", 7823),
            synth_timeout_s: env_parse("PM_WORKLOG_SYNTH_TIMEOUT_S", 300),
            interval_hours: env_parse("PM_WORKLOG_INTERVAL_HOURS", 1.0),
            min_confidence: env_parse("PM_WORKLOG_MIN_CONFIDENCE", 0.65),
            min_coverage: env_parse("PM_WORKLOG_MIN_COVERAGE", 0.80),
            readiness_aging_minutes: env_parse("PM_WORKLOG_READINESS_AGING_MIN", 90),
            min_post_seconds: env_parse("PM_WORKLOG_MIN_POST_SECONDS", 60),
            post_enabled: std::env::var("PM_WORKLOG_POST_ENABLED")
                .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                .unwrap_or(false),
        }
    }
}

impl Default for PmWorklogConfig {
    fn default() -> Self {
        Self::from_env()
    }
}
