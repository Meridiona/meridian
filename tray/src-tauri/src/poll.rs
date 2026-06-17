//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
use crate::health::check_health;
use crate::state::{ActiveSession, AppState, HealthStatus};
use reqwest::Client;
use serde::Deserialize;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::Emitter;
use tauri_plugin_notification::NotificationExt;

const TICK: Duration = Duration::from_secs(30);

fn ui_base() -> String {
    let port = std::env::var("MERIDIAN_UI_PORT").unwrap_or_else(|_| "3939".to_string());
    format!("http://127.0.0.1:{}", port)
}

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

#[derive(Deserialize)]
struct PendingNotif {
    id: i64,
    title: String,
    body: String,
}

pub async fn run_poll_loop(app: tauri::AppHandle, state: Arc<Mutex<AppState>>) {
    let client = Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .expect("reqwest client");
    let mut tick: u32 = 0;

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
        drain_notifications(&app, &client).await;

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

async fn refresh_health(app: &tauri::AppHandle, _client: &Client, state: &Arc<Mutex<AppState>>) {
    let hr = check_health().await;

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

    if notify_down && notifications_allowed("system.health").await {
        notify(app, "Meridian went quiet.", "Tap to check what happened.");
    } else if notify_back && notifications_allowed("system.health").await {
        notify(app, "Back online.", "Picking up where you left off.");
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

async fn refresh_active(client: &Client, state: &Arc<Mutex<AppState>>) {
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

async fn refresh_today(client: &Client, state: &Arc<Mutex<AppState>>) {
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

// Tracks the draft count for the tray tooltip/badge only. The "worklog ready"
// notification itself is emitted by the daemon's worklog scheduler into the
// notification outbox and delivered via drain_notifications — not fired here.
async fn refresh_worklogs(client: &Client, state: &Arc<Mutex<AppState>>) {
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

// Poll the daemon's notification outbox and deliver each pending native
// notification as a macOS toast, then acknowledge it so it never re-fires.
async fn drain_notifications(app: &tauri::AppHandle, client: &Client) {
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

fn notify(app: &tauri::AppHandle, title: &str, body: &str) {
    // v1: the native macOS toast shows title + body only. Producers populate a
    // `deep_link` (e.g. /plan, /worklogs) and the in-app banner channel renders
    // it as an "Open →" link, but click-to-navigate on a native toast needs
    // Tauri notification actions + a focus/navigate handler — deferred. The two
    // channels are intentionally asymmetric here; the banner carries the link.
    let _ = app.notification().builder().title(title).body(body).show();
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
            if let Ok(menu) = crate::build_tray_menu(app, &health) {
                let _ = tray.set_menu(Some(menu));
            }
        }
    }

    {
        let mut s = state.lock().unwrap();
        s.last_menu_state = health;
    }
}
