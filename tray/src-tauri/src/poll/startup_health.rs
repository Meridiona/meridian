//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! A one-shot fast health repaint for the first seconds after tray launch.
//!
//! # The problem this closes
//! The poll loop's [`super::refresh::refresh_health`] only runs on ticks
//! 0, 2, 4, … — every 60 s — because that cadence also drives the went-quiet
//! / back-online *notice* and the auto-restart decision
//! (`decide_health_notice`), both deliberately debounced so a genuine outage
//! doesn't flap. Tick 0 fires within moments of launch, before the daemon has
//! necessarily finished starting or the DB pool has opened, so a cold start
//! correctly reports Unhealthy at that instant — and then the popover's
//! "Meridian is offline" banner just sits there, stale, for up to 60 s even
//! once the daemon and DB are both actually ready, because nothing repaints
//! the DISPLAYED status in between. Observed live: the health panel's
//! on-demand rows ("Daemon: Running", "Database: Ready") already show the
//! true state the moment it opens, while the banner above it still says
//! offline — two reads of the same underlying signal, one fresh, one stale.
//!
//! # What this does and does not touch
//! This loop only repaints [`AppState::health`] / `ui_reachable` and re-emits
//! `status-update` — it never touches `consecutive_health_failures`,
//! `daemon_was_healthy` or `startup_health_reconciled`, so it cannot affect
//! the went-quiet notice or trigger an auto-restart; those stay solely owned
//! by `refresh_health`. It runs at most once per process, stops the instant
//! it observes a healthy check, and gives up silently after [`CEILING`],
//! letting the normal cadence take over — it must never spin forever on a
//! machine that is genuinely down.
//!
//! It also does not change what "healthy" MEANS — it shares
//! [`crate::commands::health::is_healthy`] with `refresh_health`, so the two
//! can never disagree, and this fix is scoped to the display lag only, not a
//! new readiness signal.
//!
//! # Related
//! - [`super::refresh::refresh_health`] — the slower, notice-owning check
//!   this repaints ahead of.
//! - [`super::watchdog`] — the same "separate fast loop, narrow job" shape,
//!   for starting a stopped daemon rather than painting the UI.

use crate::commands::health::{check_health, is_healthy};
use crate::state::{AppState, HealthStatus};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::Emitter;

/// How often to recheck while not yet healthy.
const TICK: Duration = Duration::from_secs(3);

/// Give up after this long. Matches the worst-case wait this loop replaces
/// (the normal cadence's 60 s), so a genuinely broken install is never worse
/// off than before this loop existed — just never better, past this point.
const CEILING: Duration = Duration::from_secs(60);

/// Poll [`check_health`] every [`TICK`] until the tray is healthy (or
/// [`CEILING`] elapses), repainting the displayed status the moment it is.
/// Intended to be spawned once, at tray startup, alongside the main poll
/// loop — see `lib.rs`.
pub async fn fast_poll_until_healthy(app: tauri::AppHandle, state: Arc<Mutex<AppState>>) {
    let deadline = tokio::time::Instant::now() + CEILING;
    loop {
        let hr = check_health().await;
        if is_healthy(&hr) {
            let payload = {
                let Ok(mut s) = state.lock() else {
                    return;
                };
                // A concurrent `refresh_health` tick may already have painted
                // this by the time we get here — overwriting it with the same
                // (now also fresh) verdict is harmless.
                s.health = HealthStatus::Healthy;
                s.ui_reachable = true;
                s.to_payload()
            };
            let _ = app.emit("status-update", payload);
            return;
        }
        if tokio::time::Instant::now() >= deadline {
            return;
        }
        tokio::time::sleep(TICK).await;
    }
}
