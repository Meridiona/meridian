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
//! The PUT (write) side is [`read_settings_value`] + [`write_settings_value`]:
//! these work on the raw JSON object (not the typed struct) so a write preserves
//! any keys not in [`RuntimeSettings`] — a faithful match of the route's
//! `{ ...current, ...body }` merge — and the write is crash-safe (temp + rename).
//!
//! # Related
//! - `ui/lib/settings.ts` is the TS mirror of this schema + defaults — keep the
//!   two in sync (the `SETTINGS_DEFAULTS` there must match [`RuntimeSettings::default`]).
//! - The tray `update_settings` command (ported `/api/settings` PUT) validates +
//!   merges the body on top of [`read_settings_value`] and persists via
//!   [`write_settings_value`].

use anyhow::Context;
use serde::{Deserialize, Serialize};
use serde_json::Value;
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

/// The 3 string-typed fields the daemon stores as `Option` (serialised to JSON
/// `null` when unset). The dashboard's `readSettings` coerces these `null → ''`
/// so TS consumers never hit a null on a string field — we mirror that here.
const NULLABLE_STRING_FIELDS: [&str; 3] = ["otlp_endpoint", "oo_email", "oo_password"];

/// Read the current settings as a raw JSON object — defaults overlaid with the
/// on-disk file (file wins), with the nullable string fields coerced `null → ''`.
/// A faithful mirror of `readSettings()`: working on the `Value` (not the typed
/// struct) preserves any extra keys the file carries, so a subsequent write can
/// round-trip them. Returns defaults alone when the file is absent/unparseable.
pub fn read_settings_value() -> Value {
    let mut v = serde_json::to_value(RuntimeSettings::default())
        .unwrap_or_else(|_| Value::Object(Default::default()));
    let path = settings_json_path();
    if let Ok(s) = std::fs::read_to_string(&path) {
        match serde_json::from_str::<Value>(&s) {
            Ok(Value::Object(file)) => {
                if let Some(obj) = v.as_object_mut() {
                    for (k, val) in file {
                        obj.insert(k, val); // file key overrides the default
                    }
                }
            }
            Ok(_) => tracing::warn!(path = %path.display(), "settings.json is not an object"),
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "settings.json parse error")
            }
        }
    }
    if let Some(obj) = v.as_object_mut() {
        for k in NULLABLE_STRING_FIELDS {
            if obj.get(k).map(Value::is_null).unwrap_or(false) {
                obj.insert(k.to_string(), Value::String(String::new()));
            }
        }
    }
    v
}

/// Persist a settings object to `settings.json`, crash-safely. Writes pretty
/// 2-space JSON (matching the dashboard's `JSON.stringify(x, null, 2)`) to a
/// temp file in the same dir, then atomically renames it over the target — so a
/// crash mid-write can never truncate the file holding the OO credentials (an
/// improvement over the route's plain `writeFileSync`, per the config-safety
/// practice). Creates the parent dir on first write.
pub fn write_settings_value(settings: &Value) -> anyhow::Result<()> {
    let path = settings_json_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("creating settings dir {}", dir.display()))?;
    }
    let json = serde_json::to_string_pretty(settings).context("serialising settings")?;
    // Temp file in the SAME directory so the rename is atomic (same filesystem).
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json.as_bytes())
        .with_context(|| format!("writing temp settings {}", tmp.display()))?;
    std::fs::rename(&tmp, &path)
        .with_context(|| format!("atomically replacing {}", path.display()))?;
    tracing::info!(path = %path.display(), "settings.json written");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Point settings resolution at a temp file for the duration of a test.
    /// (Serialised by the caller via distinct paths — each test uses its own.)
    fn with_settings_path(path: &std::path::Path) {
        std::env::set_var("MERIDIAN_SETTINGS_PATH", path);
    }

    #[test]
    fn read_coerces_nulls_and_write_round_trips_extra_keys() {
        let dir =
            std::env::temp_dir().join(format!("meridian-settings-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        with_settings_path(&path);

        // Seed a file with an extra (unknown) key + an explicit null string field.
        std::fs::write(
            &path,
            r#"{"log_level":"DEBUG","oo_email":null,"_extra":"keep me"}"#,
        )
        .unwrap();

        let v = read_settings_value();
        assert_eq!(v["log_level"], json!("DEBUG"), "file overrides default");
        assert_eq!(v["oo_email"], json!(""), "null string coerced to ''");
        assert_eq!(v["_extra"], json!("keep me"), "unknown key preserved");
        // a default-only field still present
        assert_eq!(v["poll_interval_secs"], json!(60));

        // Write it back and confirm the extra key survives a round-trip.
        write_settings_value(&v).unwrap();
        let again = read_settings_value();
        assert_eq!(again["_extra"], json!("keep me"));

        std::env::remove_var("MERIDIAN_SETTINGS_PATH");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
