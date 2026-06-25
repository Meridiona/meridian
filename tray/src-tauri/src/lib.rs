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

mod backend_install;
#[cfg(feature = "capture")]
mod capture;
mod commands;
pub(crate) mod format;
mod install;
pub(crate) mod mlx_server;
mod poll;
mod state;
mod sys;
mod tray;
mod tray_icon;
mod update;

use state::{AppState, HealthStatus};
use std::sync::{Arc, Mutex};
#[cfg(not(target_os = "macos"))]
use tauri::WindowEvent;
use tauri::{
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Emitter, Manager,
};

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

    // Capture builds without otel have no subscriber, so the `capture: …` logs
    // would be invisible. Install a console fmt subscriber (RUST_LOG-filtered,
    // default info) so a `cargo run --features capture` runtime check is
    // observable. Skipped under otel (which installs its own subscriber).
    #[cfg(all(feature = "capture", not(feature = "otel")))]
    {
        use tracing_subscriber::{fmt, EnvFilter};
        let _ = fmt()
            .with_env_filter(
                EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
            )
            .try_init();
    }

    let app_state = Arc::new(Mutex::new(AppState::default()));
    let mlx_manager: mlx_server::SharedMlxManager =
        Arc::new(tokio::sync::Mutex::new(mlx_server::MlxManager::new(7823)));

    tauri::Builder::default()
        .plugin(tauri_plugin_positioner::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_opener::init())
        // DMG auto-update: reads endpoint + minisign pubkey from tauri.conf.json.
        // Registered unconditionally; the check is a no-op in a source/dev run
        // (the running binary isn't a packaged `.app` for the updater to swap).
        .plugin(tauri_plugin_updater::Builder::new().build())
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
            // Capture (slice 4a) writes to the SAME read-write pool the commands
            // use — clone the handle before it's moved into managed state.
            #[cfg(feature = "capture")]
            let capture_pool = db_pool.clone();
            app.manage(db_pool);

            // Single source of truth for the tray menu lives in `tray.rs`, so the
            // poll loop's health-driven rebuild can't drift out of sync. Initial
            // health is Unknown until the first poll resolves it.
            let menu = tray::build_tray_menu(app.handle(), &HealthStatus::Unknown)?;

            // Start as an un-filled progress ring (the design's menu-bar glyph);
            // the 1 s ticker swaps in the filled version once a task percentage
            // is known. Rendered as a template so macOS tints it to the menu bar.
            let tray_icon = tray_icon::ring_image(None);

            // Left-click toggles the popover (positioned under the tray icon);
            // right-click still opens the native menu. `show_menu_on_left_click`
            // must be false so the left-click reaches our handler instead of
            // auto-opening the menu and swallowing the popover.
            let tray = TrayIconBuilder::new()
                .menu(&menu)
                .show_menu_on_left_click(false)
                .icon(tray_icon)
                // Monochrome mark → render as a template so macOS tints it to the
                // light/dark menu bar instead of showing it full-colour.
                .icon_as_template(true)

                .on_tray_icon_event(|tray_handle, event| {
                    let app = tray_handle.app_handle();
                    match &event {
                        TrayIconEvent::Click {
                            button: MouseButton::Left,
                            button_state: MouseButtonState::Up,
                            rect,
                            ..
                        } => {
                            tracing::info!("tray.event: Click");
                            // Hide the hover tooltip on click (popover takes over).
                            if let Some(tt) = app.get_webview_window("tray-tooltip") {
                                let _ = tt.hide();
                            }
                            if let Some(win) = app.get_webview_window("main") {
                                let visible = win.is_visible().unwrap_or(false);
                                tracing::info!(popover_visible = visible, "tray.click: toggling popover");
                                if visible {
                                    let _ = win.hide();
                                } else {
                                    #[cfg(target_os = "macos")]
                                    make_visible_over_fullscreen(&win);
                                    // Position the popover directly below the tray icon
                                    // using the click rect — same approach as the tooltip.
                                    // tauri-plugin-positioner's TrayCenter placed the window
                                    // overlapping the menu bar on macOS 14+.
                                    let pop_w = 384_i32;
                                    let icon_pos = rect.position.to_physical::<i32>(1.0);
                                    let icon_size = rect.size.to_physical::<i32>(1.0);
                                    let x = (icon_pos.x + icon_size.width / 2 - pop_w / 2).max(0);
                                    let y = icon_pos.y + icon_size.height;
                                    let _ = win.set_position(tauri::Position::Physical(
                                        tauri::PhysicalPosition::new(x, y),
                                    ));
                                    // Use orderFrontRegardless (same as the native right-click
                                    // menu) so the popover appears in the current Space without
                                    // signalling a Space switch. makeKeyAndOrderFront (what
                                    // win.show() calls) causes macOS to switch back to the home
                                    // Space when triggered from a fullscreen Space. Clicking
                                    // inside the popover naturally makes it key, so
                                    // Focused(false) auto-dismiss still works.
                                    #[cfg(target_os = "macos")]
                                    show_no_focus(&win);
                                    #[cfg(not(target_os = "macos"))]
                                    { let _ = win.show(); let _ = win.set_focus(); }
                                    tracing::info!(
                                        x, y,
                                        size = ?win.inner_size().ok(),
                                        "tray.click: popover shown"
                                    );
                                }
                            }
                        }
                        TrayIconEvent::Enter { rect, .. } => {
                            tracing::info!("tray.event: Enter");
                            // Only show the tooltip when the main popover is closed.
                            let popover_open = app
                                .get_webview_window("main")
                                .map(|w| w.is_visible().unwrap_or(false))
                                .unwrap_or(false);
                            if popover_open {
                                tracing::info!("tray.enter: popover already open — tooltip suppressed");
                                return;
                            }
                            if let Some(tt) = app.get_webview_window("tray-tooltip") {
                                // Position the tooltip centred below the tray icon.
                                // scale_factor 1.0 because the tray_icon crate already
                                // gives us physical pixel coords before the dpi wrapper.
                                let tt_w = 300_i32;
                                let icon_pos = rect.position.to_physical::<i32>(1.0);
                                let icon_size = rect.size.to_physical::<i32>(1.0);
                                let x = (icon_pos.x + icon_size.width / 2 - tt_w / 2).max(0);
                                let y = icon_pos.y + icon_size.height;
                                let _ = tt.set_position(tauri::Position::Physical(
                                    tauri::PhysicalPosition::new(x, y),
                                ));
                                #[cfg(target_os = "macos")]
                                make_visible_over_fullscreen(&tt);
                                #[cfg(target_os = "macos")]
                                show_no_focus(&tt);
                                #[cfg(not(target_os = "macos"))]
                                let _ = tt.show();
                                tracing::info!(x, y, "tray.enter: tooltip shown");
                                // Push the latest status so the tooltip renders fresh data.
                                // Fall back to a default payload if the state lock is
                                // poisoned rather than panicking this tray-event handler.
                                let _ = app.emit("status-update",
                                    app.try_state::<Arc<Mutex<AppState>>>()
                                        .and_then(|s| s.inner().lock().ok().map(|g| g.to_payload()))
                                        .unwrap_or_else(|| AppState::default().to_payload()),
                                );
                            }
                        }
                        TrayIconEvent::Leave { .. } => {
                            tracing::info!("tray.event: Leave — hiding tooltip");
                            if let Some(tt) = app.get_webview_window("tray-tooltip") {
                                let _ = tt.hide();
                            }
                        }
                        _ => {}
                    }
                })
                .on_menu_event(|app, event| tray::handle_menu_event(app, event.id.as_ref()))
                .build(app)?;

            {
                let mut s = app_state.lock().unwrap();
                s.tray_id = Some(tray.id().clone());
            }

            // Convert the popover + tooltip to NSPanel with non-activating style,
            // then raise their collection behavior and level so they appear in
            // fullscreen Spaces.
            //
            // Why NSPanel is necessary: a plain NSWindow (what Tauri creates) that
            // calls makeKeyAndOrderFront causes macOS to switch back to the home Space
            // before displaying the window, defeating fullscreen support. NSPanel +
            // NSWindowStyleMaskNonactivatingPanel never steals key-window status so
            // the fullscreen app's Space stays active and the panel appears within it.
            // NSPanel is a direct NSWindow subclass with identical ivar layout;
            // object_setClass between them is safe (the same technique used by
            // tauri-nspanel). The Tauri IPC bridge (WKWebView + __TAURI__) is
            // unaffected — it lives inside the view, not the window class.
            #[cfg(target_os = "macos")]
            for label in ["main", "tray-tooltip"] {
                if let Some(win) = app.get_webview_window(label) {
                    init_as_nspanel(&win);
                    make_visible_over_fullscreen(&win);
                }
            }

            // Live menu-bar pill: tick once a second and render the design's
            // "MER-142 · 2:05:11" — the current task key + the running session
            // elapsed (extrapolated from the last poll so it counts smoothly),
            // with the icon a progress ring filled to the task's completion.
            // The title clears when nothing is tracked / the daemon is down; the
            // ring tracks task progress regardless. Both write only on change to
            // avoid hammering the native item (icon keyed on its 1 % bucket).
            {
                let title_app = app.handle().clone();
                let title_state = app_state.clone();
                tauri::async_runtime::spawn(async move {
                    let mut last_title: Option<String> = None;
                    let mut last_bucket: i32 = i32::MIN; // -1 = un-filled; MIN = uninitialised
                    let mut ticker = tokio::time::interval(std::time::Duration::from_secs(1));
                    loop {
                        ticker.tick().await;
                        let (tray_id, title, bucket) = {
                            let s = title_state.lock().unwrap();
                            let timer = match (&s.active_session, s.health == HealthStatus::Healthy)
                            {
                                (Some(a), true) => {
                                    let extra =
                                        s.active_set_at.map(|t| t.elapsed().as_secs()).unwrap_or(0);
                                    Some(format::format_timer(a.elapsed_s + extra))
                                }
                                _ => None,
                            };
                            let title = match (s.current_task_key.as_ref(), timer) {
                                (Some(k), Some(t)) => Some(format!("{k} · {t}")),
                                (None, Some(t)) => Some(t),
                                _ => None,
                            };
                            // 1 %-resolution bucket; -1 means "no percentage → un-filled ring".
                            let bucket = match s.task_percent {
                                Some(p) => (p.clamp(0.0, 1.0) * 100.0).round() as i32,
                                None => -1,
                            };
                            (s.tray_id.clone(), title, bucket)
                        };
                        let Some(id) = tray_id else { continue };
                        let Some(tray) = title_app.tray_by_id(&id) else {
                            continue;
                        };
                        if title != last_title {
                            let _ = tray.set_title(title.as_deref());
                            last_title = title;
                        }
                        if bucket != last_bucket {
                            let pct = (bucket >= 0).then(|| bucket as f64 / 100.0);
                            if tray.set_icon(Some(tray_icon::ring_image(pct))).is_ok() {
                                // Runtime set_icon drops the template flag; re-arm it.
                                if let Err(e) = tray.set_icon_as_template(true) {
                                    tracing::warn!(
                                        error = %e,
                                        "tray: set_icon_as_template failed — ring may render full-colour"
                                    );
                                }
                            }
                            last_bucket = bucket;
                        }
                    }
                });
            }

            // The popover is a non-activating NSPanel — it never becomes key so
            // Focused(false) never fires. Dismiss paths:
            //   • click-outside → global NSEvent monitor (macOS)
            //   • tray-icon click → toggle in on_tray_icon_event above
            //   • Escape key → app.js keydown → invoke('hide_popover')
            #[cfg(target_os = "macos")]
            if let Some(main_win) = app.get_webview_window("main") {
                install_click_outside_monitor(main_win);
            }
            #[cfg(not(target_os = "macos"))]
            {
                let main_win = app.get_webview_window("main").unwrap();
                main_win.on_window_event({
                    let win = main_win.clone();
                    move |event| {
                        if let WindowEvent::Focused(false) = event {
                            let _ = win.hide();
                        }
                    }
                });
            }

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

            // Stage + register the bundled backend (daemon + a11y-helper) on the
            // self-contained .app DMG path. No-op under dev/source (no bundled
            // Resources/backend) and on launches where the binary is unchanged.
            // Spawned off the setup hook because the launchd bootout-wait can
            // take several seconds and must not block tray startup.
            {
                let backend_handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    backend_install::ensure_backend_installed(&backend_handle).await;
                });
            }

            // In-process capture (Gap-2 Bucket 2, behind the `capture` feature).
            // Spawns the screenpipe-screen frame engine + the input recorder, each
            // with its own consumer, on isolated tasks — a capture panic ends only
            // that task, never the tray (we gave up the screenpipe daemon's process
            // isolation, so this matters). Frames → capture_frames (slice 4a),
            // input events → capture_ui_events (slice 3c).
            #[cfg(feature = "capture")]
            {
                use capture::{screenpipe::ScreenpipeEngine, CaptureEngine};

                // Guard: only start the capture engine when Screen Recording is
                // already granted. Spawning ScreenCaptureKit without the grant
                // triggers the macOS system dialog on every launch — including the
                // restart after the user enables it — causing repeated prompts
                // during the setup wizard. `CGPreflightScreenCaptureAccess` is a
                // pure status read (no prompt, no side effects). If the permission
                // is absent we log and skip; after the user grants it in the wizard
                // and restarts, this preflight passes and capture starts normally.
                #[link(name = "CoreGraphics", kind = "framework")]
                extern "C" {
                    fn CGPreflightScreenCaptureAccess() -> bool;
                }
                let screen_granted = unsafe { CGPreflightScreenCaptureAccess() };

                if !screen_granted {
                    tracing::info!("capture: Screen Recording not granted — engine deferred until next launch after grant");
                } else {
                    let (tx, mut rx) = tokio::sync::mpsc::channel::<capture::CapturedFrame>(64);
                    // Persist each frame into meridian.db's capture_frames (slice 4a).
                    // Low-rate writer (~1 row / 2 s) sharing the commands' RW pool;
                    // the 5 s busy_timeout serializes it against the daemon's writes.
                    // No-op when the pool is absent or the table isn't migrated yet.
                    let consumer_pool = capture_pool.clone();
                    tauri::async_runtime::spawn(async move {
                        while let Some(frame) = rx.recv().await {
                            tracing::debug!(
                                ts = %frame.timestamp,
                                app = ?frame.app_name,
                                window = ?frame.window_name,
                                url = ?frame.browser_url,
                                chars = frame.text.len(),
                                source = frame.text_source.as_str(),
                                "capture: frame received"
                            );
                            let Some(pool) = consumer_pool.as_ref() else {
                                continue;
                            };
                            let row = meridian_core::CaptureFrameInsert {
                                timestamp: frame.timestamp,
                                app_name: frame.app_name,
                                window_name: frame.window_name,
                                browser_url: frame.browser_url,
                                text: frame.text,
                                text_source: frame.text_source.as_str().to_string(),
                            };
                            if let Err(e) = meridian_core::insert_capture_frame(pool, &row).await {
                                tracing::warn!(error = %e, "capture: failed to persist frame");
                            }
                        }
                    });
                    tauri::async_runtime::spawn(async move {
                        if let Err(e) = ScreenpipeEngine.run(tx).await {
                            tracing::error!(error = %e, "capture: engine exited with error");
                        }
                    });

                    // Input recorder (slice 3c): ui events → capture_ui_events. The
                    // recorder is blocking (its own CGEventTap thread + a crossbeam
                    // Receiver we poll), so it runs on a dedicated OS thread and
                    // forwards mapped rows over a tokio channel to this async writer.
                    let (ui_tx, mut ui_rx) =
                        tokio::sync::mpsc::channel::<meridian_core::CaptureUiEventInsert>(256);
                    let ui_pool = capture_pool;
                    tauri::async_runtime::spawn(async move {
                        while let Some(ev) = ui_rx.recv().await {
                            let Some(pool) = ui_pool.as_ref() else {
                                continue;
                            };
                            if let Err(e) = meridian_core::insert_capture_ui_event(pool, &ev).await {
                                tracing::warn!(error = %e, "capture: failed to persist ui event");
                            }
                        }
                    });
                    std::thread::spawn(move || capture::ui_events::run_ui_event_recorder(ui_tx));
                }
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

            // No silent auto-install on launch: the DMG update surfaces as an
            // in-app banner (sidebar + popover) that checks on open via the
            // `check_update` command, so the user sees + consents to the update
            // rather than the app restarting itself underneath them.

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
            commands::open_setup,
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
            commands::check_update,
            commands::install_update,
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
            commands::get_oauth_status,
            // OS/window actions
            commands::open_permission_pane,
            commands::quit_app,
            commands::hide_popover,
            // Setup wizard (first-run, permissions, MLX)
            commands::is_first_run,
            commands::mark_setup_complete,
            commands::check_accessibility,
            commands::check_screen_recording,
            commands::request_screen_recording,
            commands::check_input_monitoring,
            commands::request_input_monitoring,
            commands::get_mlx_status,
            commands::start_mlx_server_cmd,
            commands::download_runtime_cmd,
            commands::prefetch_model_cmd,
            commands::detect_system_specs,
            commands::set_model_preference,
            commands::get_model_preference,
            tray_debug,
        ])
        .run(tauri::generate_context!())
        .expect("error running meridian tray");
}

/// Debug bridge: lets the popover/tooltip JS forward `window.onerror` reports and
/// the measured card height to the tray's stderr log, so GUI-only faults (a JS
/// exception, a window/card size mismatch) are diagnosable without devtools. The
/// injected `window` names the caller and exposes its actual size for comparison.
#[tauri::command]
fn tray_debug(window: tauri::Window, msg: String) {
    tracing::info!(
        label = %window.label(),
        size = ?window.inner_size().ok(),
        msg = %msg,
        "tray_debug"
    );
}

/// Set the window's macOS `collectionBehavior` and level so it renders over
/// another app's full-screen Space, not just normal Spaces.
///
/// `WebviewWindow::set_visible_on_all_workspaces(true)` (tao) only OR-s in
/// `NSWindowCollectionBehaviorCanJoinAllSpaces`. A window over a full-screen app
/// also needs `NSWindowCollectionBehaviorFullScreenAuxiliary`, which tao never
/// sets — so the popover/tooltip silently fail to appear when a full-screen app
/// owns the active Space. We send `setCollectionBehavior:` directly, OR-ing both
/// flags onto whatever is already there, and raise the window level to
/// `NSPopUpMenuWindowLevel` (101) so it sits above full-screen app content.
/// `NSStatusWindowLevel` (25) is above the menu bar on normal Spaces but can sit
/// *below* a full-screen app's compositor layer — pop-up menu level is the safe
/// choice and is what Spotlight / Alfred / 1Password mini use.
/// Must run on the main thread (the `setup` hook and tray-event handlers do).
#[cfg(target_os = "macos")]
fn make_visible_over_fullscreen(win: &tauri::WebviewWindow) {
    use objc2::{msg_send, runtime::AnyObject};

    // AppKit NSWindowCollectionBehavior bit flags (stable since 10.x).
    const CAN_JOIN_ALL_SPACES: usize = 1 << 0;
    const FULL_SCREEN_AUXILIARY: usize = 1 << 8;
    // NSPopUpMenuWindowLevel (101): above all normal app content and above
    // full-screen app compositor layers. NSStatusWindowLevel (25) is not
    // reliably above full-screen content on macOS 14+.
    const NS_POPUP_MENU_WINDOW_LEVEL: isize = 101;

    let ptr = match win.ns_window() {
        Ok(p) if !p.is_null() => p as *const AnyObject,
        _ => {
            tracing::warn!(label = %win.label(), "make_visible_over_fullscreen: ns_window unavailable");
            return;
        }
    };
    // Safety: `ptr` is a live NSWindow for the lifetime of this call (we hold
    // `win`), and we are on the main thread. `collectionBehavior` /
    // `setCollectionBehavior:` / `setLevel:` are NSUInteger/NSInteger get/sets
    // with no ownership transfer.
    unsafe {
        let ns = &*ptr;
        let current: usize = msg_send![ns, collectionBehavior];
        let next = current | CAN_JOIN_ALL_SPACES | FULL_SCREEN_AUXILIARY;
        let _: () = msg_send![ns, setCollectionBehavior: next];
        let _: () = msg_send![ns, setLevel: NS_POPUP_MENU_WINDOW_LEVEL];
        tracing::info!(
            label = %win.label(),
            behavior_before = current,
            behavior_after = next,
            level = NS_POPUP_MENU_WINDOW_LEVEL,
            "make_visible_over_fullscreen: applied"
        );
    }
}

/// Show a window without stealing focus from the currently active app.
///
/// `WebviewWindow::show()` calls `makeKeyAndOrderFront:` which signals the
/// active app to deactivate — if the active app is in full-screen this can
/// cause macOS to switch away from its Space before our window appears.
/// `orderFrontRegardless` shows the window at its current level without
/// changing the key-window status, so the full-screen app stays active.
/// For the NSPanel popover this is complementary to the non-activating mask
/// (belt-and-suspenders): the mask prevents key-window steal, and this call
/// prevents the ordering operation from triggering a Space switch.
#[cfg(target_os = "macos")]
fn show_no_focus(win: &tauri::WebviewWindow) {
    use objc2::{msg_send, runtime::AnyObject};
    match win.ns_window() {
        Ok(p) if !p.is_null() => unsafe {
            let ns = &*(p as *const AnyObject);
            let _: () = msg_send![ns, orderFrontRegardless];
        },
        _ => {
            let _ = win.show();
        }
    }
}

/// Convert an NSWindow to a non-activating NSPanel for fullscreen Space support.
///
/// A plain NSWindow shown via `makeKeyAndOrderFront:` activates the app, which
/// macOS interprets as "switch to this app's Space" — causing a Space switch
/// away from a fullscreen app. `NSWindowStyleMaskNonactivatingPanel` (bit 7)
/// prevents the panel from ever becoming key or activating the app, so the
/// fullscreen app's Space stays active and the panel appears within it when
/// we call `orderFrontRegardless` (see `show_no_focus`).
///
/// NSPanel is a direct `NSWindow` subclass with identical ivar layout;
/// `object_setClass` between them is safe (same technique as `tauri-nspanel`).
/// The WKWebView IPC bridge is unaffected — it lives inside the view, not the
/// window class.
///
/// `hidesOnDeactivate: NO` keeps the panel visible when the Accessory-policy
/// process briefly backgrounds (otherwise it flickers).
///
/// Since a non-activating panel never becomes key, `Focused(false)` never fires.
/// Click-outside dismiss is handled instead by a global `NSEvent` monitor
/// installed via `install_click_outside_monitor`.
///
/// Must be called in the `setup` hook before the window is ever shown.
#[cfg(target_os = "macos")]
fn init_as_nspanel(win: &tauri::WebviewWindow) {
    use objc2::{class, msg_send, runtime::AnyObject};

    extern "C" {
        fn object_setClass(
            obj: *mut AnyObject,
            cls: *const objc2::runtime::AnyClass,
        ) -> *const objc2::runtime::AnyClass;
    }

    let ptr = match win.ns_window() {
        Ok(p) if !p.is_null() => p as *mut AnyObject,
        _ => {
            tracing::warn!(label = %win.label(), "init_as_nspanel: ns_window unavailable");
            return;
        }
    };
    // Safety: NSPanel is a direct NSWindow subclass with identical ivar layout.
    // object_setClass between them is safe before the window is first shown.
    // We are on the main thread (setup hook). No ownership transfer.
    unsafe {
        object_setClass(ptr, class!(NSPanel));
        let ns = &*ptr;
        // NSWindowStyleMaskNonactivatingPanel = 1 << 7.
        let current_mask: usize = msg_send![ns, styleMask];
        let _: () = msg_send![ns, setStyleMask: current_mask | (1usize << 7)];
        let _: () = msg_send![ns, setHidesOnDeactivate: false];
        tracing::info!(
            label = %win.label(),
            new_mask = current_mask | (1usize << 7),
            "init_as_nspanel: converted to non-activating NSPanel"
        );
    }
}

/// Install a global `NSEvent` monitor that hides the popover when the user
/// clicks outside it (in any other app or window).
///
/// `addGlobalMonitorForEventsMatchingMask:handler:` fires for mouse-down events
/// delivered to OTHER processes — it does NOT fire for clicks inside our own
/// windows. This makes it a clean "click outside" detector: any click that
/// doesn't land in the popover will hide it.
///
/// This replaces `Focused(false)` / `windowDidResignKey` which never fires for
/// a non-activating NSPanel (the panel never becomes key in the first place).
///
/// The block is leaked and the monitor runs for the app's lifetime. CPU impact
/// is negligible: the closure body only runs when the popover is visible.
#[cfg(target_os = "macos")]
fn install_click_outside_monitor(win: tauri::WebviewWindow) {
    use block2::RcBlock;
    use objc2::{class, msg_send, runtime::AnyObject};

    // NSLeftMouseDown (1<<1) | NSRightMouseDown (1<<3)
    let mask: u64 = (1u64 << 1) | (1u64 << 3);

    let block = RcBlock::new(move |_event: *mut AnyObject| {
        if win.is_visible().unwrap_or(false) {
            let _ = win.hide();
        }
    });

    // Leak the RcBlock so it lives for the app's lifetime.
    // RcBlock<Dyn>: Deref<Target = Block<Dyn>>; the coercion gives &Block<Dyn>
    // which implements RefEncode (encoding: @? = block pointer).
    let block_ref: &'static block2::Block<dyn Fn(*mut AnyObject)> = Box::leak(Box::new(block));

    unsafe {
        let _monitor: *mut AnyObject = msg_send![
            class!(NSEvent),
            addGlobalMonitorForEventsMatchingMask: mask,
            handler: block_ref,
        ];
        tracing::info!("install_click_outside_monitor: global NSEvent monitor installed");
    }
}
