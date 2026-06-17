//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Install-mode detection and the file paths that depend on it.
//!
//! The tray runs in three install shapes — a bundled `.app`, a source/dev
//! checkout, or a bare launch — and the credential `.env` + `meridian.db` live
//! in different places in each. This module is the single resolver for "where
//! does my data live"; it is NOT a set of Tauri commands (it's plumbing the
//! commands and startup consume).
//!
//! # Who calls this
//! - [`crate::commands::integrations`] — reads tracker keys from [`InstallMode::env_path`].
//! - `lib.rs` startup — opens the DB at [`meridian_db_path`].
//!
//! # Related
//! - [`crate::sys`] — other shared runtime helpers (uid, notify, ui_base).
//! - The daemon's own env layering (`~/.meridian/app/.env` on a bundle install)
//!   is mirrored by [`env_from_daemon_dotenv`].

/// Which install mode the tray is running in, inferred from the user's `.env` location.
///
/// - `Canonical`: `~/.meridian/.env` exists — user credentials, install-independent.
/// - `Dev`: no canonical env; a repo `.env` found by walking up from cwd (local dev / contributor).
/// - `Bare`: neither present — process-env overrides and hardcoded defaults only.
#[derive(Debug)]
pub(crate) enum InstallMode {
    Canonical(std::path::PathBuf),
    Dev(std::path::PathBuf),
    Bare,
}

impl InstallMode {
    pub(crate) fn env_path(&self) -> Option<&std::path::Path> {
        match self {
            Self::Canonical(p) | Self::Dev(p) => Some(p),
            Self::Bare => None,
        }
    }
}

/// Detect the install mode from the file system.
/// `~/.meridian/.env` is the canonical credential location for all install types —
/// install-independent, next to `meridian.db` and `settings.json`.
/// Falls back to a cwd walk for dev/contributor runs where no canonical env exists.
pub(crate) fn detect_install_mode() -> InstallMode {
    let home = std::env::var("HOME").ok().map(std::path::PathBuf::from);
    if let Some(p) = home.as_ref().map(|h| h.join(".meridian/.env")) {
        if p.exists() {
            return InstallMode::Canonical(p);
        }
    }
    if let Ok(mut dir) = std::env::current_dir() {
        for _ in 0..8 {
            let candidate = dir.join(".env");
            if candidate.exists() {
                return InstallMode::Dev(candidate);
            }
            if !dir.pop() {
                break;
            }
        }
    }
    InstallMode::Bare
}

/// Read `key` from a single line of a .env file, stripping surrounding quotes.
fn dotenv_line_value(line: &str, key: &str) -> Option<String> {
    let prefix = format!("{key}=");
    let t = line.trim();
    if t.starts_with('#') || !t.starts_with(prefix.as_str()) {
        return None;
    }
    let raw = t[prefix.len()..].trim();
    let v = raw.trim_matches('"').trim_matches('\'').trim();
    if v.is_empty() {
        None
    } else {
        Some(v.to_string())
    }
}

fn env_key_from_path(path: &std::path::Path, key: &str) -> Option<String> {
    let contents = std::fs::read_to_string(path).ok()?;
    contents.lines().find_map(|l| dotenv_line_value(l, key))
}

/// Resolve meridian.db path: process env first (launchd plist / shell export),
/// then the daemon's .env (keyed by install mode), then the hardcoded default.
/// Logs at `info!` so the install mode is visible in OpenObserve on every startup.
pub(crate) fn meridian_db_path() -> String {
    if let Ok(p) = std::env::var("MERIDIAN_DB") {
        tracing::info!(source = "process_env", path = %p, "meridian_db resolved");
        return p;
    }
    let mode = detect_install_mode();
    if let Some(env_file) = mode.env_path() {
        if let Some(p) = env_key_from_path(env_file, "MERIDIAN_DB") {
            tracing::info!(
                source = ?mode,
                env_file = %env_file.display(),
                path = %p,
                "meridian_db resolved"
            );
            return p;
        }
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let p = format!("{}/.meridian/meridian.db", home);
    tracing::info!(source = ?mode, path = %p, "meridian_db resolved (default)");
    p
}
