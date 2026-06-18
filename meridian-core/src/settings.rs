//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Runtime settings (`~/.meridian/settings.json`) — the hot-reloadable subset
//! of config the dashboard writes and the daemon reads each poll tick.
//!
//! # What this is
//! The single source of truth for the settings schema + path. Lives here (not
//! the daemon) so all three consumers share ONE definition: the daemon
//! re-exports it (its `config::{RuntimeSettings, load_runtime_settings}` are
//! unchanged), and the Tauri app reads it directly — no reimplementation.
//!
//! # Who calls this
//! - The daemon: `src/config.rs` re-exports these; `src/observability.rs` reads
//!   them each poll tick to drive log level + OTLP export.
//! - The Tauri app: the tray `get_settings` command (the ported `/api/settings`
//!   GET) serialises this, redacting `oo_password`.
//!
//! # Related
//! - `ui/lib/settings.ts` is the TS mirror of this schema + defaults — keep the
//!   two in sync (the `SETTINGS_DEFAULTS` there must match [`RuntimeSettings::default`]).
//! - The PUT (write) side of `/api/settings` is not yet ported.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Settings changeable at runtime by editing `settings.json`. The daemon
/// re-reads on every poll tick (no restart). `Serialize` is added (the daemon
/// only needs `Deserialize`) so the dashboard command can return it as JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
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
    // OpenObserve OTLP export — all three must be non-empty for export to
    // activate. settings.json is the ONLY credential source (MERIDIAN_OO_AUTH is
    // deprecated/ignored); otlp_endpoint still falls back to MERIDIAN_OTLP_ENDPOINT.
    pub otlp_enabled: bool,
    pub otlp_endpoint: Option<String>,
    pub oo_email: Option<String>,
    pub oo_password: Option<String>,
    // Notification preferences — the master switch + per-type toggles + quiet
    // hours. Read by [`crate::notifications`] (the policy ported from
    // ui/lib/notifications.ts) to decide whether an event may surface.
    // `quiet_hours_*` are 'HH:MM' local time (start inclusive, end exclusive).
    pub notifications_enabled: bool,
    pub notify_plan_nudge: bool,
    pub notify_worklog_ready: bool,
    pub notify_system_fault: bool,
    pub quiet_hours_enabled: bool,
    pub quiet_hours_start: String,
    pub quiet_hours_end: String,
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
            // OpenObserve export is opt-in — off until enabled in Settings (the UI
            // default in ui/lib/settings.ts must match this).
            otlp_enabled: false,
            otlp_endpoint: None,
            oo_email: None,
            oo_password: None,
            // Notifications on by default; quiet hours off (22:00–08:00 when
            // enabled). Must match SETTINGS_DEFAULTS in ui/lib/settings.ts.
            notifications_enabled: true,
            notify_plan_nudge: true,
            notify_worklog_ready: true,
            notify_system_fault: true,
            quiet_hours_enabled: false,
            quiet_hours_start: "22:00".to_string(),
            quiet_hours_end: "08:00".to_string(),
        }
    }
}

/// Expand a leading `~/` to `$HOME` (the only tilde form we accept — mirrors the
/// dashboard's settings.ts). Avoids pulling shellexpand into the lean crate.
fn expand_tilde(p: &str) -> PathBuf {
    if let Some(rest) = p.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    PathBuf::from(p)
}

/// Resolve `settings.json` — must agree with the dashboard (`ui/lib/settings.ts`)
/// or "Apply" never reaches the daemon. Resolution order:
///   1. `MERIDIAN_SETTINGS_PATH` override (tests / non-standard installs)
///   2. `~/.meridian/settings.json` — canonical home, next to `meridian.db`
///   3. `<cwd>/settings.json` — legacy fallback, only when the canonical is absent
///
/// cwd-relative resolution is avoided for the canonical path because the daemon's
/// working directory varies by install type (repo root vs `~/.meridian/app`).
pub fn settings_json_path() -> PathBuf {
    if let Ok(p) = std::env::var("MERIDIAN_SETTINGS_PATH") {
        if !p.is_empty() {
            let p = expand_tilde(&p);
            tracing::debug!(source = "env_override", path = %p.display(), "settings_json_path resolved");
            return p;
        }
    }

    let canonical = std::env::var("HOME")
        .ok()
        .map(|home| PathBuf::from(home).join(".meridian").join("settings.json"));

    if let Some(path) = &canonical {
        if path.exists() {
            tracing::debug!(source = "canonical", path = %path.display(), "settings_json_path resolved");
            return path.clone();
        }
    }

    let cwd_path = std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("settings.json");
    if cwd_path.exists() {
        tracing::debug!(source = "cwd_fallback", path = %cwd_path.display(), "settings_json_path resolved");
        return cwd_path;
    }

    // Neither exists yet — default to the canonical path so a first write lands
    // in the right, install-independent place.
    let p = canonical.unwrap_or(cwd_path);
    tracing::debug!(source = "default_not_yet_created", path = %p.display(), "settings_json_path resolved");
    p
}

/// Load `settings.json`, falling back to defaults if absent or unparseable.
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
        Err(_) => RuntimeSettings::default(), // file doesn't exist yet — fine
    }
}
