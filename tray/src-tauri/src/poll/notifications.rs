//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! The daemon notification outbox drain + the quiet-hours policy check.
//!
//! The tray is a dumb delivery agent: the daemon enqueues notifications
//! (preference + quiet-hours filtered server-side) and this drains them into
//! native macOS toasts with at-least-once delivery. The exception is the tray's
//! OWN health/pause toasts — the daemon can't enqueue those while it's down — so
//! they consult [`notifications_allowed`] directly against the same policy.
//!
//! # Related
//! - [`super::refresh::refresh_health`] — gates its toasts via [`notifications_allowed`].
//! - [`crate::commands::daemon::toggle_daemon`] — same gate for the pause/resume toast.

use crate::sys::{notify, ui_base};
use reqwest::Client;
use serde::Deserialize;
use std::time::Duration;

#[derive(Deserialize)]
struct PendingNotif {
    id: i64,
    title: String,
    body: String,
}

/// Poll the daemon's notification outbox and deliver each pending native
/// notification as a macOS toast, then acknowledge it so it never re-fires.
pub(super) async fn drain_notifications(app: &tauri::AppHandle, client: &Client) {
    let resp = client
        .get(format!("{}/api/notifications/pending", ui_base()))
        .send()
        .await
        .ok();

    let items: Vec<PendingNotif> = match resp {
        Some(r) if r.status().is_success() => r.json().await.unwrap_or_default(),
        _ => return,
    };

    for n in items {
        notify(app, &n.title, &n.body);
        // Acknowledge so the row is marked delivered and never shown twice. A
        // failed ack just means it retries next tick — at-least-once delivery.
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

/// Ask the dashboard whether a notification for `event_key` may fire right now,
/// honoring the user's master switch + quiet hours. The tray's direct health/
/// pause toasts don't flow through the outbox (the daemon can't enqueue while
/// it's down), so they consult the same server-side policy here. Defaults to
/// `true` when the dashboard is unreachable — an operational alert (e.g. "went
/// quiet") must not be lost just because the preference check itself failed.
pub(crate) async fn notifications_allowed(event_key: &str) -> bool {
    #[derive(Deserialize)]
    struct Allowed {
        allowed: bool,
    }
    let client = match Client::builder().timeout(Duration::from_secs(3)).build() {
        Ok(c) => c,
        Err(_) => return true,
    };
    let resp = client
        .get(format!(
            "{}/api/notifications/allowed?event={}",
            ui_base(),
            event_key
        ))
        .send()
        .await
        .ok();
    match resp {
        Some(r) if r.status().is_success() => {
            r.json::<Allowed>().await.map(|a| a.allowed).unwrap_or(true)
        }
        _ => true,
    }
}
