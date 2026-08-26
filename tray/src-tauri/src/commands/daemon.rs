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
use serde::Serialize;
use std::sync::{Arc, Mutex};
use tauri::State;
use tracing::Instrument;

// Everything below is used ONLY by the macOS reload rate limit, so it carries
// the same gate as the code that uses it. Without that, the Windows build sees
// three unused imports and fails on `-D warnings` — a break that is invisible
// from macOS and only shows up in the Windows CI job.
#[cfg(target_os = "macos")]
use std::sync::OnceLock;
#[cfg(target_os = "macos")]
use std::time::Duration;
// tokio's `Instant`, not std's: behaviourally identical in production, but it
// is the one `#[tokio::test(start_paused = true)]` can advance. With std's, a
// test that "sleeps" past the 30 s launchd window advances only tokio's timer
// while the stamps being compared stay microseconds old, so the guard under
// test never sees the window close and the test asserts the opposite of the
// truth.
#[cfg(target_os = "macos")]
use tokio::time::Instant;

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
    db_pool: State<'_, crate::db_pool::DbPool>,
) -> Result<(), String> {
    let pool = db_pool.get();
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
/// Errors when the daemon isn't running (the route's 503) — callers surface
/// that as "your change applies when the daemon starts", so it must stay
/// reachable. See [`plan_reload`] for the macOS rate limit.
///
/// Also closes and reopens the tray's own `meridian.db` pool around the
/// signal (see [`reload_with_pool_cycle`]) — a daemon restart is exactly the
/// process-generation boundary [`crate::db_pool::DbPool`] exists to keep the
/// tray's connection from spanning. See that module's header for the
/// corruption incident this closes off, and [`plan_reload`]'s doc for an
/// EARLIER, related incident this same reload path caused (repeated SIGHUPs
/// landing inside launchd's throttle window) — different mechanism, same
/// underlying shape: a daemon-down window the tray's pool did not know about.
#[tauri::command]
#[tracing::instrument(skip(db_pool))]
pub async fn reload_daemon(
    db_pool: State<'_, crate::db_pool::DbPool>,
) -> Result<ReloadResponse, String> {
    // Owned clone (cheap - `DbPool` is `Arc`-backed): `reload_daemon_with`
    // needs `'static`, which a borrowed `State<'_, _>` cannot provide.
    reload_daemon_with(db_pool.inner().clone()).await
}

/// [`reload_daemon`]'s body, taking an owned pool handle instead of a
/// `State<'_, _>` so the two OTHER places that reload the daemon after a
/// credential change (`integrations::save_integration_token`,
/// `integrations::start_oauth_github_device`'s spawned polling task — see
/// [`plan_reload`]'s doc: "a single connect flow can trip this on its own")
/// can share this exact close/reopen + throttle logic instead of calling the
/// bare `#[tauri::command]` fn, which a `tauri::async_runtime::spawn`'d task
/// cannot do (it needs `'static`, `State<'_, _>` is borrowed for the
/// invocation). `db_pool.inner().clone()` is how a command handler bridges
/// the two - see the call sites in `integrations.rs`.
pub(crate) async fn reload_daemon_with(
    pool: crate::db_pool::DbPool,
) -> Result<ReloadResponse, String> {
    #[cfg(target_os = "macos")]
    {
        reload_throttled(
            reload_state(),
            move || {
                let pool = pool.clone();
                async move { reload_with_pool_cycle(&pool).await }
            },
            || async { super::daemon_control::status().await.running },
        )
        .await
    }
    // Windows restarts the scheduled task (`schtasks /End` + `/Run`). There is
    // no launchd, no SIGHUP and no `ThrottleInterval` there, so rate-limiting
    // would delay credential pickup by 30 s to respect a constant with no
    // meaning on the platform.
    #[cfg(not(target_os = "macos"))]
    {
        let pid = reload_with_pool_cycle(&pool).await?;
        tracing::info!(pid, "daemon reload requested");
        Ok(ReloadResponse { ok: true, pid })
    }
}

/// Closes the tray's `meridian.db` pool before signaling the daemon and
/// reopens it right after — see [`reload_daemon`]'s doc for why. A `None`
/// pool inside (the DB was never open, e.g. a fresh install before the
/// daemon has created it) makes both calls no-ops, matching every other call
/// site's tolerance for an absent pool.
///
/// Reopen is LAZY (`DbPool::reopen` → `open_existing_lazy`), which is what
/// makes this safe to run immediately after sending the signal rather than
/// polling until the new daemon process is confirmed up: no physical
/// connection is made here, only a fresh, empty pool object with no
/// connections cached from the process generation that just exited. The
/// first REAL connection happens on whatever read comes next (typically the
/// following poll tick), against whatever the file looks like by then - a
/// point in time this function does not need to reason about.
async fn reload_with_pool_cycle(pool: &crate::db_pool::DbPool) -> Result<u32, String> {
    with_reload_lock(|| async {
        // Also excludes a concurrent `DbPool::recover_if_corrupt`, which closes and
        // reopens this same handle. `with_reload_lock` alone only serialises reloads
        // against each other, so without this a recycle could REOPEN the pool in the
        // window between the close below and the restart signal - leaving a live
        // connection spanning two daemon generations, which is the 2026-08-24
        // corruption profile this whole close/reopen dance exists to prevent. See
        // `DbPool::lock_cycle`.
        let _cycle = pool.lock_cycle().await;
        pool.close().await;
        let result = super::daemon_control::reload().await;
        pool.reopen().await;
        result
    })
    .await
}

/// Run one close → signal → reopen cycle with no other cycle interleaved.
///
/// # The interleaving this prevents
/// `DbPool::close` clears the SHARED handle before awaiting the underlying
/// close, so two overlapping reloads can cross:
///
/// 1. caller A closes - the handle is now `None`, A is still awaiting;
/// 2. caller B sees `None`, treats it as "nothing to close", signals its own
///    restart and REOPENS a fresh pool;
/// 3. A resumes, signals a second restart, and reopens over B's pool -
///    signalling a daemon restart while a live pool is open across it, which is
///    precisely what the close/reopen dance exists to prevent.
///
/// macOS reached this through `reload_throttled`, which serialises for a
/// different reason (launchd's `ThrottleInterval`) and so happened to cover it;
/// the Windows path deliberately skips that throttle and had nothing. The lock
/// lives HERE rather than at either call site so both platforms get it from one
/// place, and it nests inside the throttle without any ordering hazard.
///
/// Generic over the work so the serialisation itself is testable without
/// launchctl or a real pool - see `two_reload_cycles_never_interleave`.
async fn with_reload_lock<F, Fut, T>(work: F) -> T
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = T>,
{
    static RELOAD_LOCK: std::sync::LazyLock<tokio::sync::Mutex<()>> =
        std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));
    let _guard = RELOAD_LOCK.lock().await;
    work().await
}

// ── macOS reload rate limit ─────────────────────────────────────────────────

/// launchd's `ThrottleInterval` for `com.meridiona.daemon`, verbatim from
/// `scripts/com.meridiona.daemon.plist`.
#[cfg(target_os = "macos")]
const LAUNCHD_THROTTLE: Duration = Duration::from_secs(30);

/// Slack added to [`LAUNCHD_THROTTLE`] to get [`THROTTLE_INTERVAL`].
///
/// Not padding. launchd measures its interval from the job's **start**, while
/// all we can observe is when we sent the signal — and between the two sits the
/// daemon's graceful shutdown (pool close, telemetry flush). Waiting exactly
/// `ThrottleInterval` from our own signal therefore lands *inside* the window
/// launchd is enforcing, by however long that shutdown takes, re-creating a
/// smaller version of the outage this module exists to remove. Approach the
/// boundary from the safe side instead.
#[cfg(target_os = "macos")]
const SHUTDOWN_MARGIN: Duration = Duration::from_secs(2);

/// How long we actually wait between signals.
#[cfg(target_os = "macos")]
const THROTTLE_INTERVAL: Duration =
    Duration::from_secs(LAUNCHD_THROTTLE.as_secs() + SHUTDOWN_MARGIN.as_secs());

/// Attempts for a deferred reload whose signal failed. The scheduled moment is
/// the *most* likely one to fail — it is when launchd is relaunching, so the
/// probe inside `reload()` can still report the daemon down — and dropping it
/// there would silently lose the credential change it was carrying.
#[cfg(target_os = "macos")]
const DEFERRED_SEND_ATTEMPTS: u32 = 3;

/// Gap between those attempts.
#[cfg(target_os = "macos")]
const DEFERRED_RETRY_GAP: Duration = Duration::from_secs(3);

/// What to do with a reload request. See [`plan_reload`].
#[cfg(target_os = "macos")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReloadAction {
    /// Signal the daemon now.
    SendNow,
    /// Inside the throttle window - signal after this long instead.
    Defer(Duration),
    /// A deferred reload is already scheduled; it will cover this request too.
    AlreadyPending,
}

#[cfg(target_os = "macos")]
#[derive(Default)]
struct ReloadState {
    last_sent: Option<Instant>,
    pending: bool,
}

#[cfg(target_os = "macos")]
fn reload_state() -> Arc<Mutex<ReloadState>> {
    static STATE: OnceLock<Arc<Mutex<ReloadState>>> = OnceLock::new();
    STATE
        .get_or_init(|| Arc::new(Mutex::new(ReloadState::default())))
        .clone()
}

/// Lock helper that keeps working after a panic elsewhere poisoned the mutex.
///
/// A poisoned lock here used to be unrecoverable in the worst way: the guard
/// state would stay latched and every later reload would be silently dropped
/// for the life of the process. The state this protects is two plain fields
/// with no invariant a panic could have torn, so recovering is strictly better
/// than propagating. Also keeps this off `unwrap`/`expect`, per CLAUDE.md.
#[cfg(target_os = "macos")]
fn lock_state(state: &Mutex<ReloadState>) -> std::sync::MutexGuard<'_, ReloadState> {
    state.lock().unwrap_or_else(|e| e.into_inner())
}

/// Clears `pending` even if the deferred task unwinds.
///
/// Without this, a panic between scheduling and clearing latches `pending` at
/// `true` forever: [`plan_reload`] then answers every future request
/// `AlreadyPending`, so token saves, OAuth completions and the reload button
/// all return success and do nothing, with no log and no recovery short of
/// quitting the tray. A guard that fails silently and permanently is worse than
/// the bug it guards.
#[cfg(target_os = "macos")]
struct PendingGuard(Arc<Mutex<ReloadState>>);

#[cfg(target_os = "macos")]
impl Drop for PendingGuard {
    fn drop(&mut self) {
        lock_state(&self.0).pending = false;
    }
}

/// The rate-limited reload. `send` performs the signal; `is_running` reports
/// whether the daemon is up. Both are injected so the state machine — not just
/// [`plan_reload`] — is testable without launchd, a socket or a real clock.
#[cfg(target_os = "macos")]
async fn reload_throttled<S, SFut, P, PFut>(
    state: Arc<Mutex<ReloadState>>,
    send: S,
    is_running: P,
) -> Result<ReloadResponse, String>
where
    S: Fn() -> SFut + Send + Sync + Clone + 'static,
    SFut: std::future::Future<Output = Result<u32, String>> + Send,
    P: Fn() -> PFut,
    PFut: std::future::Future<Output = bool>,
{
    let action = {
        let st = lock_state(&state);
        plan_reload(st.last_sent, st.pending, Instant::now(), THROTTLE_INTERVAL)
    };

    match action {
        ReloadAction::SendNow => {
            // Stamp BEFORE awaiting the send. `send` probes and spawns a
            // process, so leaving the stamp until afterwards left a window in
            // which a second caller read the old `last_sent` and was also told
            // to send - the two credential call sites are not synchronised with
            // each other, so that overlap is reachable.
            lock_state(&state).last_sent = Some(Instant::now());
            match send().await {
                Ok(pid) => {
                    tracing::info!(pid, "daemon reload requested");
                    Ok(ReloadResponse { ok: true, pid })
                }
                Err(e) => {
                    // Nothing was signalled, so no throttle window opened;
                    // clear the stamp so the next attempt is not made to wait
                    // out a restart that never happened.
                    lock_state(&state).last_sent = None;
                    Err(e)
                }
            }
        }

        ReloadAction::Defer(wait) => {
            // A down daemon needs no reload at all - it reads `.env` and
            // `settings.json` when it starts. Report it as the error the
            // callers already understand, so the UI keeps telling the user
            // their change applies on next start. Without this probe the
            // deferred path answered `ok` unconditionally and that warning
            // became unreachable for the whole window.
            if !is_running().await {
                return Err("daemon not running".to_string());
            }

            {
                let mut st = lock_state(&state);
                if st.pending {
                    tracing::info!("daemon reload coalesced into the pending one");
                    return Ok(ReloadResponse { ok: true, pid: 0 });
                }
                st.pending = true;
            }

            let task_state = state.clone();
            let task_send = send.clone();
            tokio::spawn(async move {
                tokio::time::sleep(wait).await;
                let _guard = PendingGuard(task_state.clone());
                {
                    // Clear `pending` and stamp the send in ONE critical
                    // section, before the signal goes out. Clearing after the
                    // send would drop any request that arrived while it was in
                    // flight - and such a request's `.env` write happened after
                    // the reload it got folded into, so it would genuinely be
                    // lost. Requests arriving from here on see `pending: false`
                    // with a fresh stamp and schedule their own trailing
                    // reload, which does cover them.
                    let mut st = lock_state(&task_state);
                    st.pending = false;
                    st.last_sent = Some(Instant::now());
                }
                for attempt in 1..=DEFERRED_SEND_ATTEMPTS {
                    match task_send().await {
                        Ok(pid) => {
                            tracing::info!(pid, attempt, "daemon reload sent (deferred)");
                            return;
                        }
                        Err(e) if attempt == DEFERRED_SEND_ATTEMPTS => {
                            tracing::warn!(
                                error = %e,
                                attempts = DEFERRED_SEND_ATTEMPTS,
                                "deferred daemon reload failed - config applies on next start"
                            );
                        }
                        Err(e) => {
                            tracing::info!(
                                error = %e,
                                attempt,
                                "deferred daemon reload not ready - retrying"
                            );
                            tokio::time::sleep(DEFERRED_RETRY_GAP).await;
                        }
                    }
                }
            });

            tracing::info!(
                wait_s = wait.as_secs(),
                "daemon reload deferred - inside launchd's throttle window"
            );
            // `pid: 0` = nothing signalled yet; the reload IS going to happen.
            Ok(ReloadResponse { ok: true, pid: 0 })
        }

        ReloadAction::AlreadyPending => {
            tracing::info!("daemon reload coalesced into the pending one");
            Ok(ReloadResponse { ok: true, pid: 0 })
        }
    }
}

/// Decide when a reload may actually be signalled.
///
/// # Why this exists
/// On macOS a "reload" is `kill -HUP`, and the daemon handles SIGHUP by
/// **exiting cleanly** so launchd relaunches it (`src/platform/unix.rs`).
/// launchd will not relaunch a job more often than the plist's
/// `ThrottleInterval` (30 s), measured from the job's last *start* - so a second
/// SIGHUP inside that window does not reload the daemon, it **kills it for the
/// remainder of the window**.
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
#[cfg(target_os = "macos")]
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
        // `saturating_duration_since` rather than `-`: a monotonic clock cannot
        // go backwards, but saturating keeps a future stamp from panicking.
        Some(prev) => match throttle.checked_sub(now.saturating_duration_since(prev)) {
            Some(remaining) if !remaining.is_zero() => ReloadAction::Defer(remaining),
            _ => ReloadAction::SendNow,
        },
        None => ReloadAction::SendNow,
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {

    /// Two overlapping reloads must not interleave their close/reopen cycles.
    ///
    /// `DbPool::close` clears the shared handle BEFORE awaiting the underlying
    /// close, so without serialisation a second caller sees `None`, decides
    /// there is nothing to close, and reopens a pool that the first caller then
    /// signals a daemon restart across - the exact condition close/reopen
    /// exists to prevent.
    ///
    /// Written against `with_reload_lock` rather than `reload_with_pool_cycle`
    /// because the latter shells out to launchctl; the property under test is
    /// the mutual exclusion, and this exercises it directly.
    #[tokio::test]
    async fn two_reload_cycles_never_interleave() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc as StdArc;

        let inside = StdArc::new(AtomicUsize::new(0));
        let max_seen = StdArc::new(AtomicUsize::new(0));

        let mut handles = Vec::new();
        for _ in 0..8 {
            let inside = inside.clone();
            let max_seen = max_seen.clone();
            handles.push(tokio::spawn(async move {
                with_reload_lock(|| async {
                    let n = inside.fetch_add(1, Ordering::SeqCst) + 1;
                    max_seen.fetch_max(n, Ordering::SeqCst);
                    // Yield across the critical section: without the lock this
                    // is where a second cycle slips in, exactly as
                    // `close().await` yields in the real one.
                    tokio::task::yield_now().await;
                    tokio::time::sleep(std::time::Duration::from_millis(2)).await;
                    inside.fetch_sub(1, Ordering::SeqCst);
                })
                .await;
            }));
        }
        for h in handles {
            h.await.expect("task panicked");
        }

        assert_eq!(
            max_seen.load(Ordering::SeqCst),
            1,
            "two reload cycles were inside the critical section at once"
        );
    }
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    // ── The planner ─────────────────────────────────────────────────────────

    /// The exact field sequence from 1.84.0-staging.7 (see [`plan_reload`]):
    /// two reloads 6 s apart. The second MUST NOT be signalled - doing so exits
    /// the daemon into launchd's throttle window and takes it down for the
    /// remainder of it, which is what produced the bogus "database disk image
    /// is malformed" in the integrations panel.
    #[test]
    fn a_second_reload_six_seconds_later_is_deferred_not_signalled() {
        let start = Instant::now();
        assert_eq!(
            plan_reload(None, false, start, THROTTLE_INTERVAL),
            ReloadAction::SendNow,
            "the first reload always sends"
        );

        let six_s_later = start + Duration::from_secs(6);
        assert_eq!(
            plan_reload(Some(start), false, six_s_later, THROTTLE_INTERVAL),
            ReloadAction::Defer(THROTTLE_INTERVAL - Duration::from_secs(6)),
            "must wait out the rest of the throttle, not SIGHUP into it"
        );
    }

    /// The invariant the whole module exists for, swept across the window
    /// rather than spot-checked - including the sub-second edge, which is
    /// where `remaining.is_zero()` actually decides and where a whole-second
    /// sweep would step straight over the bug.
    #[test]
    fn nothing_inside_the_throttle_window_is_ever_signalled_immediately() {
        let start = Instant::now();
        let mut offsets: Vec<Duration> = (0..THROTTLE_INTERVAL.as_secs())
            .map(Duration::from_secs)
            .collect();
        offsets.push(THROTTLE_INTERVAL - Duration::from_millis(1));
        offsets.push(THROTTLE_INTERVAL - Duration::from_nanos(1));

        for offset in offsets {
            match plan_reload(Some(start), false, start + offset, THROTTLE_INTERVAL) {
                ReloadAction::Defer(wait) => assert_eq!(
                    offset + wait,
                    THROTTLE_INTERVAL,
                    "a deferral at +{offset:?} must fire exactly when the window closes"
                ),
                other => panic!("reload at +{offset:?} must be deferred, got {other:?}"),
            }
        }
    }

    /// Once the window has closed launchd will relaunch immediately again, so
    /// the reload must actually go out - a guard that never re-armed would
    /// leave credential changes needing a manual restart.
    #[test]
    fn a_reload_after_the_window_closes_is_signalled() {
        let start = Instant::now();
        for offset in [
            THROTTLE_INTERVAL,
            THROTTLE_INTERVAL + Duration::from_secs(1),
        ] {
            assert_eq!(
                plan_reload(Some(start), false, start + offset, THROTTLE_INTERVAL),
                ReloadAction::SendNow,
                "a reload at +{offset:?} is outside the throttle and must send"
            );
        }
    }

    /// A burst (token save + OAuth completion + a retry) must collapse into the
    /// ONE already-scheduled reload.
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

    /// The margin is the whole point of [`THROTTLE_INTERVAL`]: launchd measures
    /// from the daemon's START, we can only observe our own SIGHUP, and the
    /// graceful shutdown sits between them. Waiting exactly the plist value
    /// lands inside the window launchd enforces.
    #[test]
    fn the_throttle_we_wait_clears_the_one_launchd_enforces() {
        let plist = include_str!("../../../../scripts/com.meridiona.daemon.plist");
        let value: u64 = plist
            .split("<key>ThrottleInterval</key>")
            .nth(1)
            .and_then(|s| s.split("<integer>").nth(1))
            .and_then(|s| s.split("</integer>").next())
            .expect("the plist declares ThrottleInterval")
            .trim()
            .parse()
            .expect("ThrottleInterval parses");

        assert_eq!(
            value,
            LAUNCHD_THROTTLE.as_secs(),
            "LAUNCHD_THROTTLE drifted from the plist launchd actually reads"
        );
        assert!(
            THROTTLE_INTERVAL > LAUNCHD_THROTTLE,
            "waiting {THROTTLE_INTERVAL:?} does not clear launchd's {LAUNCHD_THROTTLE:?} \
             once the daemon's graceful shutdown is counted"
        );
    }

    // ── The state machine ───────────────────────────────────────────────────
    //
    // Everything above is a pure function; every defect that reached review
    // lived out here, in the stateful wrapper. These drive `reload_throttled`
    // with injected send/probe so the sequencing is exercised for real.

    fn fresh_state() -> Arc<Mutex<ReloadState>> {
        Arc::new(Mutex::new(ReloadState::default()))
    }

    #[tokio::test]
    async fn the_first_reload_signals_and_a_prompt_second_one_does_not() {
        let state = fresh_state();
        let calls = Arc::new(AtomicU32::new(0));

        let c = calls.clone();
        let send = move || {
            let c = c.clone();
            async move {
                c.fetch_add(1, Ordering::SeqCst);
                Ok(4242u32)
            }
        };
        let running = || async { true };

        let first = reload_throttled(state.clone(), send.clone(), running)
            .await
            .expect("first reload sends");
        assert_eq!(first.pid, 4242);
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        // Immediately again: must NOT reach the daemon.
        let second = reload_throttled(state.clone(), send.clone(), running)
            .await
            .expect("second reload is accepted but deferred");
        assert_eq!(second.pid, 0, "nothing was signalled");
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "a second SIGHUP inside the window is exactly the outage"
        );

        // …and a third is folded into the pending one rather than queuing its
        // own trailing SIGHUP.
        let third = reload_throttled(state.clone(), send, running)
            .await
            .expect("third is coalesced");
        assert_eq!(third.pid, 0);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(
            lock_state(&state).pending,
            "one deferred reload is scheduled"
        );
    }

    /// A failed signal opened no throttle window, so it must not make the next
    /// attempt wait one out.
    #[tokio::test]
    async fn a_failed_send_does_not_start_a_throttle_window() {
        let state = fresh_state();
        let send = || async { Err::<u32, String>("kill failed".to_string()) };
        let running = || async { true };

        let err = reload_throttled(state.clone(), send, running)
            .await
            .expect_err("the send failed, so the command fails");
        assert!(err.contains("kill failed"));
        assert!(
            lock_state(&state).last_sent.is_none(),
            "a signal that never landed must not stamp a window"
        );
    }

    /// The warning the integrations UI shows ("your credentials apply when the
    /// daemon starts") is driven by this Err. Inside the throttle window the
    /// deferred path used to answer `ok` unconditionally, making it unreachable
    /// exactly when the daemon was most likely to be down.
    #[tokio::test]
    async fn a_down_daemon_still_reports_an_error_inside_the_window() {
        let state = fresh_state();
        // Pretend a reload just went out, so the next request lands in the window.
        lock_state(&state).last_sent = Some(Instant::now());

        let calls = Arc::new(AtomicU32::new(0));
        let c = calls.clone();
        let send = move || {
            let c = c.clone();
            async move {
                c.fetch_add(1, Ordering::SeqCst);
                Ok(1u32)
            }
        };

        let err = reload_throttled(state.clone(), send, || async { false })
            .await
            .expect_err("a down daemon must surface as an error, not a silent ok");
        assert_eq!(err, "daemon not running");
        assert_eq!(calls.load(Ordering::SeqCst), 0, "nothing to signal");
        assert!(
            !lock_state(&state).pending,
            "no trailing reload is scheduled for a daemon that will read config on start"
        );
    }

    /// The latch that would have bricked reload for the life of the process:
    /// `pending` stuck at `true` answers every later request `AlreadyPending`,
    /// silently dropping it. It must clear even when the deferred send fails.
    #[tokio::test(start_paused = true)]
    async fn a_deferred_reload_clears_pending_and_fires_even_if_the_send_fails() {
        let state = fresh_state();
        lock_state(&state).last_sent = Some(Instant::now());

        let calls = Arc::new(AtomicU32::new(0));
        let c = calls.clone();
        // Always fails - the worst case for the latch.
        let send = move || {
            let c = c.clone();
            async move {
                c.fetch_add(1, Ordering::SeqCst);
                Err::<u32, String>("daemon not running".to_string())
            }
        };

        reload_throttled(state.clone(), send, || async { true })
            .await
            .expect("deferred");
        assert!(lock_state(&state).pending, "scheduled");

        // Past the window and every retry gap.
        tokio::time::sleep(THROTTLE_INTERVAL + DEFERRED_RETRY_GAP * DEFERRED_SEND_ATTEMPTS * 2)
            .await;

        assert_eq!(
            calls.load(Ordering::SeqCst),
            DEFERRED_SEND_ATTEMPTS,
            "the deferred send retries instead of being dropped at the one moment \
             launchd is most likely to still be relaunching"
        );
        assert!(
            !lock_state(&state).pending,
            "pending must clear, or every future reload is silently discarded"
        );
    }

    /// After the deferred reload has gone out, the guard must be usable again -
    /// and a request arriving later must not be swallowed by a stale `pending`.
    #[tokio::test(start_paused = true)]
    async fn the_guard_re_arms_after_a_deferred_reload() {
        let state = fresh_state();
        lock_state(&state).last_sent = Some(Instant::now());

        let calls = Arc::new(AtomicU32::new(0));
        let c = calls.clone();
        let send = move || {
            let c = c.clone();
            async move {
                c.fetch_add(1, Ordering::SeqCst);
                Ok(7u32)
            }
        };

        reload_throttled(state.clone(), send.clone(), || async { true })
            .await
            .expect("deferred");
        tokio::time::sleep(THROTTLE_INTERVAL + Duration::from_secs(1)).await;
        assert_eq!(calls.load(Ordering::SeqCst), 1, "the deferred send fired");
        assert!(!lock_state(&state).pending);

        // Far enough past that send for the window to have closed again.
        tokio::time::sleep(THROTTLE_INTERVAL + Duration::from_secs(1)).await;
        reload_throttled(state.clone(), send, || async { true })
            .await
            .expect("re-armed");
        assert_eq!(
            calls.load(Ordering::SeqCst),
            2,
            "a later reload still reaches the daemon"
        );
    }

    /// Every close/reopen of the tray's pool must hold `DbPool::lock_cycle`, and
    /// `reload_with_pool_cycle` must take it BEFORE it closes.
    ///
    /// `with_reload_lock` only serialises reloads against each other.
    /// `DbPool::recover_if_corrupt` is a second close/reopen site, so without a
    /// shared lock the two interleave:
    ///
    /// 1. reload closes - the handle is `None`;
    /// 2. a recycle sees `None`, treats it as nothing to close, and REOPENS;
    /// 3. reload signals the daemon restart, with that fresh pool open across it.
    ///
    /// Step 3 is the 2026-08-24 corruption profile - a tray connection spanning two
    /// daemon generations - i.e. the self-heal would have reintroduced the very bug
    /// this close/reopen dance exists to prevent. Source-scanned because reproducing
    /// it needs a real launchd daemon restart racing a real corrupt write.
    #[test]
    fn the_reload_cycle_holds_the_shared_pool_lock_before_closing() {
        const SRC: &str = include_str!("daemon.rs");
        let prod = SRC
            .split_once("\n#[cfg(test)]")
            .map_or(SRC, |(before, _)| before);
        let body = prod
            .split_once("async fn reload_with_pool_cycle")
            .expect("reload_with_pool_cycle must exist")
            .1;

        let lock_pos = body
            .find("lock_cycle().await")
            .expect("reload_with_pool_cycle must acquire DbPool::lock_cycle");
        let close_pos = body
            .find("pool.close().await")
            .expect("reload_with_pool_cycle must close the pool");
        assert!(
            lock_pos < close_pos,
            "the shared cycle lock must be held BEFORE the close - taking it after \
             leaves the window where a concurrent recycle can reopen the pool into \
             the daemon restart. lock at {lock_pos}, close at {close_pos}."
        );
    }
}
