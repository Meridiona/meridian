//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Pause / resume commands for in-process capture.
//!
//! Split out of `daemon.rs` (CLAUDE.md's 500-line file cap) — this is the
//! self-contained pause/resume unit: the two pause entry points
//! ([`pause_for_duration`], [`pause_indefinitely`]), the shared
//! [`resume_capture`] both eventually reach, and the small clock/label
//! helpers only they use.
//!
//! # Who calls this
//! Registered in `lib.rs`'s `invoke_handler!`. Invoked by the popover's
//! duration-picker + "Resume now" buttons (`tray/src/app.js`). The
//! schedule-pause poll tick ([`crate::poll`]) inlines its own pause/resume
//! logic rather than calling into this module — it's driven by work-hours
//! config, not a duration.
//!
//! # Related
//! - [`crate::commands::daemon`] — daemon lifecycle/status; the sibling module
//!   this was split from.
//! - [`crate::state::PauseSource`] — the pause-kind enum these commands set.
//! - [`crate::poll::notifications_allowed`] — quiet-hours gate for the pause toast.

use crate::state::{AppState, PauseSource};
use crate::sys;
use chrono::{DateTime, SecondsFormat, Utc};
use meridian_core::SqlitePool;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::{Emitter, State};

/// Pause in-process capture for `seconds` (0 = resume now). Rejects the popover's
/// own presets from ever exceeding a day: the UI computes "pause until tomorrow"
/// morning, which can run past 24h if paused very late at night.
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
