//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! `/api/version` ported to Rust — a faithful port of `ui/app/api/version/route.ts`.
//!
//! # What this is
//! The installed version + whether a newer one is published, for the dashboard's
//! "update available" banner. Reads the bundle's `~/.meridian/app/VERSION` (or
//! `MERIDIAN_VERSION`, else `"dev"`) and the latest published version from the
//! npm registry. NOT a DB read (file + external HTTP), so it lives tray-side, not
//! in meridian-core.
//!
//! [`run_update`] (the ported `/api/update` POST) is the action behind that
//! banner: it launches `meridian update` in a visible Terminal window (it
//! self-elevates the npm step + restarts daemons, so it must run interactively,
//! not silently inside the app).
//!
//! # Who calls this
//! The `get_version` + `run_update` Tauri commands → the dashboard `Sidebar`
//! (the version / update line). `get_version` never throws — an update check
//! must not break the UI.
//!
//! # Related
//! - The npm result is cached process-wide for [`CHECK_TTL_MS`] so a dashboard
//!   left open doesn't hammer the registry (mirrors the route's module cache).

use chrono::{DateTime, Duration as ChronoDuration, SecondsFormat, Utc};
use serde::Serialize;
use std::sync::Mutex;
use std::time::Duration;
use tracing::Instrument;

/// npm package whose published `latest` dist-tag we compare against.
const NPM_PKG_URL: &str = "https://registry.npmjs.org/@meridiona/meridian";
/// Re-check the registry at most hourly.
const CHECK_TTL_MS: i64 = 60 * 60 * 1000;

/// Process-wide cache of the npm result (`latest`, when checked). Persists
/// across `get_version` calls; `None` until the first check.
static CACHE: Mutex<Option<(Option<String>, DateTime<Utc>)>> = Mutex::new(None);

/// Mirrors the route's `VersionInfo`. `camelCase` to match the JSON the
/// dashboard's `VersionInfo` type expects (`updateAvailable`, `checkedAt`).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionInfo {
    pub current: String,
    pub latest: Option<String>,
    pub update_available: bool,
    pub checked_at: Option<String>,
}

/// Installed bundle version from `~/.meridian/app/VERSION`, else
/// `MERIDIAN_VERSION`, else `"dev"` (a source/dev run).
fn read_current_version() -> String {
    if let Ok(home) = std::env::var("HOME") {
        if let Ok(v) = std::fs::read_to_string(format!("{home}/.meridian/app/VERSION")) {
            let v = v.trim();
            if !v.is_empty() {
                return v.to_string();
            }
        }
    }
    std::env::var("MERIDIAN_VERSION")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "dev".to_string())
}

/// Dotted numeric compare — `true` if `latest` > `current`. A `dev` current is
/// never behind. Mirrors the route's `isNewer` (strip a leading `v`, drop any
/// pre-release suffix, compare segment by segment).
fn is_newer(latest: &str, current: &str) -> bool {
    if current == "dev" {
        return false;
    }
    let norm = |v: &str| -> Vec<i64> {
        v.trim_start_matches('v')
            .split('-')
            .next()
            .unwrap_or("")
            .split('.')
            .map(|n| n.parse::<i64>().unwrap_or(0))
            .collect()
    };
    let a = norm(latest);
    let b = norm(current);
    for i in 0..a.len().max(b.len()) {
        let d = a.get(i).copied().unwrap_or(0) - b.get(i).copied().unwrap_or(0);
        if d != 0 {
            return d > 0;
        }
    }
    false
}

/// Latest published version (cached for [`CHECK_TTL_MS`]). Never errors: on a
/// network/registry failure it keeps any prior result, else reports `None` —
/// an update check must not break the dashboard.
async fn fetch_latest() -> (Option<String>, DateTime<Utc>) {
    // Fast path: a fresh cached result. Lock is released before any await.
    if let Some((latest, checked)) = CACHE.lock().unwrap().as_ref() {
        if Utc::now() - *checked < ChronoDuration::milliseconds(CHECK_TTL_MS) {
            tracing::debug!("version: npm cache hit");
            return (latest.clone(), *checked);
        }
    }

    let fetched: Result<Option<String>, String> = async {
        let resp = reqwest::Client::new()
            .get(NPM_PKG_URL)
            // Abbreviated metadata: dist-tags without the full per-version payload.
            .header(
                reqwest::header::ACCEPT,
                "application/vnd.npm.install-v1+json",
            )
            .timeout(Duration::from_secs(5))
            .send()
            .await
            .map_err(|e| e.to_string())?;
        if !resp.status().is_success() {
            return Err(format!("npm registry {}", resp.status()));
        }
        let body: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
        Ok(body
            .get("dist-tags")
            .and_then(|t| t.get("latest"))
            .and_then(|l| l.as_str())
            .map(str::to_string))
    }
    .instrument(tracing::debug_span!("version.fetch.npm_registry"))
    .await;

    let mut guard = CACHE.lock().unwrap();
    match fetched {
        Ok(latest) => *guard = Some((latest, Utc::now())),
        Err(e) => {
            tracing::warn!(error = %e, "version check: npm registry unreachable");
            // Keep any prior result; otherwise record a null check now.
            if guard.is_none() {
                *guard = Some((None, Utc::now()));
            }
        }
    }
    guard.clone().expect("cache populated above")
}

/// Current vs latest version for the dashboard (the ported /api/version GET).
#[tauri::command]
#[tracing::instrument]
pub async fn get_version() -> VersionInfo {
    let current = read_current_version();
    let (latest, checked_at) = fetch_latest().await;
    let update_available = latest
        .as_deref()
        .map(|l| is_newer(l, &current))
        .unwrap_or(false);
    tracing::info!(%current, latest = ?latest, update_available, "version served");
    VersionInfo {
        current,
        latest,
        update_available,
        checked_at: Some(checked_at.to_rfc3339_opts(SecondsFormat::Millis, true)),
    }
}

/// The command a user runs to update — bare `meridian` on purpose: it runs in an
/// interactive Terminal login shell (PATH has node), the one sanctioned spot for
/// a bare invocation (see CLAUDE.md), so it does NOT use [`crate::install::meridian_bin`].
const UPDATE_CMD: &str = "meridian update";

/// `{ launched, command }` — mirrors the route. `launched=false` (still a
/// success, not an error) tells the UI to show the copyable `command` fallback.
#[derive(Debug, Clone, Serialize)]
pub struct UpdateLaunch {
    pub launched: bool,
    pub command: String,
}

/// Launch `meridian update` in a visible Terminal window (the ported /api/update
/// POST). We do NOT run the update inside the app: it restarts the daemons and
/// may prompt for a password, so it must run in an interactive terminal the user
/// can see. Mechanism (mirrors the route): write a `.command` script and
/// `open -a Terminal` it — LaunchServices, so no AppleEvents/Automation prompt,
/// and an interactive login shell where `meridian` is on PATH. Never errors —
/// returns `launched=false` so the UI falls back to the copyable command.
#[tauri::command]
#[tracing::instrument]
pub async fn run_update() -> UpdateLaunch {
    let script = [
        "#!/bin/bash",
        "echo \"→ Updating Meridian…\"",
        "echo",
        UPDATE_CMD,
        "status=$?",
        "echo",
        "if [ $status -eq 0 ]; then echo \"✓ Update complete — you can close this window.\"; \
         else echo \"✗ Update failed (exit $status). See output above.\"; fi",
        "",
    ]
    .join("\n");
    let script_path = std::env::temp_dir().join("meridian-update.command");

    let launched = write_and_open(&script_path, &script).is_ok();
    if launched {
        tracing::info!(path = %script_path.display(), "launched meridian update in Terminal");
    } else {
        tracing::warn!("could not launch Terminal for update");
    }
    UpdateLaunch {
        launched,
        command: UPDATE_CMD.to_string(),
    }
}

/// Write the `.command` script (executable) and `open -a Terminal` it. Split out
/// so the single `is_ok()` above captures any failure as `launched=false`.
fn write_and_open(script_path: &std::path::Path, script: &str) -> std::io::Result<()> {
    std::fs::write(script_path, script)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(script_path, std::fs::Permissions::from_mode(0o755))?;
    }
    // Spawn detached and drop the handle — `open` returns immediately; the
    // Terminal window outlives this command.
    std::process::Command::new("open")
        .args(["-a", "Terminal"])
        .arg(script_path)
        .spawn()
        .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::is_newer;

    #[test]
    fn is_newer_compares_dotted_versions() {
        assert!(is_newer("1.2.0", "1.1.9"));
        assert!(is_newer("2.0.0", "1.9.9"));
        assert!(!is_newer("1.1.0", "1.1.0"));
        assert!(!is_newer("1.0.0", "1.0.1"));
    }

    #[test]
    fn dev_is_never_behind() {
        assert!(!is_newer("9.9.9", "dev"));
    }

    #[test]
    fn tolerates_v_prefix_and_prerelease_suffix() {
        assert!(is_newer("v1.2.0", "1.1.0"));
        assert!(!is_newer("1.2.0-beta.1", "1.2.0")); // pre-release suffix dropped → equal
    }
}
