//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Launch-at-login registration — makes the tray come back on its own after a
//! reboot/login, the same way [`crate::backend_install`] keeps the daemon
//! running via its own `LaunchAgent`.
//!
//! # Who calls this
//! [`crate::run`]'s `setup()` hook, once per launch (bundled runs only — see
//! [`crate::sys::is_bundled`]).
//!
//! # Related
//! - [`crate::backend_install`] — the daemon's equivalent self-heal registration.
//! - [`crate::relocate`] — runs earlier in `setup()` and actively fixes a
//!   transient (DMG/translocation) launch instead of just deferring around it
//!   the way [`ensure_enabled_once`] does.

use tauri_plugin_autostart::ManagerExt;

/// Marker written after the first successful [`enable`] call — a sibling of
/// the `onboarded` marker ([`crate::commands::setup::mark_setup_complete`]),
/// but deliberately separate so it also self-heals installs that finished
/// onboarding *before* this feature shipped.
///
/// After that first success we never call `enable()` again: the OS Login
/// Items toggle becomes the user's to manage, and re-enabling on every
/// launch would silently override a manual "off" in System Settings.
const MARKER_FILE: &str = "autostart_configured";

/// Register the tray as a login item exactly once ever, self-healing across
/// both fresh installs and pre-existing ones. Best-effort: a failure (no
/// `~/.meridian`, `auto_launch` I/O error) is logged and retried on the next
/// launch — the marker is only written on success.
pub async fn ensure_enabled_once(app: &tauri::AppHandle) {
    let Some(dir) = meridian_core::paths::meridian_dir() else {
        tracing::warn!("autostart: could not resolve ~/.meridian — skipping");
        return;
    };
    let marker = dir.join(MARKER_FILE);
    if marker.exists() {
        return;
    }
    // Don't pin a login item while the app is running from a transient path
    // (a mounted DMG, or Gatekeeper's translocation quarantine). The plugin
    // records `current_exe()` as the target; from `/Volumes/…` that path is
    // gone the moment the disk image ejects, and because we register exactly
    // once (marker on success) the broken pin would never self-heal. Deferring
    // — without writing the marker — retries on the next launch, by which time
    // the user has dragged the app into a stable location (e.g. /Applications).
    if !crate::sys::running_from_stable_location() {
        tracing::info!(
            "autostart: running from a transient location (DMG mount / translocation) — \
             deferring login-item registration until the app is in a stable path"
        );
        return;
    }
    if let Err(e) = tokio::fs::create_dir_all(&dir).await {
        tracing::warn!(error = %e, "autostart: could not create ~/.meridian");
        return;
    }
    match app.autolaunch().enable() {
        Ok(()) => {
            tracing::info!("autostart: login item registered");
            if let Err(e) = tokio::fs::write(&marker, chrono::Local::now().to_rfc3339()).await {
                tracing::warn!(error = %e, "autostart: failed to write marker (will retry next launch)");
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, "autostart: failed to register login item (will retry next launch)");
        }
    }
}

// The stable-vs-transient-path check now lives in `crate::sys` — shared with
// `crate::relocate`, which is the module that actively fixes a transient
// launch rather than just deferring around it.
