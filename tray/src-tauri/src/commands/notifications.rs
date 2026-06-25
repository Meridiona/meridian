//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Notification commands — the ported banner-channel notification surface.
//!
//! - [`get_banner_notifications`] — the snapshot read (ported
//!   `/api/notifications/stream`'s query): the active banner set, served on first
//!   paint and re-pushed by the poll loop's `notifications-update` event.
//! - [`dismiss_notification`] — the dismiss write: the dashboard banner calls it
//!   when the user dismisses an in-app notification.
//!
//! The sibling *delivered* ack (`/api/notifications/:id/delivered`) is NOT a
//! command — it's an internal poll-loop write now (see [`crate::poll`]'s
//! `drain_notifications`), so the tray delivers + acks the native channel with no
//! HTTP hop.
//!
//! # Who calls this
//! Registered in `lib.rs`'s `invoke_handler!`; consumed by
//! `ui/components/NotificationBanner.tsx` (`get_banner_notifications` via
//! `bridge.subscribe`; `dismiss_notification` on user dismiss).
//!
//! # Related
//! - [`meridian_core::notifications`] — the byte-for-byte ports
//!   ([`meridian_core::notifications::active_banners`] /
//!   [`meridian_core::notifications::dismiss_banner`]).
//! - [`crate::poll`] — emits the `notifications-update` event off the same read.

use tauri::State;

/// The active banner-notification set (the ported /api/notifications/stream
/// snapshot). Resolves `now` (seconds-precision UTC, matching the route's
/// `NOW_ISO`) and the user's prefs here, so the core read stays deterministic.
/// No open DB → empty (the route's `catch → []`).
#[tauri::command]
#[tracing::instrument(skip(pool))]
pub async fn get_banner_notifications(
    pool: State<'_, Option<meridian_core::SqlitePool>>,
) -> Result<Vec<meridian_core::notifications::BannerNotification>, String> {
    let Some(pool) = pool.inner() else {
        return Ok(Vec::new());
    };
    let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let settings = meridian_core::settings::load_runtime_settings();
    let banners = meridian_core::notifications::active_banners(pool, &now, &settings).await;
    tracing::info!(count = banners.len(), "banner notifications served");
    Ok(banners)
}

/// Dismiss an in-app notification banner (the ported /api/notifications/:id/dismiss
/// POST). Idempotent — a duplicate dismiss is a no-op (the core fn's `IS NULL`
/// guard). Resolves `now` (seconds-precision UTC, matching the route's `nowIso`).
#[tauri::command]
#[tracing::instrument(skip(pool))]
pub async fn dismiss_notification(
    pool: State<'_, Option<meridian_core::SqlitePool>>,
    id: i64,
) -> Result<(), String> {
    let Some(pool) = pool.inner() else {
        return Err("meridian.db is not open yet".to_string());
    };
    let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    meridian_core::notifications::dismiss_banner(pool, id, &now)
        .await
        .map_err(|e| {
            tracing::warn!(error = %e, id, "dismiss_notification failed");
            e.to_string()
        })
}
