//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Notification commands — the ported banner-channel notification surface.
//!
//! - [`get_banner_notifications`] — the snapshot read (ported
//!   `/api/notifications/stream`'s query): the active banner set, served on first
//!   paint and re-pushed by the poll loop's `notifications-update` event.
//! - [`dismiss_notification`] — the dismiss write: the dashboard banner calls it
//!   when the user dismisses an in-app notification.
//! - [`record_notification_response`] — the interactive-toast answer write: the
//!   popover's `actionPerformed` listener forwards each button press / tap /
//!   inline reply here, which stamps it on the outbox row for the daemon's
//!   response consumer and routes foreground actions to the dashboard.
//!
//! The sibling *delivered* ack (`/api/notifications/:id/delivered`) is NOT a
//! command — it's an internal poll-loop write now (see [`crate::poll`]'s
//! `drain_notifications`), so the tray delivers + acks the native channel with no
//! HTTP hop.
//!
//! # Who calls this
//! Registered in `lib.rs`'s `invoke_handler!`; consumed by
//! `ui/components/NotificationBanner.tsx` (`get_banner_notifications` via
//! `bridge.subscribe`; `dismiss_notification` on user dismiss) and the popover
//! (`tray/src/app.js` forwards plugin action events to
//! `record_notification_response`).
//!
//! # Related
//! - [`meridian_core::notifications`] — the byte-for-byte ports
//!   ([`meridian_core::notifications::active_banners`] /
//!   [`meridian_core::notifications::dismiss_banner`]) and the response leg
//!   ([`meridian_core::notifications::record_response`]).
//! - [`crate::sys::notify_outbox`] — the delivery half that embeds the
//!   outbox id + deep link the response listener hands back here.
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

/// Record the user's answer to an interactive toast (button press, inline
/// reply, tap, or dismiss) onto its outbox row — the response leg of the
/// notification mailbox. `id` is the outbox row id (== the toast's
/// notification identifier — the only value the plugin round-trips; see
/// [`crate::sys::notify_outbox`]); `action` is the pressed action id or
/// `'tap'`/`'dismiss'`; `text` is the inline-reply input, if any. First answer
/// wins (the core write's `IS NULL` guard) — a duplicate event is a no-op.
///
/// Foreground answers (`tap`, or an `open`/`view` button) on a row that
/// carries a `deep_link` also open the dashboard window, reusing the same
/// opener as every other click-through path; the link is resolved from the DB
/// row, not the toast. Navigation is best-effort: the recorded response is the
/// source of truth, so an opener failure is logged, not surfaced.
#[tauri::command]
#[tracing::instrument(skip(app, pool, text))]
pub async fn record_notification_response(
    app: tauri::AppHandle,
    pool: State<'_, Option<meridian_core::SqlitePool>>,
    id: i64,
    action: String,
    text: Option<String>,
) -> Result<(), String> {
    let Some(pool) = pool.inner() else {
        return Err("meridian.db is not open yet".to_string());
    };
    let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    meridian_core::notifications::record_response(pool, id, &action, text.as_deref(), &now)
        .await
        .map_err(|e| {
            tracing::warn!(error = %e, id, action, "record_notification_response failed");
            e.to_string()
        })?;
    tracing::info!(
        id,
        action,
        has_text = text.is_some(),
        "notification response recorded"
    );

    // The dashboard is a single window (deep_link routes were folded into one
    // page), so every foreground answer lands on the same opener. The link is
    // handed to the window first ([`crate::deep_link::navigate_dashboard`]:
    // parked for a fresh window's mount-time pull, or emitted to an
    // already-open one) so the click actually lands on the linked view (e.g.
    // the Plan modal), not just the default timeline.
    if matches!(action.as_str(), "tap" | "open" | "view") {
        if let Some(link) = meridian_core::notifications::notification_deep_link(pool, id).await {
            crate::deep_link::navigate_dashboard(&app, &link);
            if let Err(e) = crate::commands::open_dashboard(app).await {
                tracing::warn!(error = %e, id, "notification click-through open failed");
            }
        }
    }
    Ok(())
}
