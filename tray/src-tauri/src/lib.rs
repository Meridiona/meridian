//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Meridian tray — app bootstrap and wiring.
//!
//! This file is intentionally thin: it builds the Tauri app, opens the shared
//! `meridian.db` pool, installs the tray, registers the command surface, and
//! spawns the poll loop. The substance lives in focused modules:
//!
//! - [`commands`] — the `#[tauri::command]` surface (grouped by domain).
//! - [`tray`]     — tray menu construction + menu-event dispatch.
//! - [`poll`]     — the background health/active/today/worklogs poll loop.
//! - [`state`]    — the shared [`state::AppState`] the poll loop maintains.
//! - [`install`]  — install-mode + db-path resolution.
//! - [`sys`]      — shared uid / notify / dashboard-URL helpers.
//! - [`format`]   — duration formatting for the popover.

mod commands;
pub(crate) mod format;
mod install;
mod poll;
mod state;
mod sys;
mod tray;

use state::{AppState, HealthStatus};
use std::sync::{Arc, Mutex};
use tauri::{image::Image, tray::TrayIconBuilder, Manager, WindowEvent};

pub fn run() {
    // Dev-only (`--features otel`): export tray spans to OpenObserve via the
    // daemon's OTLP setup, tagged service.name = meridian-tray. Held for the
    // process lifetime. Compiled out entirely (and `meridian` isn't even a dep)
    // when the feature is off — release builds stay lean.
    //
    // Must run INSIDE Tauri's Tokio runtime: the OTLP batch exporter spawns a
    // background task and panics ("no reactor running") if called before one
    // exists. `block_on` enters the global runtime so the spawn succeeds; the
    // exporter task then lives on that runtime for the process lifetime.
    #[cfg(feature = "otel")]
    let _otel_guard =
        tauri::async_runtime::block_on(async { meridian::observability::init("meridian-tray") })
            .ok();

    let app_state = Arc::new(Mutex::new(AppState::default()));

    tauri::Builder::default()
        .plugin(tauri_plugin_positioner::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_opener::init())
        .manage(app_state.clone())
        .setup(move |app| {
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            // Request OS notification authorization up front. Without this,
            // `.show()` is a silent no-op on macOS until the app has prompted at
            // least once — the reason the health/pause toasts never appeared.
            {
                use tauri_plugin_notification::{NotificationExt, PermissionState};
                let notifier = app.notification();
                if !matches!(notifier.permission_state(), Ok(PermissionState::Granted)) {
                    let _ = notifier.request_permission();
                }
            }

            // Open meridian.db ONCE at startup and share it with commands via
            // managed state (no migrations — the daemon owns the schema). `None`
            // if the DB can't be opened yet, so reads error gracefully instead
            // of crashing the tray.
            let db_path = install::meridian_db_path();
            let db_pool = tauri::async_runtime::block_on(meridian_core::open_existing(&db_path))
                .map_err(|e| eprintln!("tray: meridian.db not opened ({db_path}): {e}"))
                .ok();
            app.manage(db_pool);

            // Single source of truth for the tray menu lives in `tray.rs`, so the
            // poll loop's health-driven rebuild can't drift out of sync. Initial
            // health is Unknown until the first poll resolves it.
            let menu = tray::build_tray_menu(app.handle(), &HealthStatus::Unknown)?;

            let tray_icon_bytes = include_bytes!("../icons/meridiona-mark.png");
            let tray_icon = Image::from_bytes(tray_icon_bytes)?;

            let tray = TrayIconBuilder::new()
                .menu(&menu)
                .show_menu_on_left_click(true)
                .icon(tray_icon)
                .tooltip("Meridian")
                .on_tray_icon_event(|tray_handle, event| {
                    tauri_plugin_positioner::on_tray_event(tray_handle.app_handle(), &event);
                })
                .on_menu_event(|app, event| tray::handle_menu_event(app, event.id.as_ref()))
                .build(app)?;

            {
                let mut s = app_state.lock().unwrap();
                s.tray_id = Some(tray.id().clone());
            }

            // Hide popover on focus loss.
            let main_win = app.get_webview_window("main").unwrap();
            main_win.on_window_event({
                let win = main_win.clone();
                move |event| {
                    if let WindowEvent::Focused(false) = event {
                        let _ = win.hide();
                    }
                }
            });

            let app_handle = app.handle().clone();
            let state_clone = app_state.clone();
            tauri::async_runtime::spawn(async move {
                poll::run_poll_loop(app_handle, state_clone).await;
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_status,
            commands::open_dashboard,
            commands::open_worklogs,
            commands::restart_daemon,
            commands::toggle_daemon,
            commands::get_active,
            commands::get_today,
            commands::get_week,
            commands::get_coding_agents,
            commands::get_worklogs,
            commands::get_tasks,
            commands::get_settings,
            commands::get_triage,
            commands::get_integrations,
            commands::get_daemon_status,
            commands::get_health,
            commands::get_openobserve_status,
            commands::get_logs,
            commands::get_ticket_parents,
            commands::get_version,
            commands::open_permission_pane,
        ])
        .run(tauri::generate_context!())
        .expect("error running meridian tray");
}
