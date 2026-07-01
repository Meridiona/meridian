//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Daemon lifecycle + status commands.
//!
//! Controls the `com.meridiona.daemon` launchd service (restart / pause / resume)
//! and reports its liveness — both the cached tray view ([`get_status`], from
//! [`crate::state::AppState`]) and a fresh socket probe ([`get_daemon_status`],
//! the ported `/api/daemon/status`).
//!
//! # Who calls this
//! Registered in `lib.rs`'s `invoke_handler!`. `get_daemon_status` is polled by
//! `SettingsView.tsx` during a reload via `ui/lib/bridge.ts::load`.
//!
//! # Related
//! - [`crate::sys`] — shared `uid_str` (launchctl domain) + `notify` (toast).
//! - [`crate::poll::notifications_allowed`] — quiet-hours gate for the pause toast.

use crate::state::{AppState, PauseSource, StatusPayload};
use crate::sys;
use chrono::{DateTime, SecondsFormat, Utc};
use meridian_core::SqlitePool;
use serde::Serialize;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::{Emitter, State};

/// The cached tray status (health + active session + today totals), read from
/// the poll-loop-maintained [`AppState`]. Synchronous — just locks and snapshots.
#[tauri::command]
pub fn get_status(state: State<'_, Arc<Mutex<AppState>>>) -> Result<StatusPayload, String> {
    state
        .lock()
        .map(|s| s.to_payload())
        .map_err(|e| e.to_string())
}

/// Force-restart the daemon via `launchctl kickstart -k`.
#[tauri::command]
pub async fn restart_daemon() -> Result<(), String> {
    let uid = sys::uid_str();
    let status = std::process::Command::new("launchctl")
        .args([
            "kickstart",
            "-k",
            &format!("gui/{}/com.meridiona.daemon", uid),
        ])
        .status()
        .map_err(|e| format!("launchctl failed: {}", e))?;
    if status.success() {
        Ok(())
    } else {
        Err("launchctl kickstart returned non-zero".to_string())
    }
}

/// Pause (`stop`) or resume (`start`) the daemon. On success, fires a toast
/// honoring the user's notification prefs (master switch + quiet hours), the
/// same policy the outbox notifications follow.
#[tauri::command]
pub async fn toggle_daemon(app: tauri::AppHandle, is_running: bool) -> Result<(), String> {
    let uid = sys::uid_str();
    let service = format!("gui/{}/com.meridiona.daemon", uid);

    let status = if is_running {
        std::process::Command::new("launchctl")
            .args(["stop", &service])
            .status()
    } else {
        std::process::Command::new("launchctl")
            .args(["start", &service])
            .status()
    }
    .map_err(|e| format!("launchctl failed: {}", e))?;

    if status.success() {
        let (title, body) = if is_running {
            ("Paused", "Meridian is paused. Click to resume.")
        } else {
            ("Resumed", "Meridian is back tracking.")
        };
        if crate::poll::notifications_allowed("system.pause").await {
            sys::notify(&app, title, body);
        }
        Ok(())
    } else {
        Err(format!(
            "launchctl {} returned non-zero",
            if is_running { "stop" } else { "start" }
        ))
    }
}

/// Pause in-process capture for `seconds` (0 = resume immediately).
///
/// On pause: sets `AppState.capture_paused = true`, stores the expiry timestamp,
/// and spawns a Tokio task that auto-resumes when the timer expires. On resume
/// (manual or auto), writes a `tracking_paused` gap row covering the paused
/// interval and fires a toast if notifications are allowed.
///
/// # Who calls this
/// The popover's duration-picker buttons (`pause-picker`) and the "Resume now"
/// button (`resume-btn`) via `tray/src/app.js`.
#[tauri::command]
#[tracing::instrument(skip(app, state, db_pool))]
pub async fn pause_for_duration(
    app: tauri::AppHandle,
    seconds: u64,
    state: State<'_, Arc<Mutex<AppState>>>,
    db_pool: State<'_, Option<SqlitePool>>,
) -> Result<(), String> {
    let pool = db_pool.inner().clone();

    if seconds == 0 {
        resume_capture(state.inner(), pool.as_ref(), &app, false).await;
        return Ok(());
    }

    // Defence-in-depth: the popover's presets top out at "pause until tomorrow"
    // (computed seconds-until-9am can run past 8h if paused late at night), but
    // the Rust command is also callable directly, so reject anything beyond 24h.
    if seconds > 86_400 {
        return Err(format!(
            "pause duration {} s exceeds 24-hour maximum (86400 s)",
            seconds
        ));
    }

    let now = now_secs();
    let until = now + seconds;

    // If a pause is already active (e.g. a schedule pause), close it out first
    // by writing a gap row for the T0→now period before overwriting state.
    let prev = {
        let s = state.lock().map_err(|e| e.to_string())?;
        s.pause_started_at.zip(s.pause_source.clone())
    };
    if let Some((prev_started, prev_src)) = prev {
        let kind = match prev_src {
            PauseSource::Timed | PauseSource::Indefinite => "tracking_paused",
            PauseSource::Schedule => "schedule_paused",
        };
        let duration_s = now.saturating_sub(prev_started) as i64;
        if duration_s > 0 {
            if let Some(p) = pool.as_ref() {
                if let Err(e) = meridian_core::insert_pause_gap(
                    p,
                    &secs_to_iso(prev_started),
                    &secs_to_iso(now),
                    duration_s,
                    kind,
                )
                .await
                {
                    tracing::warn!(error = %e, kind, "failed to write gap for interrupted pause");
                }
            }
        }
    }

    {
        let mut s = state.lock().map_err(|e| e.to_string())?;
        // Drop cancel senders → stops the engine and UI consumer tasks, fully
        // halting ScreenCaptureKit and the CGEventTap recorder.
        drop(s.engine_cancel.take());
        drop(s.ui_consumer_cancel.take());
        s.capture_paused.store(true, Ordering::Relaxed);
        s.pause_until = Some(until);
        s.pause_source = Some(PauseSource::Timed);
        s.pause_started_at = Some(now);
        s.schedule_resume_at = None;
    }

    // Emit immediately so the popover reflects the new state without waiting for the next poll tick.
    if let Ok(s) = state.lock() {
        let _ = app.emit("status-update", s.to_payload());
    }

    tracing::info!(seconds, until, "capture paused for duration");

    if crate::poll::notifications_allowed("system.pause").await {
        let label = pause_label(seconds);
        sys::notify(&app, "Tracking paused", &format!("Paused for {}.", label));
    }

    // Spawn the auto-resume task. Checks `pause_until` on wake to detect early
    // manual resumes (which clear the field) — no-ops if already resumed.
    let state_arc = state.inner().clone();
    let app_clone = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(tokio::time::Duration::from_secs(seconds)).await;
        let still_ours = state_arc
            .lock()
            .map(|s| s.pause_until == Some(until))
            .unwrap_or(false);
        if still_ours {
            resume_capture(&state_arc, pool.as_ref(), &app_clone, true).await;
        }
    });

    Ok(())
}

/// Pause in-process capture with no expiry ("Pause indefinitely") — only a
/// manual "Resume now" (`pause_for_duration(0)`) clears it. No auto-resume
/// timer is spawned, unlike [`pause_for_duration`].
///
/// # Who calls this
/// The popover's "Pause indefinitely" duration option (`tray/src/app.js`).
#[tauri::command]
#[tracing::instrument(skip(app, state, db_pool))]
pub async fn pause_indefinitely(
    app: tauri::AppHandle,
    state: State<'_, Arc<Mutex<AppState>>>,
    db_pool: State<'_, Option<SqlitePool>>,
) -> Result<(), String> {
    let pool = db_pool.inner().clone();
    let now = now_secs();

    // If a pause is already active (e.g. a schedule pause), close it out first
    // by writing a gap row for the T0→now period before overwriting state.
    let prev = {
        let s = state.lock().map_err(|e| e.to_string())?;
        s.pause_started_at.zip(s.pause_source.clone())
    };
    if let Some((prev_started, prev_src)) = prev {
        let kind = match prev_src {
            PauseSource::Timed | PauseSource::Indefinite => "tracking_paused",
            PauseSource::Schedule => "schedule_paused",
        };
        let duration_s = now.saturating_sub(prev_started) as i64;
        if duration_s > 0 {
            if let Some(p) = pool.as_ref() {
                if let Err(e) = meridian_core::insert_pause_gap(
                    p,
                    &secs_to_iso(prev_started),
                    &secs_to_iso(now),
                    duration_s,
                    kind,
                )
                .await
                {
                    tracing::warn!(error = %e, kind, "failed to write gap for interrupted pause");
                }
            }
        }
    }

    {
        let mut s = state.lock().map_err(|e| e.to_string())?;
        drop(s.engine_cancel.take());
        drop(s.ui_consumer_cancel.take());
        s.capture_paused.store(true, Ordering::Relaxed);
        s.pause_until = None;
        s.pause_source = Some(PauseSource::Indefinite);
        s.pause_started_at = Some(now);
        s.schedule_resume_at = None;
    }

    if let Ok(s) = state.lock() {
        let _ = app.emit("status-update", s.to_payload());
    }

    tracing::info!("capture paused indefinitely");

    if crate::poll::notifications_allowed("system.pause").await {
        sys::notify(&app, "Tracking paused", "Paused until you resume.");
    }

    Ok(())
}

/// Human-readable duration label for the pause toast notification.
/// Mirrors the JS `pauseLabel` in `tray/src/pause-utils.js`.
///
/// - sub-minute: `"N second(s)"`
/// - 1–59 min:   `"N minute(s)"`
/// - ≥ 60 min:   `"N hour(s)"` (whole hours, truncated)
pub(crate) fn pause_label(seconds: u64) -> String {
    let mins = seconds / 60;
    if mins == 0 {
        format!("{} second{}", seconds, if seconds == 1 { "" } else { "s" })
    } else if mins >= 60 {
        let h = mins / 60;
        format!("{} hour{}", h, if h == 1 { "" } else { "s" })
    } else {
        format!("{} minute{}", mins, if mins == 1 { "" } else { "s" })
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn secs_to_iso(secs: u64) -> String {
    DateTime::<Utc>::from_timestamp(secs as i64, 0)
        .unwrap_or_else(Utc::now)
        .to_rfc3339_opts(SecondsFormat::Millis, true)
}

/// Clear the capture pause, write a gap row, and optionally toast the user.
/// Shared by manual resume (`seconds = 0`) and auto-resume (timer expiry).
pub(crate) async fn resume_capture(
    state: &Arc<Mutex<AppState>>,
    pool: Option<&SqlitePool>,
    app: &tauri::AppHandle,
    auto: bool,
) {
    let (started, source) = {
        let mut s = state.lock().unwrap();
        let started = s.pause_started_at.take();
        let source = s.pause_source.take();
        s.capture_paused.store(false, Ordering::Relaxed);
        s.pause_until = None;
        s.schedule_resume_at = None;
        (started, source)
    };

    if let (Some(started_secs), Some(src)) = (started, source) {
        let kind = match src {
            PauseSource::Timed | PauseSource::Indefinite => "tracking_paused",
            PauseSource::Schedule => "schedule_paused",
        };
        let now = now_secs();
        let duration_s = now.saturating_sub(started_secs) as i64;
        if duration_s > 0 {
            if let Some(p) = pool {
                if let Err(e) = meridian_core::insert_pause_gap(
                    p,
                    &secs_to_iso(started_secs),
                    &secs_to_iso(now),
                    duration_s,
                    kind,
                )
                .await
                {
                    tracing::warn!(error = %e, kind, "failed to write pause gap");
                }
            }
        }
    }

    // Restart the capture engine so screen recording resumes.
    #[cfg(feature = "capture")]
    crate::start_capture(state.clone(), pool.cloned());

    // Emit immediately so the popover reverts to the picker without waiting for the next tick.
    if let Ok(s) = state.lock() {
        let _ = app.emit("status-update", s.to_payload());
    }

    tracing::info!(auto, "capture resumed");
    if !auto && crate::poll::notifications_allowed("system.pause").await {
        sys::notify(app, "Resumed", "Meridian is back tracking.");
    }
}

/// Response shape matching the TS route's `{ running, pid? }`.
#[derive(Debug, Clone, Serialize)]
pub struct DaemonStatusResponse {
    pub running: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
}

/// Probe `~/.meridian/daemon.sock` with an 800 ms timeout (the ported
/// `/api/daemon/status` GET). Returns `{running: false}` on any error — no error
/// surfaces to the caller (resolve-empty contract: stale UI stays visible rather
/// than erroring on every health poll tick).
#[tauri::command]
#[tracing::instrument]
pub async fn get_daemon_status() -> Result<DaemonStatusResponse, String> {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let sock_path = format!("{}/.meridian/daemon.sock", home);

    let result = probe_socket(&sock_path).await;
    tracing::info!(running = result.running, pid = ?result.pid, "daemon_status");
    Ok(result)
}

/// `{ ok, pid }` on a successful reload — mirrors the route's success body.
#[derive(Debug, Clone, Serialize)]
pub struct ReloadResponse {
    pub ok: bool,
    pub pid: u32,
}

/// Reload the daemon's config by sending it SIGHUP (the ported
/// `/api/daemon/reload` POST). The daemon exits cleanly on SIGHUP and launchd
/// restarts it, picking up `settings.json` changes (OTLP config, credentials).
/// Log-level changes hot-reload in-process and don't need this. Errors when the
/// daemon isn't running (the route's 503) — we resolve its pid from the same
/// `daemon.sock` greeting [`get_daemon_status`] reads.
#[tauri::command]
#[tracing::instrument]
pub async fn reload_daemon() -> Result<ReloadResponse, String> {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let sock_path = format!("{}/.meridian/daemon.sock", home);

    let probe = probe_socket(&sock_path).await;
    let Some(pid) = probe.pid.filter(|_| probe.running) else {
        return Err("daemon not running".to_string());
    };

    // `kill -HUP <pid>` (the route's `process.kill(pid, 'SIGHUP')`) — no libc dep.
    let status = std::process::Command::new("kill")
        .args(["-HUP", &pid.to_string()])
        .status()
        .map_err(|e| format!("kill failed: {e}"))?;
    if !status.success() {
        return Err(format!("kill -HUP {pid} returned non-zero"));
    }
    tracing::info!(pid, "daemon reload (SIGHUP) sent");
    Ok(ReloadResponse { ok: true, pid })
}

async fn probe_socket(sock_path: &str) -> DaemonStatusResponse {
    use tokio::io::AsyncReadExt;
    use tokio::net::UnixStream;
    use tokio::time::timeout;

    let connect = timeout(Duration::from_millis(800), UnixStream::connect(sock_path)).await;

    let mut stream = match connect {
        Ok(Ok(s)) => s,
        _ => {
            return DaemonStatusResponse {
                running: false,
                pid: None,
            }
        }
    };

    // Read until EOF or timeout, then parse the greeting JSON.
    let mut buf = Vec::new();
    let _ = timeout(Duration::from_millis(800), stream.read_to_end(&mut buf)).await;

    if buf.is_empty() {
        return DaemonStatusResponse {
            running: false,
            pid: None,
        };
    }

    match serde_json::from_slice::<serde_json::Value>(&buf) {
        Ok(v) => {
            let pid = v.get("pid").and_then(|p| p.as_u64()).map(|p| p as u32);
            DaemonStatusResponse { running: true, pid }
        }
        Err(_) => DaemonStatusResponse {
            running: false,
            pid: None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::{pause_label, secs_to_iso};

    // ── US-5: Toast notification label ───────────────────────────────────────
    // pause_for_duration builds a toast label from the requested seconds.
    // These tests mirror the JS pauseLabel tests in tray/src/__tests__/pause.test.js.

    #[test]
    fn label_sub_minute_singular() {
        assert_eq!(pause_label(1), "1 second");
    }

    #[test]
    fn label_sub_minute_plural() {
        assert_eq!(pause_label(30), "30 seconds");
        assert_eq!(pause_label(59), "59 seconds");
    }

    #[test]
    fn label_exactly_one_minute() {
        assert_eq!(pause_label(60), "1 minute");
    }

    #[test]
    fn label_plural_minutes() {
        assert_eq!(pause_label(120), "2 minutes");
        assert_eq!(pause_label(900), "15 minutes");
        assert_eq!(pause_label(1800), "30 minutes");
        assert_eq!(pause_label(3540), "59 minutes");
    }

    #[test]
    fn label_exactly_one_hour() {
        assert_eq!(pause_label(3600), "1 hour");
    }

    #[test]
    fn label_plural_hours() {
        assert_eq!(pause_label(7200), "2 hours");
        assert_eq!(pause_label(28800), "8 hours"); // max custom duration
    }

    #[test]
    fn label_fractional_hours_truncate_to_whole() {
        // 1h 30m → "1 hour" (mins / 60 truncates)
        assert_eq!(pause_label(5400), "1 hour");
        // 2h 59m → "2 hours"
        assert_eq!(pause_label(10740), "2 hours");
    }

    // ── US-6: Resume-now path (seconds = 0) ──────────────────────────────────
    // pause_for_duration(0) takes the early-return resume path before reaching
    // pause_label, so this test documents the function's contract at 0 rather
    // than testing reachable production code.
    #[test]
    fn label_zero_seconds() {
        assert_eq!(pause_label(0), "0 seconds");
    }

    // ── secs_to_iso sanity ───────────────────────────────────────────────────
    #[test]
    fn secs_to_iso_epoch() {
        assert_eq!(secs_to_iso(0), "1970-01-01T00:00:00.000Z");
    }

    #[test]
    fn secs_to_iso_known_timestamp() {
        // 2024-01-01T00:00:00Z = 1704067200 s
        assert_eq!(secs_to_iso(1_704_067_200), "2024-01-01T00:00:00.000Z");
    }
}
