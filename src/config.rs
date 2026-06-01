// meridian — normalises screenpipe activity into structured app sessions
// https://github.com/meridiona/meridian

use std::path::PathBuf;

// ---------------------------------------------------------------------------
// RuntimeSettings — hot-reloadable subset of config (repo-local settings.json)
// ---------------------------------------------------------------------------

/// Settings that can be changed at runtime by editing the repo-local `settings.json`.
/// The daemon re-reads this file on every poll tick; no restart required.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(default)]
pub struct RuntimeSettings {
    pub log_level: String,
    pub classification_enabled: bool,
    pub min_classification_duration_s: i64,
    pub classification_timeout_s: u64,
    pub agent_auto_floor: f64,
    pub agent_queue_floor: f64,
    pub llm_prefer_local: bool,
    pub llm_budget_pct: f64,
    pub poll_interval_secs: u64,
    pub jira_update_enabled: bool,
}

impl Default for RuntimeSettings {
    fn default() -> Self {
        Self {
            log_level: "INFO".to_string(),
            classification_enabled: true,
            min_classification_duration_s: 10,
            classification_timeout_s: 120,
            agent_auto_floor: 0.65,
            agent_queue_floor: 0.40,
            llm_prefer_local: true,
            llm_budget_pct: 0.5,
            poll_interval_secs: 60,
            jira_update_enabled: true,
        }
    }
}

/// Return the path to the repo-local `settings.json`, resolved against the
/// process working directory (the daemon runs with cwd = repo root). Kept inside
/// the repo so nothing is read from outside it; absent → built-in defaults.
fn settings_json_path() -> std::path::PathBuf {
    std::env::current_dir()
        .unwrap_or_else(|_| std::path::PathBuf::from("."))
        .join("settings.json")
}

/// Load the repo-local `settings.json`, falling back to defaults if the file is
/// absent or cannot be parsed.
pub fn load_runtime_settings() -> RuntimeSettings {
    let path = settings_json_path();
    match std::fs::read_to_string(&path) {
        Ok(s) => serde_json::from_str(&s).unwrap_or_else(|e| {
            tracing::warn!(
                path = %path.display(),
                error = %e,
                "settings.json parse error — using defaults"
            );
            RuntimeSettings::default()
        }),
        Err(_) => RuntimeSettings::default(), // file doesn't exist yet — that's fine
    }
}

// ---------------------------------------------------------------------------
// Per-provider credential structs
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct JiraConfig {
    /// e.g. "https://acme.atlassian.net"
    pub base_url: String,
    pub email: String,
    pub api_token: String,
    /// Filter to specific project keys. Empty = accept all.
    pub project_keys: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct GitHubConfig {
    /// Personal access token with `repo` scope.
    pub token: String,
    /// GitHub organisation slug.
    pub org: String,
    /// Optional list of "org/repo" slugs to restrict fetching. Empty = all org repos.
    pub repos: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct LinearConfig {
    pub api_key: String,
    /// Linear team IDs to filter by. Empty = all teams.
    pub team_ids: Vec<String>,
}

/// Provider-agnostic credential variant. Add new providers here; callers
/// match on this enum, so the compiler enforces exhaustive handling.
#[derive(Clone, Debug)]
pub enum PmProviderConfig {
    Jira(JiraConfig),
    GitHub(GitHubConfig),
    Linear(LinearConfig),
}

impl PmProviderConfig {
    pub fn provider_name(&self) -> &'static str {
        match self {
            Self::Jira(_) => "jira",
            Self::GitHub(_) => "github",
            Self::Linear(_) => "linear",
        }
    }
}

// ---------------------------------------------------------------------------
// Top-level config
// ---------------------------------------------------------------------------

pub struct Config {
    pub screenpipe_db: String,
    pub meridian_db: String,
    pub poll_interval_secs: u64,
    /// All configured PM providers. Empty = intelligence silently disabled.
    pub pm_providers: Vec<PmProviderConfig>,
    /// Whether to run MLX task classification after each ETL tick.
    /// CLASSIFICATION_ENABLED — default true
    pub classification_enabled: bool,
    /// Seconds to wait for the Python classification subprocess before killing it.
    /// CLASSIFICATION_TIMEOUT_S — default 120
    pub classification_timeout_s: u64,
    /// Minimum session duration in seconds to classify (shorter = overhead/skip).
    /// MIN_LLM_DURATION_S — default 10
    pub min_classification_duration_s: i64,
    /// Path to the meridian services/ directory containing agents/run_task_linker.py.
    /// MERIDIAN_SERVICES_DIR — optional, auto-detected if not set
    pub classification_services_dir: Option<String>,
    /// Whether to classify sessions that existed before the first run.
    /// CLASSIFICATION_BACKFILL — default false (skip historical sessions)
    pub classification_backfill: bool,
    /// Number of recent classified sessions included as temporal context in each prompt.
    /// CLASSIFICATION_CONTEXT_WINDOW — default 5
    pub classification_context_window: usize,
    /// Port the persistent MLX classifier server listens on.
    /// MLX_SERVER_PORT — default 7823
    pub mlx_server_port: u16,
    /// Whether to post Jira progress updates. Auto-enabled when JIRA_BASE_URL is set.
    /// JIRA_UPDATE_ENABLED — default true if Jira is configured
    pub jira_update_enabled: bool,
    /// Minimum seconds between Jira update runs.
    /// JIRA_UPDATE_INTERVAL_HOURS — default 4 (14400s)
    pub jira_update_interval_s: u64,
    /// Local hour at which the office day starts (inclusive). OFFICE_START_HOUR — default 9
    pub jira_office_start_hour: u32,
    /// Local hour at which the office day ends (exclusive). OFFICE_END_HOUR — default 17
    pub jira_office_end_hour: u32,
    /// Hot-reloadable runtime settings loaded from the repo-local `settings.json`.
    /// Values here take precedence over the equivalent env-var defaults.
    pub runtime: RuntimeSettings,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn expand_tilde(path: &str) -> String {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return format!("{}/{}", home, rest);
        }
    }
    path.to_owned()
}

fn env_list(key: &str) -> Vec<String> {
    std::env::var(key)
        .unwrap_or_default()
        .split(',')
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .collect()
}

// ---------------------------------------------------------------------------
// Provider parsers — each returns None if required vars are missing
// ---------------------------------------------------------------------------

fn parse_jira() -> Option<PmProviderConfig> {
    let base_url = std::env::var("JIRA_URL")
        .or_else(|_| std::env::var("JIRA_BASE_URL"))
        .ok()?;
    let email = std::env::var("JIRA_EMAIL").ok()?;
    let api_token = std::env::var("JIRA_API_TOKEN").ok()?;
    Some(PmProviderConfig::Jira(JiraConfig {
        base_url,
        email,
        api_token,
        project_keys: env_list("JIRA_PROJECT_KEYS"),
    }))
}

fn parse_github() -> Option<PmProviderConfig> {
    let token = std::env::var("GITHUB_TOKEN").ok()?;
    let org = std::env::var("GITHUB_ORG").ok()?;
    Some(PmProviderConfig::GitHub(GitHubConfig {
        token,
        org,
        repos: env_list("GITHUB_REPOS"),
    }))
}

fn parse_linear() -> Option<PmProviderConfig> {
    let api_key = std::env::var("LINEAR_API_KEY").ok()?;
    Some(PmProviderConfig::Linear(LinearConfig {
        api_key,
        team_ids: env_list("LINEAR_TEAM_IDS"),
    }))
}

fn parse_providers() -> Vec<PmProviderConfig> {
    [parse_jira(), parse_github(), parse_linear()]
        .into_iter()
        .flatten()
        .collect()
}

// ---------------------------------------------------------------------------
// Config::from_env
// ---------------------------------------------------------------------------

impl Config {
    /// Build config from environment variables, falling back to defaults.
    ///
    /// Core vars:
    ///   SCREENPIPE_DB       — path to screenpipe's SQLite file
    ///   MERIDIAN_DB         — path to meridian's SQLite file
    ///   POLL_INTERVAL_SECS  — poll cadence in seconds (default 60)
    ///
    /// Jira provider (all three required):
    ///   JIRA_BASE_URL, JIRA_EMAIL, JIRA_API_TOKEN
    ///   JIRA_PROJECT_KEYS   — optional comma-separated filter, e.g. "KAN,ENG"
    ///
    /// GitHub provider (all two required):
    ///   GITHUB_TOKEN, GITHUB_ORG
    ///   GITHUB_REPOS        — optional comma-separated filter, e.g. "org/api,org/web"
    ///
    /// Linear provider (required):
    ///   LINEAR_API_KEY
    ///   LINEAR_TEAM_IDS     — optional comma-separated filter
    pub fn from_env() -> Self {
        let screenpipe_db = std::env::var("SCREENPIPE_DB")
            .map(|v| expand_tilde(&v))
            .unwrap_or_else(|_| {
                let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_owned());
                format!("{}/.screenpipe/db.sqlite", home)
            });

        let meridian_db = std::env::var("MERIDIAN_DB")
            .map(|v| expand_tilde(&v))
            .unwrap_or_else(|_| {
                let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_owned());
                format!("{}/.meridian/meridian.db", home)
            });

        // settings.json is loaded first; its values take precedence over env-var
        // baselines for the fields it covers. If the file is absent the runtime
        // defaults equal the env-var defaults, so the merge is a no-op.
        let runtime = load_runtime_settings();

        // poll_interval_secs, classification_timeout_s, min_classification_duration_s
        // come entirely from settings.json (runtime). The equivalent env vars are
        // intentionally ignored when settings.json is present.
        let poll_interval_secs = runtime.poll_interval_secs;
        let classification_timeout_s = runtime.classification_timeout_s;
        let min_classification_duration_s = runtime.min_classification_duration_s;

        // Boolean guards: env var can only further disable, never re-enable.
        let classification_enabled_env = std::env::var("CLASSIFICATION_ENABLED")
            .map(|v| !matches!(v.to_lowercase().trim(), "0" | "false" | "no" | "off"))
            .unwrap_or(true);
        let classification_enabled = runtime.classification_enabled && classification_enabled_env;

        let jira_configured = std::env::var("JIRA_BASE_URL").is_ok();
        let jira_update_enabled_env = std::env::var("JIRA_UPDATE_ENABLED")
            .map(|v| !matches!(v.to_lowercase().trim(), "0" | "false" | "no" | "off"))
            .unwrap_or(jira_configured);
        let jira_update_enabled = runtime.jira_update_enabled && jira_update_enabled_env;

        let classification_services_dir = std::env::var("MERIDIAN_SERVICES_DIR")
            .ok()
            .map(|v| expand_tilde(&v));

        let classification_backfill = std::env::var("CLASSIFICATION_BACKFILL")
            .map(|v| matches!(v.to_lowercase().trim(), "1" | "true" | "yes" | "on"))
            .unwrap_or(false);

        let jira_update_interval_s = std::env::var("JIRA_UPDATE_INTERVAL_HOURS")
            .ok()
            .and_then(|v| v.parse::<f64>().ok())
            .map(|h| (h * 3600.0) as u64)
            .unwrap_or(14400); // default 4 hours

        let jira_office_start_hour = std::env::var("OFFICE_START_HOUR")
            .ok()
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(9);

        let jira_office_end_hour = std::env::var("OFFICE_END_HOUR")
            .ok()
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(17);

        let classification_context_window = std::env::var("CLASSIFICATION_CONTEXT_WINDOW")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(5);

        let mlx_server_port = std::env::var("MLX_SERVER_PORT")
            .ok()
            .and_then(|v| v.parse::<u16>().ok())
            .unwrap_or(7823);

        Self {
            screenpipe_db,
            meridian_db,
            poll_interval_secs,
            pm_providers: parse_providers(),
            classification_enabled,
            classification_timeout_s,
            min_classification_duration_s,
            classification_services_dir,
            classification_backfill,
            classification_context_window,
            mlx_server_port,
            jira_update_enabled,
            jira_update_interval_s,
            jira_office_start_hour,
            jira_office_end_hour,
            runtime,
        }
    }

    pub fn screenpipe_db_uri(&self) -> String {
        format!("sqlite://{}", self.screenpipe_db)
    }

    pub fn meridian_db_uri(&self) -> String {
        if let Some(parent) = PathBuf::from(&self.meridian_db).parent() {
            if !parent.as_os_str().is_empty() && !parent.exists() {
                std::fs::create_dir_all(parent).ok();
            }
        }
        format!("sqlite://{}", self.meridian_db)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    // Env vars are process-global — serialize all config tests to prevent races.
    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    #[test]
    fn test_default_paths_contain_expected_dirs() {
        let _guard = env_lock().lock().unwrap();
        std::env::remove_var("SCREENPIPE_DB");
        std::env::remove_var("MERIDIAN_DB");
        std::env::set_var("HOME", "/tmp/test_home");
        let cfg = Config::from_env();
        assert!(cfg.screenpipe_db.contains(".screenpipe"));
        assert!(cfg.meridian_db.contains(".meridian"));
        std::env::remove_var("HOME");
    }

    #[test]
    fn test_env_overrides() {
        let _guard = env_lock().lock().unwrap();
        std::env::set_var("SCREENPIPE_DB", "/custom/screenpipe.db");
        std::env::set_var("MERIDIAN_DB", "/custom/meridian.db");
        let cfg = Config::from_env();
        assert_eq!(cfg.screenpipe_db, "/custom/screenpipe.db");
        assert_eq!(cfg.meridian_db, "/custom/meridian.db");
        // poll_interval_secs is driven by settings.json (runtime), not POLL_INTERVAL_SECS env var.
        // Default runtime value is 60 when settings.json is absent.
        assert_eq!(cfg.runtime.poll_interval_secs, cfg.poll_interval_secs);
        std::env::remove_var("SCREENPIPE_DB");
        std::env::remove_var("MERIDIAN_DB");
    }

    #[test]
    fn test_runtime_settings_defaults() {
        let rt = RuntimeSettings::default();
        assert_eq!(rt.poll_interval_secs, 60);
        assert_eq!(rt.classification_timeout_s, 120);
        assert_eq!(rt.min_classification_duration_s, 10);
        assert!(rt.classification_enabled);
        assert!(rt.jira_update_enabled);
    }

    #[test]
    fn test_tilde_expansion() {
        let _guard = env_lock().lock().unwrap();
        std::env::set_var("HOME", "/Users/testuser");
        std::env::set_var("SCREENPIPE_DB", "~/custom/db.sqlite");
        let cfg = Config::from_env();
        assert_eq!(cfg.screenpipe_db, "/Users/testuser/custom/db.sqlite");
        std::env::remove_var("HOME");
        std::env::remove_var("SCREENPIPE_DB");
    }

    #[test]
    fn test_uri_prefix() {
        let _guard = env_lock().lock().unwrap();
        std::env::set_var("SCREENPIPE_DB", "/some/path/db.sqlite");
        std::env::set_var("MERIDIAN_DB", "/other/meridian.db");
        let cfg = Config::from_env();
        assert!(cfg.screenpipe_db_uri().starts_with("sqlite://"));
        assert!(cfg.meridian_db_uri().starts_with("sqlite://"));
        std::env::remove_var("SCREENPIPE_DB");
        std::env::remove_var("MERIDIAN_DB");
    }
}
