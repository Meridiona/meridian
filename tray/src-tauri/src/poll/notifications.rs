//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! The daemon notification outbox drain + the delivery-policy check.
//!
//! The tray is a dumb delivery agent: the daemon enqueues notifications into
//! `meridian.db`; this reads the native-channel queue directly via
//! [`meridian_core::notifications`] (the ported `/api/notifications/pending` +
//! `/allowed`), toasts each, and acks delivery. Preference + quiet-hours
//! filtering live in `meridian-core` (one source, shared with the daemon's
//! settings), so the tray no longer round-trips the dashboard for policy. The
//! delivery ack is now a direct `meridian.db` write too — the loop is HTTP-free.
//!
//! # Related
//! - [`super::refresh::refresh_health`] — gates its toasts via [`notifications_allowed`].
//! - [`crate::commands::daemon::toggle_daemon`] — same gate for the pause/resume toast.
//! - [`meridian_core::notifications::mark_native_delivered`] — the delivery ack.

use crate::sys::{notify_outbox, retract_toast};
use tauri::Manager;

/// UTC ISO without sub-seconds — matches the route's `now` for the
/// scheduled_for/expires_at string comparison in `pending_native` and the
/// `delivered_native_at` stamp.
fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// Drain the daemon's native notification queue: read pending directly from
/// `meridian.db` (ported `/api/notifications/pending`), toast each (plain or
/// interactive — [`notify_outbox`] decides by the row's `category`), then ack
/// delivery with a direct DB write ([`meridian_core::notifications::mark_native_delivered`],
/// ported `/api/notifications/:id/delivered`). A failed ack just retries next
/// tick — at-least-once delivery.
///
/// The same pass sweeps the other end of the lifecycle: delivered-but-ignored
/// rows past their `expires_at` are retracted from the screen/Notification
/// Center and stamped `response_action = 'expired'` — under the persistent
/// Alerts style this is what makes a "fleeting" notification possible, and it
/// makes ignored-until-expiry measurable (vs. never answered at all).
pub(super) async fn drain_notifications(app: &tauri::AppHandle) {
    let pool_state = app.state::<Option<meridian_core::SqlitePool>>();
    let Some(pool) = pool_state.inner() else {
        return; // DB not open yet — nothing to drain
    };
    let settings = meridian_core::settings::load_runtime_settings();
    let now = now_iso();
    let items = meridian_core::notifications::pending_native(pool, &now, &settings).await;

    for n in items {
        notify_outbox(app, &n);
        if let Err(e) = meridian_core::notifications::mark_native_delivered(pool, n.id, &now).await
        {
            // Leave the row unacked → re-delivered next tick (at-least-once).
            tracing::warn!(error = %e, id = n.id, "notification delivered-ack failed");
        }
    }

    // Expiry sweep. Stamp BEFORE retracting: the stamp is what stops the row
    // re-qualifying next tick, and it wins races with a user answering at the
    // same moment (first answer wins via record_response's IS NULL guard).
    for id in meridian_core::notifications::expired_unanswered(pool, &now).await {
        if let Err(e) =
            meridian_core::notifications::record_response(pool, id, "expired", None, &now).await
        {
            tracing::warn!(error = %e, id, "expiry stamp failed — will retry next tick");
            continue;
        }
        retract_toast(app, id);
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
