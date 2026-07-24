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
    if !running_from_stable_location() {
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

/// macOS: `true` unless the running binary sits in a location that won't
/// survive — a mounted disk image (`/Volumes/…`, read-only and ejected after a
/// drag-install) or Gatekeeper's App Translocation quarantine
/// (`…/AppTranslocation/…`, a randomized read-only mount the OS discards). See
/// [`ensure_enabled_once`] for why pinning a login item from those paths would
/// silently and permanently break self-heal.
///
/// Non-macOS: always `true` — Windows Run-key entries and Linux desktop entries
/// point at the installed path, so the mounted-image failure class doesn't
/// apply there.
#[cfg(target_os = "macos")]
fn running_from_stable_location() -> bool {
    match std::env::current_exe() {
        Ok(exe) => path_is_stable(&exe.to_string_lossy()),
        // Can't resolve our own path ⇒ can't vouch for it. Skip and retry next
        // launch rather than gamble on pinning a bad target.
        Err(_) => false,
    }
}

#[cfg(not(target_os = "macos"))]
fn running_from_stable_location() -> bool {
    true
}

/// The pure string test behind [`running_from_stable_location`], split out so
/// it can be unit-tested without a real `current_exe()`.
#[cfg(target_os = "macos")]
fn path_is_stable(exe_path: &str) -> bool {
    !exe_path.starts_with("/Volumes/") && !exe_path.contains("/AppTranslocation/")
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::path_is_stable;

    #[test]
    fn rejects_transient_locations() {
        // Mounted DMG (drag-install source) and translocation quarantine.
        assert!(!path_is_stable(
            "/Volumes/Meridian/Meridian.app/Contents/MacOS/Meridian"
        ));
        assert!(!path_is_stable(
            "/private/var/folders/ab/xyz/T/AppTranslocation/ABC-123/d/Meridian.app/Contents/MacOS/Meridian"
        ));
    }

    #[test]
    fn accepts_stable_locations() {
        assert!(path_is_stable(
            "/Applications/Meridian.app/Contents/MacOS/Meridian"
        ));
        assert!(path_is_stable(
            "/Users/dev/Applications/Meridian.app/Contents/MacOS/Meridian"
        ));
    }
}
