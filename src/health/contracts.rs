//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Cross-process config contracts: values multiple processes must agree on, where
// a silent mismatch masquerades as an AI/runtime fault. Verified against the
// code: the UI reads MERIDIAN_DB_PATH (not MERIDIAN_DB), and the daemon reads
// <repo>/settings.json (not the UI's ~/.meridian/settings.json).

use crate::config::Config;
use crate::health::platform::repo_root;
use crate::health::Check;
use std::path::PathBuf;

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn expand(p: &str) -> String {
    p.strip_prefix("~/")
        .map(|rest| home().join(rest).display().to_string())
        .unwrap_or_else(|| p.to_string())
}

pub fn checks(cfg: &Config) -> Vec<Check> {
    vec![db_path_contract(cfg), settings_contract(), dead_poll_env()]
}

/// C1 — the daemon/MCP/MLX open MERIDIAN_DB; the UI opens MERIDIAN_DB_PATH. If
/// they diverge the dashboard silently reads a different (stale/empty) database.
fn db_path_contract(cfg: &Config) -> Check {
    let daemon_db = cfg.meridian_db.clone(); // already tilde-expanded by Config
    let ui_db = expand(
        &std::env::var("MERIDIAN_DB_PATH")
            .unwrap_or_else(|_| "~/.meridian/meridian.db".to_string()),
    );
    if daemon_db == ui_db {
        Check::ok("db path", "config", "daemon + UI agree")
    } else {
        Check::warn(
            "db path",
            "config",
            format!("daemon writes {daemon_db}; UI reads {ui_db}"),
        )
        .with_remedy("set MERIDIAN_DB_PATH (UI) equal to MERIDIAN_DB, or unset both")
    }
}

/// C7 — the daemon reads `<repo>/settings.json`; the dashboard writes
/// `~/.meridian/settings.json`. If only the latter exists, toggles in the UI
/// never reach the daemon.
fn settings_contract() -> Check {
    let daemon_exists = repo_root()
        .map(|r| r.join("settings.json").is_file())
        .unwrap_or(false);
    let ui_settings = home().join(".meridian/settings.json");
    if ui_settings.is_file() && !daemon_exists {
        Check::warn(
            "settings file",
            "config",
            "~/.meridian/settings.json is not read by the daemon",
        )
        .with_remedy("the daemon reads <repo>/settings.json — align them")
    } else {
        Check::ok("settings file", "config", "no split-brain")
    }
}

/// Dead env var: POLL_INTERVAL_SECS is documented but ignored (the daemon takes
/// the interval from settings.json). Flag if someone set it expecting an effect.
fn dead_poll_env() -> Check {
    if std::env::var("POLL_INTERVAL_SECS").is_ok() {
        Check::info(
            "poll interval",
            "config",
            "POLL_INTERVAL_SECS is set but ignored — the daemon uses settings.json",
        )
    } else {
        Check::ok("poll interval", "config", "from settings.json")
    }
}

#[cfg(test)]
mod tests {
    use super::expand;

    #[test]
    fn expand_handles_tilde_and_absolute() {
        assert!(expand("~/x").ends_with("/x"));
        assert!(!expand("~/x").starts_with('~'));
        assert_eq!(expand("/abs/path"), "/abs/path");
    }
}
