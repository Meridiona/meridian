//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! DMG auto-update via `tauri-plugin-updater`.
//!
//! # What this is
//! The in-app side of the public-distribution plan: the running app checks the
//! GitHub `latest.json` (endpoint + minisign pubkey are in `tauri.conf.json`),
//! and when a newer version is published, downloads the signed `.app.tar.gz`,
//! verifies it against the embedded pubkey, swaps it over the installed bundle,
//! and relaunches into it.
//!
//! This is the **DMG** path. An npm/CLI install updates instead via `meridian
//! update` in a Terminal ([`crate::commands::run_update`]); the two mechanisms
//! are independent and never both fire — a DMG install has no `~/.meridian/app`
//! npm bundle, and the npm install isn't the `.app` the updater rewrites.
//!
//! # Surfaces
//! - [`check_status`] → the dashboard sidebar + the tray popover banner (via the
//!   `check_update` command) — a structured, never-throwing status so the UI can
//!   render "available / up to date / unsupported / error" instead of a silent
//!   toast. **This is the primary surface** (the tray notification path proved
//!   invisible when notifications aren't granted).
//! - [`download_and_apply`] → both banners' "Restart & Update" button (via the
//!   `install_update` command); emits `update-progress` events for a progress bar.
//! - [`check_for_updates`] → the tray menu's "Check for Updates…" (notification
//!   fallback, reusing the same core).
//!
//! # Related
//! - [`crate::sys::notify`] — the toast the tray-menu path surfaces through.
//! - Plan: Obsidian `Decisions/Public distribution + auto-update for the DMG`.

use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::{AppHandle, Emitter};
use tauri_plugin_updater::UpdaterExt;

/// Guards the tray-menu path against re-entry (a second click mid-download).
static CHECKING: AtomicBool = AtomicBool::new(false);

/// Guards the non-idempotent install across *all* UI surfaces (tray menu + both
/// banners). [`CHECKING`] only covers the tray-menu path; the `install_update`
/// command can fire from the sidebar and popover too, so the single-flight guard
/// has to wrap the install itself or parallel `download_and_install` + restart
/// attempts would race.
static INSTALLING: AtomicBool = AtomicBool::new(false);

/// Structured result of an update check, for the in-app banners. Deliberately
/// NOT a `Result` — a failed check is data (`state = "error"` + `error`) the UI
/// renders, not a thrown command that the banner would have to swallow. `state`
/// is one of: `available` · `uptodate` · `unsupported` · `error`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateStatus {
    /// `available` | `uptodate` | `unsupported` | `error`.
    pub state: String,
    /// The running app version (the baked `tauri.conf.json` version).
    pub current_version: String,
    /// The newer version, when `state == "available"`.
    pub version: Option<String>,
    /// Release notes (the manifest `notes`), when present.
    pub notes: Option<String>,
    /// Human-readable error text, when `state == "error"`.
    pub error: Option<String>,
}

impl UpdateStatus {
    /// A status with no version/notes/error — for the plain `uptodate` /
    /// `unsupported` cases.
    fn simple(state: &str, current: String) -> Self {
        Self {
            state: state.to_string(),
            current_version: current,
            version: None,
            notes: None,
            error: None,
        }
    }

    /// An `error` status carrying `msg`.
    fn errored(current: String, msg: String) -> Self {
        Self {
            state: "error".to_string(),
            current_version: current,
            version: None,
            notes: None,
            error: Some(msg),
        }
    }
}

/// True only for a packaged `.app` run. A source/dev run can't be swapped by the
/// updater (the running binary lives under `target/…`, not inside a bundle), and
/// the GitHub manifest may not be published yet — so we report `unsupported` and
/// keep the banner hidden rather than surfacing a 404 as a scary error.
fn is_packaged() -> bool {
    std::env::current_exe()
        .map(|p| p.to_string_lossy().contains(".app/Contents/MacOS"))
        .unwrap_or(false)
}

/// Check the configured endpoint for a newer version. A pure status read — does
/// NOT download — so it's safe on every popover open / sidebar mount.
#[tracing::instrument(skip(app))]
pub async fn check_status(app: &AppHandle) -> UpdateStatus {
    let current = app.package_info().version.to_string();
    if !is_packaged() {
        tracing::info!("update: not a packaged .app — reporting unsupported");
        return UpdateStatus::simple("unsupported", current);
    }
    let updater = match app.updater() {
        Ok(u) => u,
        Err(e) => return UpdateStatus::errored(current, e.to_string()),
    };
    match updater.check().await {
        Ok(Some(u)) => {
            tracing::info!(version = %u.version, "update: newer version available");
            UpdateStatus {
                state: "available".to_string(),
                current_version: current,
                version: Some(u.version.clone()),
                notes: u.body.clone(),
                error: None,
            }
        }
        Ok(None) => {
            tracing::info!("update: already up to date");
            UpdateStatus::simple("uptodate", current)
        }
        Err(e) => {
            tracing::warn!(error = %e, "update: check failed");
            UpdateStatus::errored(current, e.to_string())
        }
    }
}

/// Download → verify → install the available update, emitting `update-progress`
/// `{ downloaded, contentLength }` events so the banners can show a bar, then
/// relaunch into the new version. Returns `Err` only on a pre-relaunch failure —
/// the success path never returns (`restart()` re-execs the app).
#[tracing::instrument(skip(app))]
pub async fn download_and_apply(app: &AppHandle) -> Result<(), String> {
    // Single-flight: refuse a concurrent install. On the happy path `restart()`
    // re-execs the process so the guard's reset never matters; on every error
    // path the drop guard clears it so a failed install can be retried.
    if INSTALLING.swap(true, Ordering::SeqCst) {
        return Err("An update is already being installed.".to_string());
    }
    struct ResetOnDrop;
    impl Drop for ResetOnDrop {
        fn drop(&mut self) {
            INSTALLING.store(false, Ordering::SeqCst);
        }
    }
    let _reset = ResetOnDrop;

    let updater = app.updater().map_err(|e| e.to_string())?;
    let update = updater
        .check()
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "No update available".to_string())?;

    let emitter = app.clone();
    let mut downloaded: usize = 0;
    update
        .download_and_install(
            move |chunk, total| {
                downloaded += chunk;
                let _ = emitter.emit(
                    "update-progress",
                    serde_json::json!({ "downloaded": downloaded, "contentLength": total }),
                );
            },
            || tracing::info!("update: download finished, installing"),
        )
        .await
        .map_err(|e| e.to_string())?;

    tracing::info!("update: installed — restarting");
    app.restart();
    #[allow(unreachable_code)]
    Ok(())
}

/// Tray-menu entry ("Check for Updates…"). Notification-driven (the menu has no
/// window for a banner), reusing the same core as the in-app surfaces. Always
/// user-initiated, so it installs + relaunches on success and toasts every
/// outcome. The in-app banners are the primary path; this is the fallback.
pub fn check_for_updates(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if CHECKING.swap(true, Ordering::SeqCst) {
            tracing::info!("update: check already in progress — skipping");
            return;
        }
        let status = check_status(&app).await;
        match status.state.as_str() {
            "available" => {
                let v = status.version.clone().unwrap_or_default();
                crate::sys::notify(&app, "Updating Meridian", &format!("Downloading v{v}…"));
                if let Err(e) = download_and_apply(&app).await {
                    tracing::warn!(error = %e, "update: install failed");
                    crate::sys::notify(&app, "Update failed", "The update couldn't be installed.");
                }
            }
            "uptodate" => crate::sys::notify(
                &app,
                "Meridian is up to date",
                "You're on the latest version.",
            ),
            "unsupported" => crate::sys::notify(
                &app,
                "Updates via npm",
                "This build updates with `meridian update`.",
            ),
            _ => {
                let msg = status
                    .error
                    .as_deref()
                    .unwrap_or("Couldn't reach the update server.");
                crate::sys::notify(&app, "Update check failed", msg);
            }
        }
        CHECKING.store(false, Ordering::SeqCst);
    });
}
