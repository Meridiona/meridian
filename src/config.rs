//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
// https://github.com/meridiona/meridian

use std::path::PathBuf;

// ---------------------------------------------------------------------------
// RuntimeSettings — hot-reloadable subset of config (~/.meridian/settings.json)
// ---------------------------------------------------------------------------

/// Settings that can be changed at runtime by editing `settings.json` (resolved by
/// [`settings_json_path`]). The daemon re-reads this file on every poll tick; no
/// restart required.
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
    // OpenObserve OTLP export — all three must be non-empty for export to activate.
    // Takes precedence over MERIDIAN_OTLP_ENDPOINT / MERIDIAN_OO_AUTH env vars.
    pub otlp_enabled: bool,
    pub otlp_endpoint: Option<String>,
    pub oo_email: Option<String>,
    pub oo_password: Option<String>,
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
            otlp_enabled: true,
            otlp_endpoint: None,
            oo_email: None,
            oo_password: None,
        }
    }
}

/// Resolve the path to `settings.json` — the hot-reloadable runtime settings the
/// UI writes and the daemon reads. Both sides MUST agree on this path or the UI's
/// "Apply" never reaches the daemon.
///
/// The daemon's working directory depends on the install type (repo root under
/// `cargo run`, `~/.meridian/app` for a bundle install), so resolving relative to
/// cwd makes the UI and daemon disagree on anything but a source checkout. Instead
/// we resolve to a fixed, install-independent location. Resolution order:
///   1. `MERIDIAN_SETTINGS_PATH` — explicit override (tests, non-standard installs)
///   2. `~/.meridian/settings.json` — canonical home, next to `meridian.db`; this
///      is what the UI writes (see `ui/lib/settings.ts`)
///   3. `<cwd>/settings.json` — legacy fallback, honoured only when the canonical
///      file is absent (a source checkout still keeping settings in the repo). The
///      canonical path takes precedence so a UI write always wins once it exists.
fn settings_json_path() -> std::path::PathBuf {
    if let Ok(p) = std::env::var("MERIDIAN_SETTINGS_PATH") {
        if !p.is_empty() {
            return PathBuf::from(shellexpand::tilde(&p).into_owned());
        }
    }

    let canonical = std::env::var("HOME")
        .ok()
        .map(|home| PathBuf::from(home).join(".meridian").join("settings.json"));

    if let Some(path) = &canonical {
        if path.exists() {
            return path.clone();
        }
    }

    // Legacy: a source checkout may still carry settings.json in the repo root
    // (the daemon's cwd). Honoured only when the canonical file does not exist.
    let cwd_path = std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("settings.json");
    if cwd_path.exists() {
        return cwd_path;
    }

    // Neither exists yet — default to the canonical path so a first write lands
    // in the right, install-independent place.
    canonical.unwrap_or(cwd_path)
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
    /// e.g. "https://acme.atlassian.net". Empty in OAuth-only setups (the site
    /// URL then comes from the OAuth token store's `accessible-resources` result).
    pub base_url: String,
    /// Basic-auth email; empty under OAuth.
    pub email: String,
    /// Basic-auth API token; empty under OAuth.
    pub api_token: String,
    /// Filter to specific project keys. Empty = accept all.
    pub project_keys: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct GitHubConfig {
    /// Personal access token or gh CLI OAuth token with `repo`, `read:org`, `project` scopes.
    pub token: String,
    /// GitHub Projects v2 node IDs (PVT_xxx). Empty → no tasks synced.
    pub project_ids: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct LinearConfig {
    pub api_key: String,
    /// Linear team IDs to filter by. Empty = all teams.
    pub team_ids: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct TrelloConfig {
    /// Meridian's Trello Power-Up app key (baked in or TRELLO_APP_KEY override).
    pub app_key: String,
    /// Board IDs to limit card fetches. Empty = all boards the user's cards span.
    pub board_ids: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct AzureDevOpsConfig {
    /// Personal access token (PAT). Auth: Basic base64(":token").
    pub pat: String,
    /// Resolved API root. Supports all three URL shapes:
    ///   https://dev.azure.com/{org}           (cloud, standard)
    ///   https://{org}.visualstudio.com        (cloud, legacy)
    ///   https://tfs.company.com/{collection}  (on-premises)
    /// Set AZURE_DEVOPS_ORG_URL for legacy/on-prem; AZURE_DEVOPS_ORG for cloud.
    pub api_base: String,
    /// Project name or ID — scopes all work item queries.
    pub project: String,
}

/// Provider-agnostic credential variant. Add new providers here; callers
/// match on this enum, so the compiler enforces exhaustive handling.
#[derive(Clone, Debug)]
pub enum PmProviderConfig {
    Jira(JiraConfig),
    GitHub(GitHubConfig),
    Linear(LinearConfig),
    Trello(TrelloConfig),
    AzureDevOps(AzureDevOpsConfig),
}

impl PmProviderConfig {
    pub fn provider_name(&self) -> &'static str {
        match self {
            Self::Jira(_) => "jira",
            Self::GitHub(_) => "github",
            Self::Linear(_) => "linear",
            Self::Trello(_) => "trello",
            Self::AzureDevOps(_) => "azure_devops",
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
        .unwrap_or_default()
        .trim()
        .to_owned();
    let email = std::env::var("JIRA_EMAIL")
        .unwrap_or_default()
        .trim()
        .to_owned();
    let api_token = std::env::var("JIRA_API_TOKEN")
        .unwrap_or_default()
        .trim()
        .to_owned();

    // Configured if EITHER auth path is viable:
    //   * browser OAuth — the user has run `meridian oauth-login jira`, so a token
    //     store exists (client id is baked in / env-overridable, not required here);
    //   * static basic auth — all three legacy vars present.
    // The store check is what makes zero-config OAuth work with no env at all.
    let basic_complete = !base_url.is_empty() && !email.is_empty() && !api_token.is_empty();
    let oauth_active = crate::intelligence::oauth::store::exists("jira");
    if !basic_complete && !oauth_active {
        return None;
    }

    Some(PmProviderConfig::Jira(JiraConfig {
        base_url,
        email,
        api_token,
        project_keys: env_list("JIRA_PROJECT_KEYS"),
    }))
}

fn parse_github() -> Option<PmProviderConfig> {
    let token = std::env::var("GITHUB_TOKEN").ok()?;
    Some(PmProviderConfig::GitHub(GitHubConfig {
        token,
        project_ids: env_list("GITHUB_PROJECT_IDS"),
    }))
}

fn parse_linear() -> Option<PmProviderConfig> {
    let api_key = std::env::var("LINEAR_API_KEY").ok()?;
    Some(PmProviderConfig::Linear(LinearConfig {
        api_key,
        team_ids: env_list("LINEAR_TEAM_IDS"),
    }))
}

fn parse_trello() -> Option<PmProviderConfig> {
    if !crate::intelligence::oauth::store::exists("trello") {
        return None;
    }
    let app_key = std::env::var("TRELLO_APP_KEY")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(crate::intelligence::oauth::trello::app_key);
    Some(PmProviderConfig::Trello(TrelloConfig {
        app_key,
        board_ids: env_list("TRELLO_BOARD_IDS"),
    }))
}

/// Split a project URL into (api_base, project).
///
/// Last path segment = project; everything before (scheme + host + any preceding
/// segments) = api_base. Works for all three URL shapes:
///
///   https://dev.azure.com/org/project      → ("https://dev.azure.com/org", "project")
///   https://org.visualstudio.com/project   → ("https://org.visualstudio.com", "project")
///   https://tfs.corp.com/Coll/project      → ("https://tfs.corp.com/Coll", "project")
pub fn split_azure_devops_url(url: &str) -> Option<(String, String)> {
    let url = url.trim().trim_end_matches('/');
    let path_start = url.find("://").map(|i| i + 3).unwrap_or(0);
    let host_end = url[path_start..]
        .find('/')
        .map(|i| path_start + i)
        .unwrap_or(url.len());
    let host = &url[..host_end];
    let path = url[host_end..].trim_matches('/');
    if path.is_empty() {
        return None;
    }
    let (api_base, project) = if let Some((before, proj)) = path.rsplit_once('/') {
        // Multi-segment path (dev.azure.com or on-prem).
        let base = if before.is_empty() {
            host.to_owned()
        } else {
            format!("{}/{}", host, before.trim_matches('/'))
        };
        (base, proj.to_owned())
    } else {
        // Single-segment path (visualstudio.com): host alone is the api_base.
        (host.to_owned(), path.to_owned())
    };
    if project.is_empty() {
        return None;
    }
    Some((api_base, project))
}

fn parse_azure_devops() -> Option<PmProviderConfig> {
    let pat = std::env::var("AZURE_DEVOPS_PAT").ok()?;
    if pat.trim().is_empty() {
        return None;
    }

    // Primary: AZURE_DEVOPS_URL — user pastes the project URL from the browser.
    // Fallback: legacy three-variable form (AZURE_DEVOPS_ORG / AZURE_DEVOPS_ORG_URL
    // + AZURE_DEVOPS_PROJECT) for users who migrated from the earlier config.
    let (api_base, project) = if let Ok(url) = std::env::var("AZURE_DEVOPS_URL") {
        split_azure_devops_url(url.trim())?
    } else if let Ok(url) = std::env::var("AZURE_DEVOPS_ORG_URL") {
        let base = url.trim().trim_end_matches('/').to_owned();
        let project = std::env::var("AZURE_DEVOPS_PROJECT").ok()?;
        let project = project.trim().to_owned();
        if project.is_empty() {
            return None;
        }
        (base, project)
    } else {
        let org = std::env::var("AZURE_DEVOPS_ORG").ok()?;
        let org = org.trim();
        if org.is_empty() {
            return None;
        }
        let base = if org.contains("://") {
            org.trim_end_matches('/').to_owned()
        } else if org.contains(".visualstudio.com") {
            format!("https://{}", org.trim_end_matches('/'))
        } else {
            format!("https://dev.azure.com/{org}")
        };
        let project = std::env::var("AZURE_DEVOPS_PROJECT").ok()?;
        let project = project.trim().to_owned();
        if project.is_empty() {
            return None;
        }
        (base, project)
    };

    Some(PmProviderConfig::AzureDevOps(AzureDevOpsConfig {
        pat,
        api_base,
        project,
    }))
}

fn parse_providers() -> Vec<PmProviderConfig> {
    [
        parse_jira(),
        parse_github(),
        parse_linear(),
        parse_trello(),
        parse_azure_devops(),
    ]
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
    /// GitHub provider (required):
    ///   GITHUB_TOKEN
    ///   GITHUB_PROJECT_IDS  — optional comma-separated Projects v2 node IDs (PVT_xxx)
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

        // Jira is "configured" for update-gating purposes under either auth path:
        // legacy basic auth (JIRA_BASE_URL), browser OAuth env var (JIRA_OAUTH_CLIENT_ID),
        // or OAuth token store (zero-config path).
        let jira_configured = std::env::var("JIRA_BASE_URL").is_ok()
            || std::env::var("JIRA_OAUTH_CLIENT_ID").is_ok()
            || crate::intelligence::oauth::store::exists("jira");
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

        let pm_providers = parse_providers();
        let provider_names: Vec<&str> = pm_providers.iter().map(|p| p.provider_name()).collect();

        tracing::info!(
            screenpipe_db = %screenpipe_db,
            meridian_db = %meridian_db,
            poll_interval_secs,
            classification_enabled,
            pm_providers = ?provider_names,
            "config loaded"
        );

        Self {
            screenpipe_db,
            meridian_db,
            poll_interval_secs,
            pm_providers,
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
    fn test_parse_github_reads_token_and_project_ids() {
        let _guard = env_lock().lock().unwrap();
        std::env::set_var("GITHUB_TOKEN", "gho_testtoken");
        std::env::set_var("GITHUB_PROJECT_IDS", "PVT_aaa, PVT_bbb");
        let parsed = parse_github();
        assert!(matches!(parsed, Some(PmProviderConfig::GitHub(_))));
        if let Some(PmProviderConfig::GitHub(gh)) = parsed {
            assert_eq!(gh.token, "gho_testtoken");
            // env_list splits on commas and trims surrounding whitespace.
            assert_eq!(gh.project_ids, vec!["PVT_aaa", "PVT_bbb"]);
        }
        std::env::remove_var("GITHUB_TOKEN");
        std::env::remove_var("GITHUB_PROJECT_IDS");
    }

    #[test]
    fn test_parse_github_requires_token() {
        let _guard = env_lock().lock().unwrap();
        std::env::remove_var("GITHUB_TOKEN");
        std::env::set_var("GITHUB_PROJECT_IDS", "PVT_aaa");
        assert!(
            parse_github().is_none(),
            "no GITHUB_TOKEN → no GitHub provider"
        );
        std::env::remove_var("GITHUB_PROJECT_IDS");
    }

    #[test]
    fn test_parse_github_token_only_empty_project_ids() {
        let _guard = env_lock().lock().unwrap();
        std::env::set_var("GITHUB_TOKEN", "gho_x");
        std::env::remove_var("GITHUB_PROJECT_IDS");
        let parsed = parse_github();
        assert!(matches!(parsed, Some(PmProviderConfig::GitHub(_))));
        if let Some(PmProviderConfig::GitHub(gh)) = parsed {
            // Unset GITHUB_PROJECT_IDS → empty vec; refresh_if_stale skips the sync.
            assert!(gh.project_ids.is_empty());
        }
        std::env::remove_var("GITHUB_TOKEN");
    }

    /// Clear every Jira-related env var and isolate HOME to a clean temp dir so a
    /// real `~/.meridian/oauth/jira.json` on the dev machine can't make parse_jira
    /// see an OAuth login the test never set up. Returns the isolated HOME.
    fn clear_jira_env() -> std::path::PathBuf {
        for k in [
            "JIRA_URL",
            "JIRA_BASE_URL",
            "JIRA_EMAIL",
            "JIRA_API_TOKEN",
            "JIRA_OAUTH_CLIENT_ID",
            "JIRA_PROJECT_KEYS",
        ] {
            std::env::remove_var(k);
        }
        let home = std::env::temp_dir().join(format!("merid_jira_cfg_{}", std::process::id()));
        std::fs::remove_dir_all(&home).ok();
        std::fs::create_dir_all(&home).ok();
        std::env::set_var("HOME", &home);
        home
    }

    #[test]
    fn test_parse_jira_basic_auth_complete() {
        let _guard = env_lock().lock().unwrap();
        let home = clear_jira_env();
        std::env::set_var("JIRA_BASE_URL", "https://acme.atlassian.net");
        std::env::set_var("JIRA_EMAIL", "a@b.com");
        std::env::set_var("JIRA_API_TOKEN", "tok");
        let parsed = parse_jira();
        assert!(matches!(parsed, Some(PmProviderConfig::Jira(_))));
        if let Some(PmProviderConfig::Jira(j)) = parsed {
            assert_eq!(j.base_url, "https://acme.atlassian.net");
            assert_eq!(j.api_token, "tok");
        }
        std::fs::remove_dir_all(&home).ok();
        clear_jira_env();
    }

    #[test]
    fn test_parse_jira_oauth_store_configures() {
        let _guard = env_lock().lock().unwrap();
        let home = clear_jira_env();
        // No basic creds at all — a present OAuth token store alone configures Jira.
        crate::intelligence::oauth::store::save(&crate::intelligence::oauth::store::OAuthTokens {
            provider: "jira".into(),
            client_id: "cid".into(),
            access_token: "a".into(),
            refresh_token: "r".into(),
            expires_at: 9_999_999_999,
            scopes: String::new(),
            cloud_id: "c".into(),
            site_url: "https://acme.atlassian.net".into(),
        })
        .unwrap();
        assert!(
            matches!(parse_jira(), Some(PmProviderConfig::Jira(_))),
            "an OAuth token store alone must yield a Jira provider"
        );
        std::fs::remove_dir_all(&home).ok();
        clear_jira_env();
    }

    #[test]
    fn test_parse_jira_incomplete_basic_is_none() {
        let _guard = env_lock().lock().unwrap();
        let home = clear_jira_env();
        // base_url + email but NO token, and no OAuth login → not configured.
        std::env::set_var("JIRA_BASE_URL", "https://acme.atlassian.net");
        std::env::set_var("JIRA_EMAIL", "a@b.com");
        assert!(
            parse_jira().is_none(),
            "incomplete basic creds with no OAuth login must yield no provider"
        );
        std::fs::remove_dir_all(&home).ok();
        clear_jira_env();
    }

    #[test]
    fn test_parse_jira_nothing_configured_is_none() {
        let _guard = env_lock().lock().unwrap();
        let home = clear_jira_env();
        // No creds, no token store → no Jira provider (no per-tick auth-fail spam).
        assert!(parse_jira().is_none());
        std::fs::remove_dir_all(&home).ok();
        clear_jira_env();
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

    #[test]
    fn test_split_azure_devops_url_cloud_standard() {
        let (base, project) =
            split_azure_devops_url("https://dev.azure.com/mycompany/MyProject").unwrap();
        assert_eq!(base, "https://dev.azure.com/mycompany");
        assert_eq!(project, "MyProject");
    }

    #[test]
    fn test_split_azure_devops_url_visualstudio() {
        let (base, project) =
            split_azure_devops_url("https://mycompany.visualstudio.com/MyProject").unwrap();
        assert_eq!(base, "https://mycompany.visualstudio.com");
        assert_eq!(project, "MyProject");
    }

    #[test]
    fn test_split_azure_devops_url_on_premises() {
        let (base, project) =
            split_azure_devops_url("https://tfs.corp.com/DefaultCollection/MyProject").unwrap();
        assert_eq!(base, "https://tfs.corp.com/DefaultCollection");
        assert_eq!(project, "MyProject");
    }

    #[test]
    fn test_split_azure_devops_url_trailing_slash() {
        let (base, project) =
            split_azure_devops_url("https://dev.azure.com/mycompany/MyProject/").unwrap();
        assert_eq!(base, "https://dev.azure.com/mycompany");
        assert_eq!(project, "MyProject");
    }

    #[test]
    fn test_split_azure_devops_url_no_path() {
        assert!(split_azure_devops_url("https://dev.azure.com").is_none());
    }
}
