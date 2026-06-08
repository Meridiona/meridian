// meridian — normalises screenpipe activity into structured app sessions
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
struct HealthResp {
    database_ready: Option<bool>,
    daemon_running: Option<bool>,
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
            refresh_worklogs(&app, &client, &state).await;
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

async fn refresh_health(app: &tauri::AppHandle, client: &Client, state: &Arc<Mutex<AppState>>) {
    let resp = client
        .get(format!("{}/api/health", ui_base()))
        .send()
        .await
        .ok()
        .and_then(|r| {
            if r.status().is_success() {
                Some(r)
            } else {
                None
            }
        });

    let (ui_reachable, db_ready, daemon_running) = match resp {
        None => (false, false, false),
        Some(r) => {
            let hr: HealthResp = r.json().await.unwrap_or(HealthResp {
                database_ready: None,
                daemon_running: None,
            });
            // daemon_running defaults to true when the field is absent (older UI build).
            (
                true,
                hr.database_ready.unwrap_or(false),
                hr.daemon_running.unwrap_or(true),
            )
        }
    };

    let new_health = if ui_reachable && db_ready && daemon_running {
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
        s.ui_reachable = ui_reachable;
        s.health = new_health;

        (notify_down, notify_back)
    };

    if notify_down {
        notify(app, "Meridian went quiet.", "Tap to check what happened.");
    } else if notify_back {
        notify(app, "Back online.", "Picking up where you left off.");
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

async fn refresh_worklogs(app: &tauri::AppHandle, client: &Client, state: &Arc<Mutex<AppState>>) {
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

    let notify_new = {
        let mut s = state.lock().unwrap();
        let changed = draft_count > s.last_notified_drafts;
        if draft_count == 0 {
            s.last_notified_drafts = 0;
        }
        s.drafts_count = draft_count;
        if changed {
            s.last_notified_drafts = draft_count;
        }
        changed
    };

    if notify_new {
        let msg = if draft_count == 1 {
            "1 worklog drafted. Worth a look.".to_string()
        } else {
            format!(
                "{} drafts ready — took you hours, took me 30 seconds.",
                draft_count
            )
        };
        notify(app, "Meridian", &msg);
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
    let _ = app.notification().builder().title(title).body(body).show();
}

fn update_toggle_menu(app: &tauri::AppHandle, state: &Arc<Mutex<AppState>>) {
    use tauri::menu::MenuBuilder;
    use tauri::menu::MenuItemBuilder;

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

    let label = match &health {
        HealthStatus::Healthy => "Connected ●",
        HealthStatus::Unhealthy | HealthStatus::Unknown => "Disconnected ○",
    };

    if let Some(id) = tray_id {
        if let Some(tray) = app.tray_by_id(&id) {
            if let Ok(toggle_item) = MenuItemBuilder::with_id("toggle_daemon", label).build(app) {
                if let Ok(open_item) =
                    MenuItemBuilder::with_id("open_dashboard", "Open Dashboard").build(app)
                {
                    if let Ok(worklogs_item) =
                        MenuItemBuilder::with_id("open_worklogs", "Review Drafts").build(app)
                    {
                        if let Ok(restart_item) =
                            MenuItemBuilder::with_id("restart_daemon", "Restart Daemon").build(app)
                        {
                            if let Ok(quit_item) =
                                MenuItemBuilder::with_id("quit", "Quit Meridian Tray").build(app)
                            {
                                if let Ok(menu) = MenuBuilder::new(app)
                                    .items(&[
                                        &toggle_item,
                                        &open_item,
                                        &worklogs_item,
                                        &restart_item,
                                        &quit_item,
                                    ])
                                    .build()
                                {
                                    let _ = tray.set_menu(Some(menu));
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    {
        let mut s = state.lock().unwrap();
        s.last_menu_state = health;
    }
}
