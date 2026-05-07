// meridian — normalises screenpipe activity into structured app sessions
// https://github.com/meridiona/meridian

use std::path::PathBuf;

pub struct Config {
    pub screenpipe_db: String,
    pub meridian_db: String,
    pub poll_interval_secs: u64,
}

/// Expand a leading `~` to the current user's home directory.
fn expand_tilde(path: &str) -> String {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return format!("{}/{}", home, rest);
        }
    }
    path.to_owned()
}

impl Config {
    /// Build config from environment variables, falling back to defaults.
    ///
    /// Variables read:
    ///   SCREENPIPE_DB       — path to screenpipe's SQLite file
    ///   MERIDIAN_DB         — path to meridian's SQLite file
    ///   POLL_INTERVAL_SECS  — poll cadence in seconds (u64)
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
        }
    }

    /// Returns an `sqlite://` URI suitable for sqlx, pointing at the screenpipe DB.
    pub fn screenpipe_db_uri(&self) -> String {
        format!("sqlite://{}", self.screenpipe_db)
    }

    /// Returns an `sqlite://` URI suitable for sqlx, pointing at the meridian DB.
    /// Creates the parent directory if it does not already exist.
    pub fn meridian_db_uri(&self) -> String {
        if let Some(parent) = PathBuf::from(&self.meridian_db).parent() {
            if !parent.as_os_str().is_empty() && !parent.exists() {
                std::fs::create_dir_all(parent).ok();
            }
        }
        format!("sqlite://{}", self.meridian_db)
    }
}

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
