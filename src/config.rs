// meridian — normalises screenpipe activity into structured app sessions
// https://github.com/meridiona/meridian

use std::path::PathBuf;

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
    let base_url = std::env::var("JIRA_BASE_URL").ok()?;
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

        let poll_interval_secs = std::env::var("POLL_INTERVAL_SECS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(60);

        Self {
            screenpipe_db,
            meridian_db,
            poll_interval_secs,
            pm_providers: parse_providers(),
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

    #[test]
    fn test_default_paths_contain_expected_dirs() {
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
        std::env::set_var("SCREENPIPE_DB", "/custom/screenpipe.db");
        std::env::set_var("MERIDIAN_DB", "/custom/meridian.db");
        std::env::set_var("POLL_INTERVAL_SECS", "30");
        let cfg = Config::from_env();
        assert_eq!(cfg.screenpipe_db, "/custom/screenpipe.db");
        assert_eq!(cfg.meridian_db, "/custom/meridian.db");
        assert_eq!(cfg.poll_interval_secs, 30);
        std::env::remove_var("SCREENPIPE_DB");
        std::env::remove_var("MERIDIAN_DB");
        std::env::remove_var("POLL_INTERVAL_SECS");
    }

    #[test]
    fn test_tilde_expansion() {
        std::env::set_var("HOME", "/Users/testuser");
        std::env::set_var("SCREENPIPE_DB", "~/custom/db.sqlite");
        let cfg = Config::from_env();
        assert_eq!(cfg.screenpipe_db, "/Users/testuser/custom/db.sqlite");
        std::env::remove_var("HOME");
        std::env::remove_var("SCREENPIPE_DB");
    }

    #[test]
    fn test_uri_prefix() {
        std::env::set_var("SCREENPIPE_DB", "/some/path/db.sqlite");
        std::env::set_var("MERIDIAN_DB", "/other/meridian.db");
        let cfg = Config::from_env();
        assert!(cfg.screenpipe_db_uri().starts_with("sqlite://"));
        assert!(cfg.meridian_db_uri().starts_with("sqlite://"));
        std::env::remove_var("SCREENPIPE_DB");
        std::env::remove_var("MERIDIAN_DB");
    }

}
