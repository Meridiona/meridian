//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Daemon lifecycle + status commands.
//!
//! Controls the daemon service (restart / pause / resume) and reports its
//! liveness — both the cached tray view ([`get_status`], from
//! [`crate::state::AppState`]) and a fresh endpoint probe ([`get_daemon_status`],
//! the ported `/api/daemon/status`).
//!
//! The OS-specific mechanics — launchd + a Unix socket on macOS, the Task
//! Scheduler + a named pipe on Windows — live in [`super::daemon_control`]; this
//! module is the platform-agnostic command surface over them.
//!
//! # Who calls this
//! Registered in `lib.rs`'s `invoke_handler!`. `get_daemon_status` is polled by
//! `SettingsView.tsx` during a reload via `ui/lib/bridge.ts::load`.
//!
//! # Related
//! - [`crate::sys`] — shared `uid_str` (launchctl domain).
//! - [`meridian::notices`] — the fault-bus the toggle notice routes through (id
//!   `tray.daemon_paused`, event_key `system.pause`, same toggle as
//!   [`crate::commands::pause`]'s capture-pause notice).
//! - [`crate::commands::pause`] — `pause_for_duration`/`pause_indefinitely`, split
//!   out of this module (CLAUDE.md's 500-line file cap).

use crate::state::{AppState, StatusPayload};
use meridian_core::SqlitePool;
use serde::Serialize;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use tauri::State;
use tracing::Instrument;

/// The cached tray status (health + active session + today totals), read from
/// the poll-loop-maintained [`AppState`]. Synchronous — just locks and snapshots.
#[tauri::command]
pub fn get_status(state: State<'_, Arc<Mutex<AppState>>>) -> Result<StatusPayload, String> {
    state
        .lock()
        .map(|s| s.to_payload())
        .map_err(|e| e.to_string())
}

/// Force-restart the daemon (macOS `launchctl kickstart -k`; Windows restarts
/// the scheduled task).
#[tauri::command]
pub async fn restart_daemon() -> Result<(), String> {
    super::daemon_control::restart().await
}

/// Pause (stop) or resume (start) the daemon. On success, raises/clears a
/// `tray.daemon_paused` notice — same `system.pause` event_key as
/// [`crate::commands::pause`]'s capture-pause notice, so quiet-hours/master-
/// switch policy applies identically.
///
/// Start/stop goes through [`crate::daemon_lifecycle`], **not**
/// [`super::daemon_control::set_running`]. That is the fix for a toggle that
/// never actually paused anything: `set_running(false)` runs `launchctl stop`,
/// and the daemon's plist sets `KeepAlive=true`, so launchd restarted it after
/// `ThrottleInterval` (30 s) while the menu went on reading "Disconnected ○".
/// The same trap is documented from the other side in
/// [`meridian::db::repair::marker`].
///
/// The paused flag and the OS call are sequenced inside
/// [`crate::daemon_lifecycle`], under its lifecycle guard, rather than here.
/// That matters: a paused daemon is `bootout`ed and so is indistinguishable
/// from a crashed one, and both [`crate::poll::watchdog`] (every 5 s) and the
/// installer's launch-time restore will start a daemon they believe is down.
/// Sequencing them from the command would leave both races open.
#[tauri::command]
#[tracing::instrument(skip(_app, db_pool), fields(action))]
pub async fn toggle_daemon(
    _app: tauri::AppHandle,
    is_running: bool,
    db_pool: State<'_, Option<SqlitePool>>,
) -> Result<(), String> {
    let pool = db_pool.inner().clone();
    // `is_running` is the CURRENT state, so the useful field is what the user
    // asked for, not what it already was.
    tracing::Span::current().record("action", if is_running { "pause" } else { "resume" });

    // The OS work and its own spans live in `daemon_lifecycle`; this command
    // owns the request boundary and the notice write.
    if is_running {
        crate::daemon_lifecycle::stop_for_pause().await?;
    } else {
        crate::daemon_lifecycle::resume_from_pause().await?;
    }

    if let Some(p) = pool.as_ref() {
        let result = if is_running {
            meridian::notices::raise_typed(
                p,
                meridian::notices::Notice {
                    id: "tray.daemon_paused",
                    severity: "info",
                    title: "Paused",
                    detail: "Meridian is paused. Click to resume.",
                    remedy: None,
                    event_key: "system.pause",
                    deep_link: None,
                },
            )
            .instrument(tracing::debug_span!("daemon.toggle.write.notices"))
            .await
        } else {
            meridian::notices::clear_typed(p, "tray.daemon_paused", "system.pause")
                .instrument(tracing::debug_span!("daemon.toggle.write.notices"))
                .await
        };
        if let Err(e) = result {
            tracing::warn!(error = %e, is_running, "daemon toggle notice write failed");
        }
    }
    tracing::info!(is_running, "daemon toggled");
    Ok(())
}

/// Response shape matching the TS route's `{ running, pid? }`.
#[derive(Debug, Clone, Serialize)]
pub struct DaemonStatusResponse {
    pub running: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
}

/// Probe the daemon with the process-alive second opinion (the ported
/// `/api/daemon/status` GET). Returns `{running: false}` on any error — no error
/// surfaces to the caller (resolve-empty contract: stale UI stays visible rather
/// than erroring on every health poll tick).
///
/// [`daemon_control::status`], NOT the bare probe: this drives the dashboard's
/// current-hour PAUSED badge (polled every 30 s), which flapped against a
/// healthy daemon whenever the tray's own load starved the 800 ms probe — see
/// `status`'s doc.
#[tauri::command]
#[tracing::instrument]
pub async fn get_daemon_status() -> Result<DaemonStatusResponse, String> {
    let probe = super::daemon_control::status().await;
    tracing::info!(running = probe.running, pid = ?probe.pid, "daemon_status");
    Ok(DaemonStatusResponse {
        running: probe.running,
        pid: probe.pid,
    })
}

/// `{ ok, pid }` on a successful reload — mirrors the route's success body.
#[derive(Debug, Clone, Serialize)]
pub struct ReloadResponse {
    pub ok: bool,
    pub pid: u32,
}

/// Reload the daemon's config (the ported `/api/daemon/reload` POST).
///
/// macOS sends SIGHUP: the daemon exits cleanly and launchd relaunches it,
/// picking up startup-only `settings.json` values (OTLP config, credentials).
/// Windows restarts the scheduled task to the same end — most settings are
/// re-read every poll tick regardless, so only startup-only ones need this.
/// Log-level changes hot-reload in-process and need neither.
///
/// Errors when the daemon isn't running (the route's 503).
///
/// Rate-limited by [`plan_reload`] — see it for why a second SIGHUP inside 30 s
/// takes the daemon DOWN rather than reloading it.
#[tauri::command]
#[tracing::instrument]
pub async fn reload_daemon() -> Result<ReloadResponse, String> {
    let action = {
        let mut st = reload_state().lock().expect("reload state mutex poisoned");
        let action = plan_reload(st.last_sent, st.pending, Instant::now(), THROTTLE_INTERVAL);
        if let ReloadAction::Defer(_) = action {
            st.pending = true;
        }
        action
    };

    match action {
        ReloadAction::SendNow => {
            let pid = send_reload().await?;
            Ok(ReloadResponse { ok: true, pid })
        }
        // Inside the throttle window. Fire ONE trailing reload when the window
        // closes instead of blocking this caller (both call sites are
        // best-effort and one of them answers a UI request). Correctness rests
        // on the daemon re-reading `.env`/`settings.json` at STARTUP: a reload
        // that happens later still picks up every write made before it fires,
        // so coalescing cannot lose a credential update.
        ReloadAction::Defer(wait) => {
            tokio::spawn(async move {
                tokio::time::sleep(wait).await;
                let sent = send_reload().await;
                let mut st = reload_state().lock().expect("reload state mutex poisoned");
                st.pending = false;
                match sent {
                    Ok(pid) => tracing::info!(pid, "daemon reload sent (deferred past throttle)"),
                    Err(e) => tracing::warn!(error = %e, "deferred daemon reload failed"),
                }
            });
            tracing::info!(
                wait_s = wait.as_secs(),
                "daemon reload deferred - inside launchd's throttle window"
            );
            // `pid: 0` = nothing signalled yet. The reload IS going to happen,
            // so this stays `ok` - the callers only use it to warn when the
            // daemon is down entirely.
            Ok(ReloadResponse { ok: true, pid: 0 })
        }
        ReloadAction::AlreadyPending => {
            tracing::info!("daemon reload coalesced into the pending one");
            Ok(ReloadResponse { ok: true, pid: 0 })
        }
    }
}

/// Send the SIGHUP and stamp the moment, so [`plan_reload`] can space the next.
async fn send_reload() -> Result<u32, String> {
    let pid = super::daemon_control::reload().await?;
    reload_state()
        .lock()
        .expect("reload state mutex poisoned")
        .last_sent = Some(Instant::now());
    tracing::info!(pid, "daemon reload requested");
    Ok(pid)
}

/// launchd's `ThrottleInterval` for `com.meridiona.daemon` (see
/// `scripts/com.meridiona.daemon.plist`). Must stay in step with the plist.
const THROTTLE_INTERVAL: Duration = Duration::from_secs(30);

/// What to do with a reload request. See [`plan_reload`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReloadAction {
    /// Signal the daemon now.
    SendNow,
    /// Inside the throttle window - signal after this long instead.
    Defer(Duration),
    /// A deferred reload is already scheduled; it will cover this request too.
    AlreadyPending,
}

#[derive(Default)]
struct ReloadState {
    last_sent: Option<Instant>,
    pending: bool,
}

fn reload_state() -> &'static Mutex<ReloadState> {
    static STATE: OnceLock<Mutex<ReloadState>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(ReloadState::default()))
}

/// Decide when a reload may actually be signalled.
///
/// # Why this exists
/// On macOS a "reload" is `kill -HUP`, and the daemon handles SIGHUP by
/// **exiting cleanly** so launchd relaunches it ([`crate::commands::daemon_control::reload`],
/// `src/platform/unix.rs`). launchd will not relaunch a job more often than the
/// plist's `ThrottleInterval` (30 s), measured from the job's last *start* - so
/// a second SIGHUP inside that window does not reload the daemon, it **kills it
/// for the remainder of the window**.
///
/// That is not hypothetical. Measured on 1.84.0-staging.7:
///
/// ```text
/// 03:05:42.556  daemon starting          (first reload - launchd relaunches immediately)
/// 03:05:48.331  SIGHUP received          (second reload, 6 s later - daemon exits)
/// 03:05:55      watchdog: "not running"  (launchd is throttled, nothing to probe)
/// 03:06:12.847  daemon starting          = 03:05:42.556 + 30 s, exactly
/// ```
///
/// The daemon was down for 24 s, and the tray's long-lived SQLCipher pool spent
/// that window returning `(code: 11) database disk image is malformed` from
/// `get_tasks` - surfaced in the integrations panel as a corrupt database, on a
/// file whose `PRAGMA integrity_check` was (and stayed) `ok`.
///
/// Two independent call sites reload after a credential change
/// (`integrations.rs`'s token save and the GitHub device-flow completion), so a
/// single connect flow can trip this on its own.
///
/// # The rule
/// Never signal within [`THROTTLE_INTERVAL`] of the previous signal; defer to
/// the moment the window closes, and collapse every request arriving in the
/// meantime into that one deferred reload.
fn plan_reload(
    last_sent: Option<Instant>,
    pending: bool,
    now: Instant,
    throttle: Duration,
) -> ReloadAction {
    if pending {
        return ReloadAction::AlreadyPending;
    }
    match last_sent {
        // `checked_duration_since` rather than `-`: a monotonic clock cannot go
        // backwards, but saturating to zero keeps a future stamp from panicking.
        Some(prev) => match throttle.checked_sub(now.saturating_duration_since(prev)) {
            Some(remaining) if !remaining.is_zero() => ReloadAction::Defer(remaining),
            _ => ReloadAction::SendNow,
        },
        None => ReloadAction::SendNow,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The exact field sequence from 1.84.0-staging.7 (see [`plan_reload`]):
    /// two reloads 6 s apart. The second MUST NOT be signalled - doing so exits
    /// the daemon into launchd's throttle window and takes it down for the
    /// remainder of it, which is what produced the bogus "database disk image
    /// is malformed" in the integrations panel.
    #[test]
    fn a_second_reload_six_seconds_later_is_deferred_not_signalled() {
        let start = Instant::now();
        let first = plan_reload(None, false, start, THROTTLE_INTERVAL);
        assert_eq!(
            first,
            ReloadAction::SendNow,
            "the first reload always sends"
        );

        let six_s_later = start + Duration::from_secs(6);
        let second = plan_reload(Some(start), false, six_s_later, THROTTLE_INTERVAL);
        assert_eq!(
            second,
            ReloadAction::Defer(Duration::from_secs(24)),
            "must wait out the rest of the 30 s throttle, not SIGHUP into it"
        );
    }

    /// The invariant the whole module exists for, swept across the window
    /// rather than spot-checked: inside the throttle, `SendNow` is never a
    /// legal answer, and the deferral always lands exactly at the boundary.
    #[test]
    fn nothing_inside_the_throttle_window_is_ever_signalled_immediately() {
        let start = Instant::now();
        for elapsed in 0..THROTTLE_INTERVAL.as_secs() {
            let now = start + Duration::from_secs(elapsed);
            match plan_reload(Some(start), false, now, THROTTLE_INTERVAL) {
                ReloadAction::Defer(wait) => assert_eq!(
                    Duration::from_secs(elapsed) + wait,
                    THROTTLE_INTERVAL,
                    "a deferral at +{elapsed}s must fire exactly when the window closes"
                ),
                other => panic!("reload at +{elapsed}s must be deferred, got {other:?}"),
            }
        }
    }

    /// Once the window has closed launchd will relaunch immediately again, so
    /// the reload must actually go out - a guard that never re-armed would
    /// leave credential changes needing a manual restart.
    #[test]
    fn a_reload_after_the_window_closes_is_signalled() {
        let start = Instant::now();
        for elapsed in [30u64, 31, 600] {
            assert_eq!(
                plan_reload(
                    Some(start),
                    false,
                    start + Duration::from_secs(elapsed),
                    THROTTLE_INTERVAL
                ),
                ReloadAction::SendNow,
                "a reload {elapsed}s later is outside the throttle and must send"
            );
        }
    }

    /// A burst (token save + OAuth completion + a retry) must collapse into the
    /// ONE already-scheduled reload. Without this each request would queue its
    /// own trailing SIGHUP and rebuild the same storm a few seconds later.
    #[test]
    fn requests_arriving_while_one_is_pending_are_coalesced() {
        let start = Instant::now();
        for elapsed in [0u64, 3, 6, 29, 45] {
            assert_eq!(
                plan_reload(
                    Some(start),
                    true,
                    start + Duration::from_secs(elapsed),
                    THROTTLE_INTERVAL
                ),
                ReloadAction::AlreadyPending,
                "a pending reload covers the request arriving at +{elapsed}s"
            );
        }
    }

    /// The throttle constant is only correct while it matches the plist launchd
    /// actually reads. If someone retunes the plist, this fails instead of
    /// silently reintroducing the outage window.
    #[test]
    fn the_throttle_constant_matches_the_daemon_plist() {
        let plist = include_str!("../../../../scripts/com.meridiona.daemon.plist");
        let after = plist
            .split("<key>ThrottleInterval</key>")
            .nth(1)
            .expect("the plist declares ThrottleInterval");
        let value: u64 = after
            .split("<integer>")
            .nth(1)
            .and_then(|s| s.split("</integer>").next())
            .expect("ThrottleInterval has an integer value")
            .trim()
            .parse()
            .expect("ThrottleInterval parses");
        assert_eq!(
            value,
            THROTTLE_INTERVAL.as_secs(),
            "THROTTLE_INTERVAL drifted from the plist launchd enforces"
        );
    }
}
