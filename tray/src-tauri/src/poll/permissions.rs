//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Periodic re-checks of the OS permissions the notification system and
//! capture pipeline depend on.
//!
//! Each of these is checked exactly once during onboarding
//! ([`crate::commands::setup`]) and never again — if the user revokes one
//! afterward (System Settings, or a macOS reset), the app degrades silently:
//! capture starts producing thin/no frames, or every native toast just stops
//! firing with nothing to say why. This module closes that gap by raising a
//! `system_notices` row per permission on loss, cleared on restore. The
//! dashboard banner this produces is the resilient path by construction — it
//! reads `system_notices` independent of the toast channel, so it's the one
//! notice guaranteed to reach the user even when the thing it's reporting is
//! "toasts are broken."
//!
//! The notification leg is the one check that is genuinely cross-platform:
//! [`crate::sys::notification_permission_state`] reads a real
//! `UNUserNotificationCenter` authorization on macOS and a real WinRT
//! `ToastNotifier::Setting()` on Windows, so this raises `system.notif_permission`
//! identically on either OS. Accessibility and Screen Recording stay
//! macOS-only checks in practice — Windows has no TCC analogue, so
//! [`crate::sys::accessibility_trusted`] / [`crate::sys::screen_recording_trusted`]
//! report `true` there and their `check_bool_permission` calls never fire.
//!
//! # Related
//! - [`crate::sys::notification_permission_state`] / [`crate::sys::accessibility_trusted`] /
//!   [`crate::sys::screen_recording_trusted`] — the shared probes, also used by the setup wizard.
//! - [`crate::commands::setup`] — the one-shot onboarding checks this periodically re-runs.
//! - [`meridian::notices`] — the fault-bus this writes into.

use meridian_core::SqlitePool;

/// Re-check notification, accessibility, and screen-recording permission and
/// raise/clear the matching `system_notices` row for each. Runs on the same
/// cadence as [`super::refresh::refresh_health`] (every 2nd tick, ~60s) — a
/// TCC read is cheap, no need for the full 30s tick. Skipped entirely in
/// unbundled runs (`tauri dev`): the notification plugin never registers
/// there and TCC prompts behave differently outside a signed `.app`, so
/// there's nothing meaningful to police.
#[tracing::instrument(skip(app, pool))]
pub(super) async fn check_permissions(app: &tauri::AppHandle, pool: &SqlitePool) {
    if !crate::sys::is_bundled() {
        return;
    }
    check_notification_permission(app, pool).await;
    check_bool_permission(
        pool,
        "tray.accessibility_revoked",
        "system.capture_permission",
        "Accessibility access is off",
        "Meridian can't see window and app activity without it, so tracking has effectively stopped.",
        "Open System Settings \u{2192} Privacy & Security to re-grant it.",
        crate::sys::accessibility_trusted(),
    )
    .await;
    check_bool_permission(
        pool,
        "tray.screen_recording_revoked",
        "system.capture_permission",
        "Screen Recording access is off",
        "Meridian can't read on-screen text without it, so tracking has effectively stopped.",
        "Open System Settings \u{2192} Privacy & Security to re-grant it.",
        crate::sys::screen_recording_trusted(),
    )
    .await;
}

/// Notifications-specific remedy text — the one `check_bool_permission` caller
/// that now fires on both platforms. macOS keeps the exact original copy
/// unchanged (Accessibility/Screen Recording still use the same string
/// inline); only Windows — new behaviour, nothing to stay compatible with —
/// gets an OS-accurate string instead of the macOS pane name.
#[cfg(target_os = "windows")]
const NOTIFICATION_REMEDY: &str =
    "Open Settings \u{2192} System \u{2192} Notifications to re-grant it.";
#[cfg(not(target_os = "windows"))]
const NOTIFICATION_REMEDY: &str =
    "Open System Settings \u{2192} Privacy & Security to re-grant it.";

async fn check_notification_permission(app: &tauri::AppHandle, pool: &SqlitePool) {
    let denied = matches!(
        crate::sys::notification_permission_state(app).await,
        Some(tauri_plugin_notifications::PermissionState::Denied)
    );
    check_bool_permission(
        pool,
        "tray.notifications_denied",
        "system.notif_permission",
        "Notifications are off",
        "Meridian can't show reminders or alerts without them.",
        NOTIFICATION_REMEDY,
        !denied,
    )
    .await;
}

/// Raise `id` when `granted` is false, clear it when true. Idempotent either
/// way — [`meridian::notices::raise_typed`]/[`meridian::notices::clear_typed`]
/// upsert/delete, so a steady-state tick that finds nothing changed is a cheap
/// no-op (same pattern as the daemon's own `etl.failed`/`pm.*` checks).
async fn check_bool_permission(
    pool: &SqlitePool,
    id: &str,
    event_key: &str,
    title: &str,
    detail: &str,
    remedy: &'static str,
    granted: bool,
) {
    let result = if granted {
        meridian::notices::clear_typed(pool, id, event_key).await
    } else {
        meridian::notices::raise_typed(
            pool,
            meridian::notices::Notice {
                id,
                severity: "warning",
                title,
                detail,
                remedy: Some(remedy),
                event_key,
                deep_link: Some("/settings"),
            },
        )
        .await
    };
    if let Err(e) = result {
        tracing::warn!(error = %e, id, "permission notice write failed");
    }
}

#[cfg(test)]
mod tests {
    use super::NOTIFICATION_REMEDY;

    /// Windows-only behaviour: `system.notif_permission` can now actually
    /// fire on Windows (see `sys::notification_permission_state`), so its
    /// remedy needs to point somewhere that exists on Windows — the macOS
    /// "Privacy & Security" pane name would be actively wrong there.
    #[cfg(target_os = "windows")]
    #[test]
    fn windows_remedy_points_to_windows_settings() {
        assert_eq!(
            NOTIFICATION_REMEDY,
            "Open Settings \u{2192} System \u{2192} Notifications to re-grant it."
        );
    }

    /// Regression guard for the explicit "don't touch macOS behaviour"
    /// requirement: this must stay byte-for-byte the string that shipped
    /// before the Windows notification-permission probe existed.
    #[cfg(not(target_os = "windows"))]
    #[test]
    fn non_windows_remedy_is_unchanged_from_the_original_copy() {
        assert_eq!(
            NOTIFICATION_REMEDY,
            "Open System Settings \u{2192} Privacy & Security to re-grant it."
        );
    }
}
