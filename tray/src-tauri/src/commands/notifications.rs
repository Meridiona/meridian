//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Notification commands — the ported notification write routes.
//!
//! Only the *dismiss* write surfaces as a command: the dashboard banner calls it
//! when the user dismisses an in-app notification. The sibling *delivered* ack
//! (`/api/notifications/:id/delivered`) is NOT a command — it's an internal poll-
//! loop write now (see [`crate::poll`]'s `drain_notifications`), so the tray
//! delivers and acks without any HTTP hop.
//!
//! # Who calls this
//! Registered in `lib.rs`'s `invoke_handler!`; consumed by
//! `ui/components/NotificationBanner.tsx` (dual-path `invoke` / `/api` fetch).
//!
//! # Related
//! - [`meridian_core::notifications::dismiss_banner`] — the byte-for-byte port.
//! - [`crate::poll`] — drains + acks native delivery (the delivered write).

use tauri::State;

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
