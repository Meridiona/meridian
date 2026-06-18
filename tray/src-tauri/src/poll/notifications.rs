//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! The daemon notification outbox drain + the delivery-policy check.
//!
//! The tray is a dumb delivery agent: the daemon enqueues notifications into
//! `meridian.db`; this reads the native-channel queue directly via
//! [`meridian_core::notifications`] (the ported `/api/notifications/pending` +
//! `/allowed`), toasts each, and acks delivery. Preference + quiet-hours
//! filtering live in `meridian-core` (one source, shared with the daemon's
//! settings), so the tray no longer round-trips the dashboard for policy.
//!
//! # Related
//! - [`super::refresh::refresh_health`] — gates its toasts via [`notifications_allowed`].
//! - [`crate::commands::daemon::toggle_daemon`] — same gate for the pause/resume toast.
//! - The `/api/notifications/:id/delivered` ack is still HTTP — that *write*
//!   route isn't ported yet; it's the last HTTP hop here.

use crate::sys::{notify, ui_base};
use reqwest::Client;
use tauri::Manager;

/// UTC ISO without sub-seconds — matches the route's `now` for the
/// scheduled_for/expires_at string comparison in `pending_native`.
fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// Drain the daemon's native notification queue: read pending directly from
/// `meridian.db` (ported `/api/notifications/pending`), toast each, then ack via
/// `/api/notifications/:id/delivered`. A failed ack just retries next tick —
/// at-least-once delivery.
pub(super) async fn drain_notifications(app: &tauri::AppHandle, client: &Client) {
    let pool_state = app.state::<Option<meridian_core::SqlitePool>>();
    let Some(pool) = pool_state.inner() else {
        return; // DB not open yet — nothing to drain
    };
    let settings = meridian_core::settings::load_runtime_settings();
    let items = meridian_core::notifications::pending_native(pool, &now_iso(), &settings).await;

    for n in items {
        notify(app, &n.title, &n.body);
        let _ = client
            .post(format!(
                "{}/api/notifications/{}/delivered",
                ui_base(),
                n.id
            ))
            .send()
            .await;
    }
}

/// Whether a notification for `event_key` may fire right now — master switch +
/// per-type toggle + quiet hours, computed directly from `settings.json` (ported
/// `/api/notifications/allowed`, no HTTP). Fails open when settings are
/// missing/corrupt (`load_runtime_settings` → defaults, notifications on) so an
/// operational alert (e.g. "went quiet") is never lost to a policy-read failure.
pub(crate) async fn notifications_allowed(event_key: &str) -> bool {
    let s = meridian_core::settings::load_runtime_settings();
    meridian_core::notifications::event_allowed(event_key, &s)
        && !meridian_core::notifications::in_quiet_hours(&s)
}
