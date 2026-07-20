//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Periodic re-checks of the macOS permissions the notification system and
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
        crate::sys::accessibility_trusted(),
    )
    .await;
    check_bool_permission(
        pool,
        "tray.screen_recording_revoked",
        "system.capture_permission",
        "Screen Recording access is off",
        "Meridian can't read on-screen text without it, so tracking has effectively stopped.",
        crate::sys::screen_recording_trusted(),
    )
    .await;
}

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
                remedy: Some("Open System Settings \u{2192} Privacy & Security to re-grant it."),
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
