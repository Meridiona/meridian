//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Parent-side supervisor for the isolated capture child process.
//!
//! Spawns `<current_exe> __capture-helper --db <path>` (see [`crate::capture_child`])
//! and keeps it running: if the helper dies — including a native
//! ScreenCaptureKit `abort()` that no in-process handler could catch — the tray
//! itself is untouched, and this supervisor restarts the helper with backoff.
//! This restores the blast-radius isolation the in-process capture cutover gave
//! up (the [`crate::capture::CaptureEngine`] trait doc calls this out: "capture
//! must never crash the tray").
//!
//! Capture fidelity is unchanged — the child runs the identical engine. Only the
//! process boundary is new.
//!
//! # Pause / resume
//! There is no separate stop path: pausing drops [`crate::state::AppState::capture_cancel`],
//! which signals the supervisor to kill its child and exit; resuming calls
//! [`start`] again, which spawns a fresh supervised child. The gap-row accounting
//! stays entirely in [`crate::commands::pause`] — unchanged.
//!
//! # Crash-loop guard
//! A helper that dies immediately every time (e.g. a ScreenCaptureKit fault that
//! reproduces on every launch) would otherwise spin. We track restarts in a
//! rolling window; past [`CRASH_LIMIT`] within [`CRASH_WINDOW`] we back off to
//! [`MAX_BACKOFF`] and raise a `capture.unavailable` notice so the "no data"
//! state is visible rather than silent — then keep retrying slowly so capture
//! recovers on its own once the underlying condition clears.
//!
//! # Related
//! - [`crate::capture_child`] — the child this supervises.
//! - [`crate::commands::pause`] — pause/resume, which drives the cancel handle.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use meridian_core::SqlitePool;
use tokio::process::Command;
use tokio::sync::oneshot;

use crate::capture_child::HELPER_ARG;
use crate::state::AppState;

/// Shortest wait before restarting a helper that just died.
const MIN_BACKOFF: Duration = Duration::from_secs(1);
/// Longest wait between restarts (also the crash-loop cool-down).
const MAX_BACKOFF: Duration = Duration::from_secs(30);
/// Rolling window over which rapid restarts count toward the crash-loop guard.
const CRASH_WINDOW: Duration = Duration::from_secs(60);
/// A child that stays up at least this long is treated as a healthy session,
/// resetting the backoff + crash counters.
const HEALTHY_RUN: Duration = Duration::from_secs(60);
/// More than this many restarts within [`CRASH_WINDOW`] trips the crash-loop guard.
const CRASH_LIMIT: usize = 5;

/// Start (or restart) the supervised capture child.
///
/// Idempotent: dropping any previous [`AppState::capture_cancel`] first, so a
/// stale supervisor + its child are torn down before a new one starts (matches
/// the old in-process `start_capture` contract, which pause/resume relies on).
/// Does nothing when Screen Recording is not granted (logs and returns) — same
/// as before, no point spawning a helper that cannot capture.
///
/// `pool` is used only to raise the `capture.unavailable` notice on crash-loop;
/// the child opens its own pool to the same `meridian.db` (WAL is multi-process
/// safe, exactly as the daemon + tray already share it).
///
/// # Who calls this
/// `lib.rs`'s setup hook on launch, [`crate::commands::pause`]'s resume, and the
/// schedule-resume path in [`crate::poll`].
pub(crate) fn start(app_state: Arc<Mutex<AppState>>, pool: Option<SqlitePool>) {
    // Don't spawn a helper that can't capture. Preflight is macOS-only (the
    // capture stack is macOS-only); other targets assume granted.
    #[cfg(target_os = "macos")]
    {
        #[link(name = "CoreGraphics", kind = "framework")]
        extern "C" {
            fn CGPreflightScreenCaptureAccess() -> bool;
        }
        // Safety: no args, no memory — a pure capability query.
        if !unsafe { CGPreflightScreenCaptureAccess() } {
            tracing::info!(
                "capture-supervisor: Screen Recording not granted — deferred until next launch after grant"
            );
            return;
        }
    }

    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            tracing::error!(error = %e, "capture-supervisor: cannot resolve current_exe — capture not started");
            return;
        }
    };
    let db_path = crate::install::meridian_db_path();

    // Install a fresh cancel channel, dropping any prior one so the previous
    // supervisor tears down its child before this one spawns.
    let (cancel_tx, mut cancel_rx) = oneshot::channel::<()>();
    {
        let mut s = app_state.lock().unwrap();
        drop(s.capture_cancel.take());
        s.capture_cancel = Some(cancel_tx);
    }

    // tauri::async_runtime::spawn works from any thread (incl. the macOS main
    // thread the setup hook runs on); tokio::spawn would panic off a worker.
    tauri::async_runtime::spawn(async move {
        let mut restarts: Vec<Instant> = Vec::new();
        let mut backoff = MIN_BACKOFF;

        loop {
            let started = Instant::now();
            let mut child = match Command::new(&exe)
                .arg(HELPER_ARG)
                .arg("--db")
                .arg(&db_path)
                // If this supervisor task is dropped (tray shutdown), take the
                // child with it. The child's own ppid watcher is the backstop.
                .kill_on_drop(true)
                .spawn()
            {
                Ok(c) => c,
                Err(e) => {
                    tracing::error!(error = %e, "capture-supervisor: failed to spawn helper — capture not running");
                    return;
                }
            };
            tracing::info!(pid = ?child.id(), "capture-supervisor: helper started");

            tokio::select! {
                // Pause / shutdown: kill the child and stop supervising.
                _ = &mut cancel_rx => {
                    tracing::info!("capture-supervisor: stop requested — killing helper");
                    let _ = child.kill().await;
                    break;
                }
                // Helper exited on its own (clean end, error, or native abort).
                status = child.wait() => {
                    let ran = started.elapsed();
                    if ran >= HEALTHY_RUN {
                        // A real capture session ran; treat the exit as isolated
                        // and restart promptly.
                        restarts.clear();
                        backoff = MIN_BACKOFF;
                        tracing::warn!(?status, ran_s = ran.as_secs(), "capture-supervisor: helper exited after a healthy run — restarting");
                    } else {
                        let now = Instant::now();
                        restarts.retain(|t| now.duration_since(*t) < CRASH_WINDOW);
                        restarts.push(now);
                        tracing::warn!(
                            ?status,
                            ran_s = ran.as_secs(),
                            restarts_in_window = restarts.len(),
                            "capture-supervisor: helper died quickly — backing off before restart"
                        );
                        if restarts.len() > CRASH_LIMIT {
                            tracing::error!(
                                limit = CRASH_LIMIT,
                                window_s = CRASH_WINDOW.as_secs(),
                                "capture-supervisor: helper crash-looping — capture unavailable, slowing retries"
                            );
                            if let Some(p) = pool.as_ref() {
                                raise_capture_unavailable(p).await;
                            }
                            backoff = MAX_BACKOFF;
                        } else {
                            backoff = (backoff * 2).min(MAX_BACKOFF);
                        }
                    }

                    // Wait out the backoff, but stay responsive to a pause/stop.
                    tokio::select! {
                        _ = &mut cancel_rx => {
                            tracing::info!("capture-supervisor: stop requested during backoff");
                            break;
                        }
                        _ = tokio::time::sleep(backoff) => {}
                    }
                }
            }
        }
    });
}

/// Surface a `capture.unavailable` notice when the helper is crash-looping, so a
/// prolonged "no capture data" state is visible rather than silent. Cleared
/// implicitly the next time a healthy helper runs (the notice is advisory; we
/// don't gate capture on it). Best-effort — a notice write failure is logged,
/// never fatal.
async fn raise_capture_unavailable(pool: &SqlitePool) {
    if let Err(e) = meridian::notices::raise_typed(
        pool,
        meridian::notices::Notice {
            id: "capture.unavailable",
            severity: "warning",
            title: "Screen capture unavailable",
            detail: "Meridian could not run screen capture and is retrying. If this persists, re-grant Screen Recording to Meridian in System Settings > Privacy & Security.",
            remedy: None,
            event_key: "capture.unavailable",
            deep_link: None,
        },
    )
    .await
    {
        tracing::warn!(error = %e, "capture-supervisor: failed to raise capture.unavailable notice");
    }
}
