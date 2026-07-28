//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! `/api/version` ported to Rust — a faithful port of `ui/app/api/version/route.ts`.
//!
//! # What this is
//! The installed version + whether a newer one is published, for the dashboard's
//! "update available" banner. Reads the app's baked version and the latest
//! published version from the GitHub releases API (the app is distributed as a
//! signed DMG — there is no npm package). NOT a DB read (file + external HTTP),
//! so it lives tray-side, not in meridian-core.
//!
//! [`run_update`] (the ported `/api/update` POST) is the source-checkout action
//! behind that banner: it launches `meridian update` in a visible Terminal window
//! (it rebuilds + restarts daemons, so it must run interactively). A packaged
//! `.app` updates itself in-app instead — see [`install_update`].
//!
//! # Who calls this
//! The `get_version` + `run_update` Tauri commands → the dashboard `Sidebar`
//! (the version / update line). `get_version` never throws — an update check
//! must not break the UI. `get_app_info` → the dashboard's Account settings
//! section and the popover footer (the version/channel badge, dev vs staging
//! vs prod) — synchronous, no network round trip.
//!
//! # Related
//! - The GitHub result is cached process-wide for [`CHECK_TTL_MS`] so a dashboard
//!   left open doesn't hammer the API.

use chrono::{DateTime, Duration as ChronoDuration, SecondsFormat, Utc};
use serde::Serialize;
use std::sync::Mutex;
use std::time::Duration;
use tracing::Instrument;

/// GitHub releases API — its `tag_name` is the latest published version.
const GITHUB_LATEST_URL: &str = "https://api.github.com/repos/Meridiona/meridian/releases/latest";
/// Re-check the API at most hourly.
const CHECK_TTL_MS: i64 = 60 * 60 * 1000;

/// Process-wide cache of the GitHub result (`latest`, when checked). Persists
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

/// Installed version: `MERIDIAN_VERSION` if set (a legacy override), else the
/// app's baked `tauri.conf.json` version, else `"dev"` (an unbundled run).
fn read_current_version(app: &tauri::AppHandle) -> String {
    if let Ok(v) = std::env::var("MERIDIAN_VERSION") {
        let v = v.trim();
        if !v.is_empty() {
            return v.to_string();
        }
    }
    if cfg!(debug_assertions) {
        return "dev".to_string();
    }
    app.package_info().version.to_string()
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
            tracing::debug!("version: github cache hit");
            return (latest.clone(), *checked);
        }
    }

    let fetched: Result<Option<String>, String> = async {
        let resp = reqwest::Client::new()
            .get(GITHUB_LATEST_URL)
            // GitHub's API rejects requests without a User-Agent (403).
            .header(reqwest::header::USER_AGENT, "meridian-tray")
            .header(reqwest::header::ACCEPT, "application/vnd.github+json")
            .timeout(Duration::from_secs(5))
            .send()
            .await
            .map_err(|e| e.to_string())?;
        if !resp.status().is_success() {
            return Err(format!("github api {}", resp.status()));
        }
        let body: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
        // `tag_name` is like "v1.71.0"; is_newer() strips the leading v itself.
        Ok(body
            .get("tag_name")
            .and_then(|t| t.as_str())
            .map(str::to_string))
    }
    .instrument(tracing::debug_span!("version.fetch.github_releases"))
    .await;

    let mut guard = CACHE.lock().unwrap();
    match fetched {
        Ok(latest) => *guard = Some((latest, Utc::now())),
        Err(e) => {
            tracing::warn!(error = %e, "version check: github releases API unreachable");
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
#[tracing::instrument(skip(app))]
pub async fn get_version(app: tauri::AppHandle) -> VersionInfo {
    let current = read_current_version(&app);
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
///
/// **macOS only.** On Windows the update path is the in-app auto-updater
/// (`tauri-plugin-updater` → the NSIS installer), and there is no `meridian`
/// CLI on `PATH` to run in a terminal, so this reports `launched=false` without
/// spawning anything — the UI then shows its "check for updates" affordance,
/// which drives the auto-updater.
#[tauri::command]
#[tracing::instrument]
pub async fn run_update() -> UpdateLaunch {
    #[cfg(target_os = "macos")]
    let launched = {
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
        let ok = write_and_open(&script_path, &script).is_ok();
        if ok {
            tracing::info!(path = %script_path.display(), "launched meridian update in Terminal");
        } else {
            tracing::warn!("could not launch Terminal for update");
        }
        ok
    };

    #[cfg(not(target_os = "macos"))]
    let launched = {
        // No terminal launch on Windows/Linux — the auto-updater is the path.
        tracing::debug!("run_update: no terminal-update flow off macOS — auto-updater handles it");
        false
    };

    UpdateLaunch {
        launched,
        command: UPDATE_CMD.to_string(),
    }
}

/// Write the `.command` script (executable) and `open -a Terminal` it. Split out
/// so the single `is_ok()` above captures any failure as `launched=false`.
#[cfg(target_os = "macos")]
fn write_and_open(script_path: &std::path::Path, script: &str) -> std::io::Result<()> {
    std::fs::write(script_path, script)?;
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

/// Build channel this binary was compiled for — lets a user tell a local dev
/// run apart from a staging or production build at a glance. `dev` is a
/// runtime check (`tauri dev` / unbundled `cargo run`); `staging` is baked in
/// at compile time via `MERIDIAN_CHANNEL` (an `option_env!` baked in by the
/// staging release workflow);
/// anything else (a plain release build with no channel baked in) is `prod`.
/// `pub(crate)` — also read by [`crate::analytics::base_properties`] to tag
/// every PostHog event with the same dev/staging/prod distinction.
pub(crate) fn build_channel() -> &'static str {
    if cfg!(debug_assertions) {
        return "dev";
    }
    option_env!("MERIDIAN_CHANNEL").unwrap_or("prod")
}

/// `{ version, channel, supportId }` for the small badge in the dashboard and
/// popover, plus the Account panel's support identifier.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    pub version: String,
    pub channel: String,
    /// This machine's pseudonym (ALPHA TESTING, until 2026-08-28: while
    /// signed in, the signed-in account's pseudonym instead — see
    /// `meridian::telemetry_spool::redact::local_host_pseudonym`'s doc) — the
    /// exact value error telemetry from here carries as `host.name` in the
    /// central backend. Shown in Settings so a user can quote it when they
    /// contact support; without it their error rows are unfindable, since
    /// pseudonymisation is one-way by design.
    pub support_id: String,
}

/// The binary's baked `tauri.conf.json` version (what the DMG updater compares
/// against — set in lockstep across the repo by `scripts/set-version.sh`) plus
/// [`build_channel`]. Synchronous and never touches the network, unlike
/// [`get_version`] above.
///
/// `support_id` comes from [`meridian::telemetry_spool::redact::local_host_pseudonym`]
/// — the SAME function the daemon's ship leg uses, so the displayed value and
/// the shipped one cannot drift (see that function's doc for why sharing it
/// matters).
#[tauri::command]
#[tracing::instrument(skip(app))]
pub fn get_app_info(app: tauri::AppHandle) -> AppInfo {
    let version = app.package_info().version.to_string();
    let channel = build_channel().to_string();
    let support_id = meridian::telemetry_spool::redact::local_host_pseudonym();
    // `support_id` is deliberately NOT a field here. It is a machine
    // identifier, and it is already available where it is actually needed - the
    // command's return value, Settings → Account, and `bundle-info.txt` in an
    // export bundle - so logging it adds no diagnostic value and only widens
    // where an identifier is written.
    tracing::info!(%version, %channel, "app info served");
    AppInfo {
        version,
        channel,
        support_id,
    }
}

/// DMG auto-update check for the in-app banners (sidebar + popover). Wraps
/// [`crate::update::check_status`] — a structured, never-throwing status
/// (`available` / `uptodate` / `unsupported` / `error`). Distinct from
/// [`get_version`] above: that reports current-vs-latest for the version line
/// (GitHub releases tag); this drives the in-app DMG updater against the GitHub
/// `latest.json` the packaged `.app` installs from.
#[tauri::command]
#[tracing::instrument(skip(app))]
pub async fn check_update(app: tauri::AppHandle) -> crate::update::UpdateStatus {
    crate::update::check_status(&app).await
}

/// Download + install the available DMG update, then relaunch. Emits
/// `update-progress` events the banners render as a bar. Returns `Err(String)`
/// only on a pre-relaunch failure (success never returns — the app re-execs).
#[tauri::command]
#[tracing::instrument(skip(app))]
pub async fn install_update(app: tauri::AppHandle) -> Result<(), String> {
    crate::update::download_and_apply(&app).await
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
