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
//! - [`install`]     — install-mode + db-path resolution.
//! - [`mlx_server`]  — MLX child-process manager (Approach A bundled venv).
//! - [`sys`]         — shared uid / notify / dashboard-URL helpers.
//! - [`format`]      — duration formatting for the popover.

mod commands;
pub(crate) mod format;
mod install;
pub(crate) mod mlx_server;
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
    let mlx_manager: mlx_server::SharedMlxManager =
        Arc::new(tokio::sync::Mutex::new(mlx_server::MlxManager::new(7823)));

    tauri::Builder::default()
        .plugin(tauri_plugin_positioner::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_opener::init())
        .manage(app_state.clone())
        .manage(mlx_manager.clone())
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

            // Tail the daemon log → `log-tail` events for the dashboard Logs view
            // (the ported `/api/logs/stream`). Independent of the 30 s poll tick
            // so log lines stream at ~1 s.
            poll::spawn_log_tailer(app.handle().clone());

            // Adopt any MLX server that survived from a previous tray run so we
            // don't spawn a duplicate.
            {
                let mlx = mlx_manager.clone();
                tauri::async_runtime::spawn(async move {
                    let home = std::env::var("HOME").unwrap_or_default();
                    mlx_server::reclaim_orphan(&home, 7823, &mlx).await;
                });
            }

            // Auto-open the setup wizard on first launch (no ~/.meridian/onboarded).
            // The 800 ms delay lets the tray menu settle before the window appears.
            {
                let wizard_handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    tokio::time::sleep(tokio::time::Duration::from_millis(800)).await;
                    let home = std::env::var("HOME").unwrap_or_default();
                    if !std::path::Path::new(&format!("{home}/.meridian/onboarded")).exists() {
                        tray::open_wizard_window(&wizard_handle);
                    }
                });
            }

            Ok(())
        })
        // Hand-maintained command list (grouped by domain). Adding a command
        // means: write it in a `commands/` submodule, glob-re-export it in
        // `commands.rs`, AND list it here — a missing entry fails the frontend
        // `invoke` at runtime ("command not found"), not at compile time.
        .invoke_handler(tauri::generate_handler![
            // tray popover + daemon lifecycle
            commands::get_status,
            commands::open_dashboard,
            commands::open_worklogs,
            commands::restart_daemon,
            commands::toggle_daemon,
            commands::get_daemon_status,
            // dashboard DB reads (ported /api/* GETs)
            commands::get_active,
            commands::get_today,
            commands::get_week,
            commands::get_coding_agents,
            commands::get_worklogs,
            commands::get_tasks,
            commands::get_task_detail,
            commands::get_plan,
            commands::get_settings,
            commands::get_triage,
            commands::get_integrations,
            commands::get_notices,
            commands::get_banner_notifications,
            commands::get_health,
            commands::get_openobserve_status,
            commands::get_logs,
            commands::get_ticket_parents,
            commands::get_version,
            // DB writes (ported /api/* POSTs/PATCH/DELETE)
            commands::plan_action,
            commands::triage_decision,
            commands::triage_ignore,
            commands::apply_ticket_fix,
            commands::dismiss_notification,
            commands::delete_notice,
            commands::edit_worklog,
            commands::worklog_action,
            commands::update_settings,
            // process / service control (ported /api process routes)
            commands::reload_daemon,
            commands::set_openobserve,
            commands::sync_tasks,
            commands::run_update,
            // tracker connect/disconnect (ported /api/integrations + /api/auth/oauth)
            commands::disconnect_integration,
            commands::discover_azure_devops,
            commands::start_oauth,
            // OS/window actions
            commands::open_permission_pane,
            // Setup wizard (first-run, permissions, MLX)
            commands::is_first_run,
            commands::mark_setup_complete,
            commands::check_accessibility,
            commands::check_screen_recording,
            commands::get_mlx_status,
            commands::start_mlx_server_cmd,
            commands::download_runtime_cmd,
        ])
        .run(tauri::generate_context!())
        .expect("error running meridian tray");
}
