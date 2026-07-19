//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Headless capture **child process** — screen capture, isolated from the tray.
//!
//! Screen capture drives Apple ScreenCaptureKit (via the vendored
//! `screenpipe-screen`/`sck-rs` stack). A native fault there — e.g. a
//! ScreenCaptureKit reply-block double-free that `libmalloc` turns into an
//! `abort()` — cannot be caught in-process: it kills the whole process. When
//! capture ran inside the tray that meant the tray died with it. Running the
//! engine in a **separate process** restores the blast-radius isolation the old
//! screenpipe daemon gave us: a capture crash ends only this helper, and the
//! parent [`crate::capture_supervisor`] restarts it.
//!
//! This is the exact same capture code the in-process path ran — same
//! ScreenCaptureKit + Apple Vision OCR + a11y walk, same 2 s cadence, writing
//! the same rows to `capture_frames`/`capture_ui_events`. **Capture fidelity is
//! unchanged**; only the process boundary moved.
//!
//! # Lifecycle
//! Spawned as `<current_exe> __capture-helper --db <path>` by
//! [`crate::capture_supervisor::start`]. Runs until:
//! - the parent kills it (pause, or tray shutdown) — SIGKILL/SIGTERM, or
//! - the parent process dies unexpectedly — the ppid watcher below exits so no
//!   orphaned helper keeps capturing after the tray is gone, or
//! - the capture engine returns (error) — we exit non-zero and the supervisor
//!   restarts us with backoff.
//!
//! # Why settings are re-read from disk
//! The in-process engine shared `AppState.capture_ignore`, refreshed live by
//! `update_settings`. A child can't share that memory, but `update_settings`
//! also persists the list to `settings.json`, so this process re-reads that file
//! on a timer and rebuilds the matcher — the same live-refresh, over the file
//! that is already the source of truth. Pause/resume is handled by the parent
//! (it kills/respawns this process), so there is no `capture_paused` flag here.
//!
//! # Related
//! - [`crate::capture_supervisor`] — the parent side that spawns + restarts this.
//! - [`crate::capture`] — the engine + frame/ui-event consumers this drives.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::capture::{screenpipe::ScreenpipeEngine, CaptureEngine};
use crate::capture_ignore::CaptureIgnore;

/// Argument that selects this mode instead of launching the Tauri tray.
pub const HELPER_ARG: &str = "__capture-helper";

/// How often to re-read `settings.json` for live ignore-list changes.
const SETTINGS_POLL: Duration = Duration::from_secs(5);

/// How often the ppid watcher checks whether the parent tray is still alive.
const PARENT_WATCH: Duration = Duration::from_secs(1);

/// Entry point for `<exe> __capture-helper --db <path>`.
///
/// Builds its own Tokio runtime (this process never enters Tauri), runs the
/// capture loop to completion, and exits the process with the resulting code —
/// `0` when the engine ended cleanly, non-zero on engine error, so the
/// supervisor can distinguish "asked to stop" from "crashed" where possible.
/// (A native `abort()` bypasses this entirely and the OS reports the signal.)
pub fn run_headless(db_path: String) -> ! {
    // Minimal console subscriber (RUST_LOG-filtered, default info). This is a
    // separate process, so it needs its own subscriber to be observable; the
    // parent's subscriber does not carry across the process boundary.
    {
        use tracing_subscriber::{fmt, EnvFilter};
        // DIAGNOSTIC: default surfaces the forked screenshot stack's own debug
        // logs (esp. `capture_image` failures, normally at debug) so an SCK
        // screenshot failure is visible without a RUST_LOG override. Revert to a
        // plain "info" default once the OCR/SCK root cause is fixed.
        let default_filter = "info,screenpipe_screen=debug,sck_rs=debug";
        let _ = fmt()
            .with_env_filter(
                EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| EnvFilter::new(default_filter)),
            )
            .try_init();
    }

    let rt = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("capture-helper: failed to build runtime: {e}");
            std::process::exit(70); // EX_SOFTWARE
        }
    };

    let code = rt.block_on(async move { run_capture_loop(db_path).await });
    std::process::exit(code);
}

/// Open the DB, wire the consumers + engine, and run until the engine returns.
/// Returns the process exit code.
async fn run_capture_loop(db_path: String) -> i32 {
    let pool = match meridian_core::open_existing(&db_path).await {
        Ok(p) => p,
        Err(e) => {
            tracing::error!(error = %e, db = %db_path, "capture-helper: cannot open meridian.db");
            return 74; // EX_IOERR
        }
    };

    // Exit if the parent tray goes away, so a crashed/quit tray never leaves an
    // orphaned capture process running (and holding the Screen Recording light).
    spawn_parent_watcher();

    // Seed the ignore list from settings.json and keep it fresh on a timer —
    // the file is the same source `update_settings` writes, so this mirrors the
    // in-process live refresh without shared memory.
    let settings = meridian_core::settings::load_runtime_settings();
    let pause_on_streaming_video = settings.pause_on_streaming_video;
    let capture_ignore = Arc::new(Mutex::new(CaptureIgnore::new(
        &settings.ignored_apps,
        &settings.ignored_urls,
    )));
    spawn_settings_poller(capture_ignore.clone());

    // Frame consumer: drains captured frames, applies the ignore list, writes to
    // capture_frames. Same consumer the tray ran in-process before capture was
    // isolated into this child.
    let (tx, mut rx) = tokio::sync::mpsc::channel::<crate::capture::CapturedFrame>(64);
    let frame_pool = pool.clone();
    let frame_ignore = capture_ignore.clone();
    tokio::spawn(async move {
        while let Some(frame) = rx.recv().await {
            tracing::debug!(
                ts = %frame.timestamp,
                app = ?frame.app_name,
                chars = frame.text.len(),
                "capture: frame received"
            );
            if frame_ignore
                .lock()
                .unwrap()
                .should_drop_frame(frame.app_name.as_deref(), frame.browser_url.as_deref())
            {
                tracing::debug!(app = ?frame.app_name, "capture: frame ignored (user ignore list)");
                continue;
            }
            let row = meridian_core::CaptureFrameInsert {
                timestamp: frame.timestamp,
                app_name: frame.app_name,
                window_name: frame.window_name,
                browser_url: frame.browser_url,
                text: frame.text,
                text_source: frame.text_source.as_str().to_string(),
            };
            if let Err(e) = meridian_core::insert_capture_frame(&frame_pool, &row).await {
                tracing::warn!(error = %e, "capture: failed to persist frame");
            }
        }
    });

    // UI-event consumer: drains input events (keystroke counts, clipboard) and
    // writes to capture_ui_events, applying the app-only ignore match.
    let (ui_tx, mut ui_rx) = tokio::sync::mpsc::channel::<meridian_core::CaptureUiEventInsert>(256);
    let ui_pool = pool.clone();
    let ui_ignore = capture_ignore.clone();
    tokio::spawn(async move {
        while let Some(ev) = ui_rx.recv().await {
            if ui_ignore
                .lock()
                .unwrap()
                .should_drop_app(ev.app_name.as_deref())
            {
                continue;
            }
            if let Err(e) = meridian_core::insert_capture_ui_event(&ui_pool, &ev).await {
                tracing::warn!(error = %e, "capture: failed to persist ui event");
            }
        }
    });
    std::thread::spawn(move || crate::capture::ui_events::run_ui_event_recorder(ui_tx));

    tracing::info!("capture-helper: engine and ui recorder started");

    // Run the engine to completion. It normally loops forever; if it returns
    // Err, exit non-zero so the supervisor restarts us.
    let engine = ScreenpipeEngine {
        pause_on_streaming_video,
    };
    match engine.run(tx).await {
        Ok(()) => {
            tracing::info!("capture-helper: engine ended cleanly");
            0
        }
        Err(e) => {
            tracing::error!(error = %e, "capture-helper: engine exited with error");
            1
        }
    }
}

/// Poll `settings.json` on [`SETTINGS_POLL`] and rebuild the ignore matcher when
/// it changes. Rebuilds only when the file's modified-time advances, so an
/// unchanged file costs a single `stat`.
fn spawn_settings_poller(capture_ignore: Arc<Mutex<CaptureIgnore>>) {
    tokio::spawn(async move {
        let mut last_mtime: Option<std::time::SystemTime> = None;
        let mut ticker = tokio::time::interval(SETTINGS_POLL);
        loop {
            ticker.tick().await;
            let path = meridian_core::settings::settings_json_path();
            let mtime = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
            if mtime == last_mtime {
                continue;
            }
            last_mtime = mtime;
            let s = meridian_core::settings::load_runtime_settings();
            if let Ok(mut ig) = capture_ignore.lock() {
                *ig = CaptureIgnore::new(&s.ignored_apps, &s.ignored_urls);
                tracing::debug!(
                    ignored_apps = s.ignored_apps.len(),
                    ignored_urls = s.ignored_urls.len(),
                    "capture-helper: ignore list refreshed from settings.json"
                );
            }
        }
    });
}

/// Exit the process if the parent tray dies, so no orphaned helper keeps
/// capturing. On Unix a reparented process's ppid becomes 1 (or otherwise
/// differs from the tray that spawned us); either way, once our original parent
/// is gone we stop.
fn spawn_parent_watcher() {
    // Safety: getppid() is always safe — no args, no memory, returns the caller's
    // parent pid.
    let original_ppid = unsafe { libc::getppid() };
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(PARENT_WATCH);
        loop {
            ticker.tick().await;
            let ppid = unsafe { libc::getppid() };
            if ppid != original_ppid || ppid == 1 {
                tracing::info!(
                    original_ppid,
                    ppid,
                    "capture-helper: parent tray gone — exiting to avoid orphaned capture"
                );
                std::process::exit(0);
            }
        }
    });
}
