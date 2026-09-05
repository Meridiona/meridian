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
//! - [`meridian::notices`] — the fault-bus the pause/resume notice routes through
//!   (id `tray.paused`, event_key `system.pause`); quiet-hours/master-switch
//!   policy is all enforced there, not by these commands.

use crate::state::{AppState, PauseSource};
use chrono::{DateTime, SecondsFormat, Utc};
use meridian_core::SqlitePool;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::{Emitter, State};

/// Close out any active pause (writing its gap row) and commit a new pause
/// state. The prior-state read and the new-state write happen under a single
/// lock acquisition (no `await` in between), so a concurrent pause/resume call
/// — a double-click before the UI disables the button, or a race with the
/// schedule-pause poll tick — can't read the same stale `prev` snapshot and
/// interleave writes, which previously could corrupt `pause_started_at`/gap
/// accounting that feeds the "Logged" time metrics. The gap-row DB write
/// itself happens after the lock is released, using the values captured
/// atomically with the state swap rather than a fresh read.
async fn transition_pause(
    state: &Arc<Mutex<AppState>>,
    pool: Option<&SqlitePool>,
    now: u64,
    new_source: PauseSource,
    new_until: Option<u64>,
) -> Result<(), String> {
    let prev = {
        let mut s = state.lock().map_err(|e| e.to_string())?;
        let prev = s.pause_started_at.take().zip(s.pause_source.take());
        drop(s.engine_cancel.take());
        drop(s.ui_consumer_cancel.take());
        s.capture_paused.store(true, Ordering::Relaxed);
        s.pause_until = new_until;
        s.pause_source = Some(new_source);
        s.pause_started_at = Some(now);
        s.schedule_resume_at = None;
        prev
    };
    if let Some((prev_started, prev_src)) = prev {
        let kind = match prev_src {
            PauseSource::Timed | PauseSource::Indefinite => "tracking_paused",
            PauseSource::Schedule => "schedule_paused",
            PauseSource::DiskLow => "disk_low_paused",
        };
        let duration_s = now.saturating_sub(prev_started) as i64;
        if duration_s > 0 {
            if let Some(p) = pool {
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
    Ok(())
}

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
    db_pool: State<'_, crate::db_pool::DbPool>,
) -> Result<(), String> {
    let pool = db_pool.get();

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

    // Drops engine_cancel/ui_consumer_cancel (halting ScreenCaptureKit + the
    // CGEventTap recorder) and closes out any already-active pause, all in
    // one atomic state transition — see transition_pause's doc comment.
    transition_pause(
        state.inner(),
        pool.as_ref(),
        now,
        PauseSource::Timed,
        Some(until),
    )
    .await?;

    // Emit immediately so the popover reflects the new state without waiting for the next poll tick.
    if let Ok(s) = state.lock() {
        let _ = app.emit("status-update", s.to_payload());
    }

    tracing::info!(seconds, until, "capture paused for duration");

    if let Some(p) = pool.as_ref() {
        let detail = format!("Paused for {}.", pause_label(seconds));
        if let Err(e) = meridian::notices::raise_typed(
            p,
            meridian::notices::Notice {
                id: "tray.paused",
                severity: "info",
                title: "Tracking paused",
                detail: &detail,
                remedy: None,
                event_key: "system.pause",
                deep_link: None,
            },
        )
        .await
        {
            tracing::warn!(error = %e, "pause notice raise failed");
        }
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
    db_pool: State<'_, crate::db_pool::DbPool>,
) -> Result<(), String> {
    let pool = db_pool.get();
    let now = now_secs();

    // Same atomic close-out-then-commit as pause_for_duration — see
    // transition_pause's doc comment.
    transition_pause(
        state.inner(),
        pool.as_ref(),
        now,
        PauseSource::Indefinite,
        None,
    )
    .await?;

    if let Ok(s) = state.lock() {
        let _ = app.emit("status-update", s.to_payload());
    }

    tracing::info!("capture paused indefinitely");

    if let Some(p) = pool.as_ref() {
        if let Err(e) = meridian::notices::raise_typed(
            p,
            meridian::notices::Notice {
                id: "tray.paused",
                severity: "info",
                title: "Tracking paused",
                detail: "Paused until you resume.",
                remedy: None,
                event_key: "system.pause",
                deep_link: None,
            },
        )
        .await
        {
            tracing::warn!(error = %e, "pause notice raise failed");
        }
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
///
/// Gated on free disk space first: a nearly-full disk is how a real
/// `meridian.db` got corrupted (SQLite/SQLCipher torn page allocations under
/// WAL when a write landed with no room left to grow). If disk is still low
/// at resume time — for any pause reason, including a manual "Resume now"
/// click — this refuses to restart the capture engine and instead converts
/// the pause to [`PauseSource::DiskLow`], so it can only clear once
/// [`poll::check_disk_space`] sees space recover. The existing
/// `system.disk_low` daemon notice (raised every poll tick) already tells the
/// user why, so no separate notification is raised here.
pub(crate) async fn resume_capture(
    state: &Arc<Mutex<AppState>>,
    pool: Option<&SqlitePool>,
    app: &tauri::AppHandle,
    auto: bool,
) {
    if meridian::health::platform::meridian_data_low_gb().is_some() {
        tracing::warn!("resume_capture: refusing to resume — disk space still low");
        let now = now_secs();
        if let Err(e) = transition_pause(state, pool, now, PauseSource::DiskLow, None).await {
            tracing::warn!(error = %e, "resume_capture: failed to convert pause to disk-low");
        }
        if let Ok(s) = state.lock() {
            let _ = app.emit("status-update", s.to_payload());
        }
        return;
    }

    let (started, source) = {
        let mut s = match state.lock() {
            Ok(s) => s,
            Err(e) => {
                tracing::error!(error = %e, "pause mutex poisoned; capture may remain paused");
                return;
            }
        };
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
            PauseSource::DiskLow => "disk_low_paused",
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
    //
    // Takes the SWAPPABLE handle from managed state rather than cloning the raw
    // `pool` this function was given. A raw clone keeps writing to the pool
    // object `DbPool::close` shut, so capture would be dead from the next daemon
    // restart onward - see `crate::start_capture`'s doc. `try_state` because a
    // tray whose database never opened has no handle to manage, and capture
    // degrades to dropping frames exactly as it already does on a cold start.
    #[cfg(feature = "capture")]
    crate::restart_capture(app, state, "pause resumed");

    // Emit immediately so the popover reverts to the picker without waiting for the next tick.
    if let Ok(s) = state.lock() {
        let _ = app.emit("status-update", s.to_payload());
    }

    tracing::info!(auto, "capture resumed");
    if let Some(p) = pool {
        // The pause condition is over either way — clear the "you're paused"
        // notice/banner regardless of who resumed it.
        if let Err(e) = meridian::notices::clear_typed(p, "tray.paused", "system.pause").await {
            tracing::warn!(error = %e, "pause notice clear failed");
        }
        // The "Resumed" confirmation is a discrete one-shot event, not a
        // state to clear later — only for a manual resume (matches the prior
        // behavior of staying silent on auto-resume, e.g. after a schedule
        // window or an expired timed pause, to avoid over-notifying).
        if !auto {
            let dedup = format!("system.pause:resumed:{}", now_secs());
            let n = meridian::notifications::NewNotification::event(
                &dedup,
                "system.pause",
                "Resumed",
                "Meridian is back tracking.",
            )
            .via(meridian::notifications::CHANNEL_NATIVE);
            if let Err(e) = meridian::notifications::enqueue(p, n).await {
                tracing::warn!(error = %e, "resume notification enqueue failed");
            }
        }
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
