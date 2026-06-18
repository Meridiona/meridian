//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! The tray's background poll loop.
//!
//! Every 30 s tick refreshes a slice of [`AppState`] (active session each tick;
//! health + today every 2nd; worklog drafts every 10th), drains the daemon's
//! notification outbox, pushes the live notice/banner sets to the dashboard
//! webview, then syncs the tray (event emit + tooltip + menu).
//!
//! - [`refresh`] — the per-tick fetch-and-store functions (also emits
//!   `health-update`, the ported `/api/health/stream`).
//! - [`notifications`] — outbox drain + the quiet-hours policy check
//!   ([`notifications_allowed`], re-exported here for [`crate::commands::daemon`]).
//! - [`live`] — the live data → Tauri events that replace the dashboard's SSE
//!   streams: `notices-update`, `notifications-update`, and the `log-tail`
//!   tailer ([`spawn_log_tailer`], started from `lib.rs`, runs at ~1 s
//!   independent of this 30 s loop).
//!
//! The tray-sync helpers (emit / tooltip / menu) stay here, coupled to the loop.

mod live;
mod notifications;
mod refresh;

pub(crate) use live::spawn_log_tailer;
pub(crate) use notifications::notifications_allowed;

use crate::state::{AppState, HealthStatus};
use notifications::drain_notifications;
use refresh::{refresh_active, refresh_health, refresh_today, refresh_worklogs};
use reqwest::Client;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{Emitter, Manager};

const TICK: Duration = Duration::from_secs(30);

pub async fn run_poll_loop(app: tauri::AppHandle, state: Arc<Mutex<AppState>>) {
    let client = Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .expect("reqwest client");
    let mut tick: u32 = 0;
    // Last-emitted JSON snapshots for the live events — emit only on change
    // (mirrors the SSE stores' change-only broadcast).
    let mut last_notices = String::new();
    let mut last_banners = String::new();

    loop {
        // Tick 0, 1, 2… every 30s.
        // Active: every tick (30s)
        // Health + today: every 2nd tick (60s)
        // Worklogs: every 10th tick (5 min)
        let do_health = tick.is_multiple_of(2);
        let do_worklogs = tick.is_multiple_of(10);

        if do_health {
            refresh_health(&app, &client, &state).await;
        }
        refresh_active(&client, &state).await;
        if do_health {
            refresh_today(&client, &state).await;
        }
        if do_worklogs {
            refresh_worklogs(&client, &state).await;
        }
        // Drain the daemon's notification outbox every tick — this is the single
        // delivery path for all daemon-originated notifications (plan nudge,
        // worklog ready, promoted faults). The tray is a dumb delivery agent;
        // preference + quiet-hours filtering already happened server-side.
        drain_notifications(&app).await;

        // Push live notices + banner notifications to the dashboard webview
        // (the ported SSE streams). Skipped silently when the DB isn't open.
        if let Some(pool) = app
            .try_state::<Option<meridian_core::SqlitePool>>()
            .and_then(|s| s.inner().clone())
        {
            live::emit_notices(&app, &pool, &mut last_notices).await;
            live::emit_banners(&app, &pool, &mut last_banners).await;
        }

        {
            let mut s = state.lock().unwrap();
            s.last_poll = Some(Instant::now());
        }

        emit_update(&app, &state);
        update_tray_icon(&app, &state);
        update_toggle_menu(&app, &state);

        tokio::time::sleep(TICK).await;
        tick = tick.wrapping_add(1);
    }
}

fn emit_update(app: &tauri::AppHandle, state: &Arc<Mutex<AppState>>) {
    let payload = state.lock().unwrap().to_payload();
    let _ = app.emit("status-update", payload);
}

fn update_tray_icon(app: &tauri::AppHandle, state: &Arc<Mutex<AppState>>) {
    let (health, drafts, tray_id) = {
        let s = state.lock().unwrap();
        (s.health.clone(), s.drafts_count, s.tray_id.clone())
    };

    let tooltip = match &health {
        HealthStatus::Healthy if drafts > 0 => {
            format!(
                "Meridian — {} draft{} waiting",
                drafts,
                if drafts == 1 { "" } else { "s" }
            )
        }
        HealthStatus::Healthy => "Meridian — everything's running.".to_string(),
        HealthStatus::Unhealthy => "Meridian — gone quiet.".to_string(),
        HealthStatus::Unknown => "Meridian".to_string(),
    };

    if let Some(id) = tray_id {
        if let Some(tray) = app.tray_by_id(&id) {
            let _ = tray.set_tooltip(Some(&tooltip));
        }
    }
}

fn update_toggle_menu(app: &tauri::AppHandle, state: &Arc<Mutex<AppState>>) {
    let (health, tray_id, last_menu_state) = {
        let s = state.lock().unwrap();
        (
            s.health.clone(),
            s.tray_id.clone(),
            s.last_menu_state.clone(),
        )
    };

    if health == last_menu_state {
        return;
    }

    // Rebuild via the single source of truth in lib.rs so this health-driven
    // refresh always carries the full item set (it used to hardcode a 5-item
    // menu here and silently drop "Open Dashboard (native)").
    if let Some(id) = tray_id {
        if let Some(tray) = app.tray_by_id(&id) {
            if let Ok(menu) = crate::tray::build_tray_menu(app, &health) {
                let _ = tray.set_menu(Some(menu));
            }
        }
    }

    {
        let mut s = state.lock().unwrap();
        s.last_menu_state = health;
    }
}
