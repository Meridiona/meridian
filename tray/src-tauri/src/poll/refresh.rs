//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! The poll loop's per-tick refreshers — each pulls one slice of state and
//! writes it into the shared [`AppState`].
//!
//! `refresh_health` runs the local health check directly (no HTTP); the rest
//! still fetch the transitional `/api/*` endpoints until those reads finish
//! folding into Rust commands.
//!
//! # Related
//! - [`super`] — the loop that schedules these and the tray-sync that follows.
//! - [`super::notifications_allowed`] — the quiet-hours gate `refresh_health` consults.
//! - [`crate::commands::health::check_health`] — the direct health check.

use crate::commands::health::check_health;
use crate::state::{ActiveSession, AppState, HealthStatus};
use crate::sys::{notify, ui_base};
use reqwest::Client;
use serde::Deserialize;
use std::sync::{Arc, Mutex};
use tauri::Emitter;

#[derive(Deserialize)]
struct ActiveResp {
    app_name: Option<String>,
    elapsed_s: Option<u64>,
}

#[derive(Deserialize)]
struct TodayResp {
    focus_s: Option<u64>,
    switch_count: Option<u32>,
}

#[derive(Deserialize)]
struct WorklogItem {
    state: String,
}

#[derive(Deserialize)]
struct WorklogsResp {
    items: Vec<WorklogItem>,
}

/// Run the local health check, fold it into [`AppState`], and fire the
/// went-quiet / back-online toasts (debounced to the 2nd consecutive failure).
pub(super) async fn refresh_health(
    app: &tauri::AppHandle,
    _client: &Client,
    state: &Arc<Mutex<AppState>>,
) {
    let hr = check_health().await;

    // Push the health detail to the dashboard webview (the ported
    // `/api/health/stream`). HealthResponse is a superset of the route's payload
    // (it also carries `daemon_running`, which the banner ignores).
    let _ = app.emit("health-update", &hr);

    // db_ready and daemon_running both default true when absent (older schema compat).
    let db_ready = hr.database_ready.unwrap_or(false);
    let daemon_running = hr.daemon_running.unwrap_or(true);

    let new_health = if db_ready && daemon_running {
        HealthStatus::Healthy
    } else {
        HealthStatus::Unhealthy
    };

    let (notify_down, notify_back) = {
        let mut s = state.lock().unwrap();
        let now_healthy = new_health == HealthStatus::Healthy;

        let notify_down = if !now_healthy {
            s.consecutive_health_failures += 1;
            // Notify only on the second consecutive failure — one miss is a transient blip.
            s.consecutive_health_failures == 2 && s.daemon_was_healthy
        } else {
            false
        };
        // Fire "back online" only when we had previously sent a "gone quiet" notification
        // (consecutive_health_failures reached 2), so a brief outage during startup is silent.
        let notify_back = now_healthy && s.consecutive_health_failures >= 2;

        if now_healthy {
            s.consecutive_health_failures = 0;
            s.daemon_was_healthy = true;
        }
        s.ui_reachable = true; // health checks are now direct (no HTTP); always reachable
        s.health = new_health;

        (notify_down, notify_back)
    };

    if notify_down && super::notifications_allowed("system.health").await {
        notify(app, "Meridian went quiet.", "Tap to check what happened.");
    } else if notify_back && super::notifications_allowed("system.health").await {
        notify(app, "Back online.", "Picking up where you left off.");
    }
}

/// Fetch the active session and store the app name + elapsed seconds.
pub(super) async fn refresh_active(client: &Client, state: &Arc<Mutex<AppState>>) {
    let resp = client
        .get(format!("{}/api/active", ui_base()))
        .send()
        .await
        .ok();

    let session = match resp {
        None => None,
        Some(r) if !r.status().is_success() => None,
        Some(r) => {
            let ar: ActiveResp = r.json().await.unwrap_or(ActiveResp {
                app_name: None,
                elapsed_s: None,
            });
            ar.app_name.map(|app_name| ActiveSession {
                app_name,
                elapsed_s: ar.elapsed_s.unwrap_or(0),
            })
        }
    };

    state.lock().unwrap().active_session = session;
}

/// Fetch today's totals (focus seconds + switch count) into [`AppState`].
pub(super) async fn refresh_today(client: &Client, state: &Arc<Mutex<AppState>>) {
    let resp = client
        .get(format!("{}/api/today", ui_base()))
        .send()
        .await
        .ok();

    if let Some(r) = resp {
        if let Ok(tr) = r.json::<TodayResp>().await {
            let mut s = state.lock().unwrap();
            if let Some(f) = tr.focus_s {
                s.focus_s = f;
            }
            if let Some(sw) = tr.switch_count {
                s.switch_count = sw;
            }
        }
    }
}

/// Track the drafted-worklog count for the tray tooltip/badge only. The "worklog
/// ready" notification itself is emitted by the daemon's worklog scheduler into
/// the notification outbox and delivered via `drain_notifications` — not here.
pub(super) async fn refresh_worklogs(client: &Client, state: &Arc<Mutex<AppState>>) {
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let resp = client
        .get(format!("{}/api/worklogs?day={}", ui_base(), today))
        .send()
        .await
        .ok();

    let draft_count = match resp {
        None => return,
        Some(r) if !r.status().is_success() => return,
        Some(r) => match r.json::<WorklogsResp>().await {
            Err(_) => return,
            Ok(wr) => wr.items.iter().filter(|i| i.state == "drafted").count() as u32,
        },
    };

    state.lock().unwrap().drafts_count = draft_count;
}
