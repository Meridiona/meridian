//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Taking the daemon down when the user says so - on quit, and on the tray
//! menu's Connected / Disconnected toggle.
//!
//! # Why this exists
//!
//! The daemon is a separate OS process under launchd (macOS) or the Task
//! Scheduler (Windows), so quitting the tray never touched it. That was wrong
//! in three ways at once:
//!
//! - **`meridian db repair` was unreachable.** The `db.corrupt` notice tells
//!   the user to quit Meridian and run it, but [`meridian::db::repair`]'s
//!   `ensure_no_writers` checks the daemon first and refuses while it is up. The
//!   documented recovery dead-ended on its own instruction, and the last
//!   incident had to be driven by hand with `launchctl bootout`.
//! - **"Quit" was not honest.** Capture runs in *this* process, so quitting
//!   already stopped tracking - but the surviving daemon kept polling, kept
//!   `meridian.db` open, and kept the summariser spawning third-party LLM CLIs
//!   and making network calls after the user had closed the app.
//! - **"Pause" did nothing.** It ran `daemon_control::set_running(false)` =
//!   `launchctl stop`, and the plist sets `KeepAlive=true`, so launchd put the
//!   daemon straight back after `ThrottleInterval` (30 s). The menu read
//!   "Disconnected ○" over a running daemon.
//!
//! # `bootout`, not `stop`
//!
//! `launchctl stop` cannot hold a `KeepAlive` job down; `bootout` removes it
//! from the domain, and nothing resurrects it until something bootstraps it
//! again. [`crate::backend_install::stop_daemon_for_migration`] already relies
//! on exactly that, for exactly that reason.
//!
//! [`meridian::db::repair::marker`]'s header warns that *automating* a bootout
//! risks the wedge that once left a plist needing a hand reinstall. That warning
//! is about `launchctl disable`, an override that persists across boots and is
//! the thing that actually wedged (a screenpipe plist, historically). We never
//! call it. The bootout here goes through
//! [`crate::backend_install::bootout_agent_and_wait`], which polls until the
//! launchd entry has genuinely cleared - the wait that prevents a following
//! `bootstrap` from returning EIO, and the same path `register_agent` takes on
//! every single install. The marker remains the right tool for *repair*, whose
//! requirement is different: the daemon must stay down across an unbounded
//! number of relaunches by a tray that may itself be crash-looping.
//!
//! # The invariant that makes this safe
//!
//! **The tray restores the daemon on launch.** A stop here is only ever "until
//! the tray comes back", and [`crate::backend_install::ensure_backend_installed`]
//! re-registers a booted-out agent on every start, on the packaged and the
//! dev/source path alike. Without that half, a single quit would leave the
//! daemon down permanently - `RunAtLoad` cannot fire for a job that is no longer
//! loaded, so not even the next login would recover it.
//!
//! Pause state is process-local ([`DAEMON_PAUSED`]) and deliberately not
//! persisted, matching the capture pause in [`crate::commands::pause`]:
//! relaunching the tray resumes the daemon. The alternative - a pause that
//! outlives the process that set it - is the failure mode `repair::marker` had
//! to grow an expiry to escape.
//!
//! # Who calls this
//!
//! - [`decide_exit`] / [`stop_for_quit`] - [`crate::run`]'s
//!   `RunEvent::ExitRequested` handler.
//! - [`restore_unless_paused`] - every bail-out in
//!   [`crate::backend_install::ensure_backend_installed`].
//! - [`stop_for_pause`] / [`resume_from_pause`] - [`crate::commands::daemon::toggle_daemon`],
//!   behind the tray menu's Connected / Disconnected item.
//!
//! # Related
//!
//! - [`crate::backend_install`] - owns the OS mechanics and the restore half.
//! - [`crate::poll::watchdog`] - must not fight a pause; it reads
//!   [`is_paused`] each tick and its `decide` takes `daemon_paused` for that
//!   reason.
//! - [`crate::commands::daemon_control`] - the probe and the launchd verbs.

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::time::Duration;
use tracing::Instrument;

/// How long a quit waits for the daemon to go down before giving up and exiting
/// anyway.
///
/// The bootout itself is near-instant; this covers launchd taking its time to
/// clear the entry. Bounded because a quit that hangs is worse than a daemon
/// that outlives it by a few seconds: the user asked for the app to close, and
/// the next launch reconciles the daemon regardless.
const QUIT_STOP_BUDGET: Duration = Duration::from_secs(5);

/// Where the app is in its exit sequence.
///
/// The quit path is not atomic - the daemon stop is async and the exit is not -
/// so "are we exiting?" is a three-state question, not a boolean. An earlier
/// version used a single `already_stopping` latch and got this wrong: a second
/// `ExitRequested` arriving mid-stop found the latch set, declined to prevent
/// the exit, and Tauri tore the process down with the stop task still in
/// flight. That is not a hypothetical - the quit appears to hang for a moment,
/// which is exactly when a user presses Quit or Cmd+Q again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ExitPhase {
    /// No exit in progress.
    Running,
    /// The daemon stop is in flight; every exit must be held.
    Stopping,
    /// The stop is done and the internal `exit()` is about to fire. The one
    /// phase in which an exit is allowed straight through.
    ReadyToExit,
}

/// What the `RunEvent::ExitRequested` handler should do this time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ExitAction {
    /// Let the exit proceed untouched.
    Proceed,
    /// Hold the exit and start the daemon stop.
    HoldAndStop,
    /// Hold the exit. A stop is already running and will finish the job.
    Hold,
}

/// The exit policy.
///
/// Pure and free of `cfg` so it is pinned by tests on every platform - the same
/// discipline as [`crate::poll::watchdog::decide`] and
/// [`crate::commands::daemon_control`]'s `status_running`, and for the same
/// reason: everything around it shells out to the OS or needs a live event
/// loop, and cannot be tested at all.
///
/// `code` is `RunEvent::ExitRequested`'s: `None` for a user-driven quit (the
/// tray menu item, macOS Cmd+Q), `Some` when something called
/// `AppHandle::exit` (the popover's Quit button, and our own internal exit) or
/// `AppHandle::restart`.
///
/// [`tauri::RESTART_EXIT_CODE`] is never held and never stops the daemon. Both
/// `update.rs`'s post-install relaunch and `commands::repair`'s
/// restart-into-`repair_boot` call `AppHandle::restart`, which routes through
/// this event from a command thread. The app is coming straight back in both
/// cases, so a bootout would be pure latency on the update path - and on the
/// repair path it would fight the marker handshake already holding the daemon
/// off. Holding a restart would be worse still: the pending stop finishes with
/// `exit(0)`, which would silently turn the user's restart into a plain quit.
pub(crate) fn decide_exit(code: Option<i32>, phase: ExitPhase) -> ExitAction {
    if phase == ExitPhase::ReadyToExit || code == Some(tauri::RESTART_EXIT_CODE) {
        return ExitAction::Proceed;
    }
    match phase {
        ExitPhase::Running => ExitAction::HoldAndStop,
        ExitPhase::Stopping => ExitAction::Hold,
        ExitPhase::ReadyToExit => ExitAction::Proceed,
    }
}

/// [`ExitPhase`], as a process-global the event-loop closure can reach without
/// captured state.
static EXIT_PHASE: AtomicU8 = AtomicU8::new(0);

/// The current exit phase.
pub(crate) fn exit_phase() -> ExitPhase {
    match EXIT_PHASE.load(Ordering::SeqCst) {
        1 => ExitPhase::Stopping,
        2 => ExitPhase::ReadyToExit,
        _ => ExitPhase::Running,
    }
}

/// Advance the exit phase.
pub(crate) fn set_exit_phase(phase: ExitPhase) {
    let v = match phase {
        ExitPhase::Running => 0,
        ExitPhase::Stopping => 1,
        ExitPhase::ReadyToExit => 2,
    };
    EXIT_PHASE.store(v, Ordering::SeqCst);
}

/// Did the daemon actually come back after a resume?
///
/// `Some(true)`/`Some(false)` are verified answers; `None` means the platform
/// has no liveness check wired (Windows - see
/// [`crate::commands::daemon_control::process_alive`]) and must never be read
/// as a failure.
///
/// Polled rather than asked once: on macOS the restore ends in `launchctl
/// kickstart`, and the process takes a moment to appear. A single immediate
/// query would report a healthy resume as failed on a slow machine, which is
/// the same false-signal problem in the opposite direction.
async fn verify_daemon_came_back() -> Option<bool> {
    for attempt in 0..RESUME_VERIFY_ATTEMPTS {
        match crate::commands::daemon_control::process_alive().await {
            Some(true) => return Some(true),
            // Unknown is terminal: it is a property of the platform, not a
            // timing artefact, so retrying cannot change it.
            None => return None,
            Some(false) => {
                if attempt + 1 < RESUME_VERIFY_ATTEMPTS {
                    tokio::time::sleep(RESUME_VERIFY_INTERVAL).await;
                }
            }
        }
    }
    Some(false)
}

/// How many times [`verify_daemon_came_back`] asks before calling a resume
/// failed.
const RESUME_VERIFY_ATTEMPTS: u32 = 4;

/// Gap between those attempts - ~1.5 s total, comfortably inside the tray's
/// menu-click responsiveness budget and well under the watchdog's first
/// opportunity to act.
const RESUME_VERIFY_INTERVAL: std::time::Duration = std::time::Duration::from_millis(500);

/// Releases a held exit if the stop task dies without finishing.
///
/// [`ExitPhase::Stopping`] holds **every** subsequent `ExitRequested` via
/// [`ExitAction::Hold`], and only the stop task advances past it. So a panic in
/// that task - anywhere before its `set_exit_phase(ReadyToExit)` - leaves the
/// phase at `Stopping` with nothing left alive to move it, and the tray becomes
/// permanently unquittable: every later quit is held, and no path resets the
/// phase. The user's only way out is killing the process.
///
/// [`stop_for_quit`] is documented as infallible and is bounded by
/// `QUIT_STOP_BUDGET`, so this needs a panic in the platform stop path to
/// trigger. It is guarded anyway because the failure is unrecoverable from
/// inside the app, and the guard costs one `compare_exchange` on the way out.
///
/// Deliberately a **conditional** advance: the normal path sets `ReadyToExit`
/// explicitly right before `exit(0)`, and this must not disturb that (nor
/// promote a phase that has since been reset to `Running`). Only `Stopping`
/// - the stuck state - is advanced.
pub(crate) struct HeldExitGuard;

impl Drop for HeldExitGuard {
    fn drop(&mut self) {
        // 1 -> 2, and only 1 -> 2. Fails harmlessly on the normal path, where
        // the explicit `set_exit_phase(ReadyToExit)` has already stored 2.
        let _ = EXIT_PHASE.compare_exchange(1, 2, Ordering::SeqCst, Ordering::SeqCst);
    }
}

/// Whether the user has paused the daemon from the tray menu.
///
/// A process-global rather than a field on [`crate::state::AppState`] because
/// two unrelated callers need it and neither has an `AppHandle` in hand: the
/// watchdog runs on its own bare task, and the installer's restore reaches it
/// several call frames below `ensure_backend_installed`. There is exactly one
/// daemon per tray process, so a global is the honest shape.
static DAEMON_PAUSED: AtomicBool = AtomicBool::new(false);

/// Serializes every daemon lifecycle transition against every other one.
///
/// The flag alone is not enough. Without this the installer can read "not
/// paused", the user can pause and complete a bootout, and the installer can
/// then register the agent anyway - leaving a running daemon behind a UI that
/// says Paused, which the watchdog will not correct precisely because the pause
/// flag is set. The window is small and the resulting state is stuck until the
/// user toggles again, which is the worst combination.
///
/// A `tokio` mutex, not a `std` one: it is held across `await` points that
/// shell out to `launchctl` for seconds at a time.
static LIFECYCLE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Whether the daemon is deliberately paused. Read by
/// [`crate::poll::watchdog`], which must not "recover" a pause.
pub(crate) fn is_paused() -> bool {
    DAEMON_PAUSED.load(Ordering::Relaxed)
}

/// Set while the installer is stopping the daemon in order to replace its
/// binary — see [`begin_staging`].
static DAEMON_STAGING: AtomicBool = AtomicBool::new(false);

/// Whether the installer is mid-swap of the daemon binary. Read by
/// [`crate::poll::watchdog`] and [`crate::poll::refresh`], both of which start a
/// daemon they find stopped and must not do so while the installer is
/// deliberately holding it down.
///
/// SEPARATE FROM [`is_paused`] on purpose, though both make the watchdog stand
/// down. Pause is the user's instruction and persists until they revoke it;
/// this is a few seconds of internal exclusion the user never sees, and
/// borrowing the pause flag for it would surface "Paused" in the tray menu
/// during every update — and, worse, leave the daemon genuinely paused if the
/// installer died before clearing it.
pub(crate) fn is_staging() -> bool {
    DAEMON_STAGING.load(Ordering::Relaxed)
}

/// Claim the staging window, clearing it when the returned guard drops.
///
/// # Why this exists
///
/// The tray is BOTH the daemon's installer and — on Windows, where there is no
/// launchd `KeepAlive` — its only supervisor, and the two halves used to fight
/// each other with no interlock at all:
///
/// 1. `ensure_backend_installed` kills the running daemon so it can overwrite
///    `~/.meridian/bin/meridian.exe` (Windows keeps a running exe's pages
///    mapped, so the copy fails with os error 32 otherwise).
/// 2. Roughly 10 s later — [`crate::poll::watchdog`]'s `TICK` × `STRIKES` — the
///    watchdog notices the endpoint has gone silent and starts the daemon back
///    up, from the very path being staged.
/// 3. The installer's post-kill poll then finds a live process holding the
///    binary, and fails the whole install with "still running after stop
///    attempts (pids: […]) - cannot overwrite a locked binary".
///
/// The daemon it reported as un-killable was its own supervisor's child,
/// spawned seconds earlier. The watchdog's existing liveness guard does not
/// help: `process_alive()` is `None` on Windows, which [`crate::poll::watchdog::decide`]
/// treats as non-blocking by design.
///
/// This only bites on an UPDATE — a first install has no running daemon to
/// stop, returns immediately, and never wakes the watchdog. That is why the
/// failure looked intermittent while being, on the update path, reliable.
///
/// # Shape
///
/// An `AtomicBool` + RAII guard rather than [`LIFECYCLE`]. The mutex would also
/// serialise the two, but the watchdog would then block on it for the length of
/// an install and start the daemon the instant it was released — correct, but it
/// makes a 5 s-budgeted [`stop_for_quit`] queue behind a multi-second install,
/// and a flag keeps [`crate::poll::watchdog::decide`] a pure function that can
/// be tested for this exact case. The guard is what makes an early `?` return
/// or a panic mid-install clear the flag rather than wedging the watchdog off
/// for the rest of the session.
pub(crate) fn begin_staging() -> StagingGuard {
    DAEMON_STAGING.store(true, Ordering::Relaxed);
    StagingGuard
}

/// Clears the [`begin_staging`] flag on drop.
pub(crate) struct StagingGuard;

impl Drop for StagingGuard {
    fn drop(&mut self) {
        DAEMON_STAGING.store(false, Ordering::Relaxed);
    }
}

/// Take the daemon down because the app is quitting.
///
/// Bounded by [`QUIT_STOP_BUDGET`] and **infallible by design**: every failure
/// path logs and returns, because the caller's next act is to exit and a quit
/// the user cannot complete is a worse bug than a daemon that outlives it. The
/// next launch reconciles either way.
///
/// The budget covers acquiring [`LIFECYCLE`] as well as the stop itself - an
/// install in progress can hold it for longer than the whole quit is allowed to
/// take.
pub(crate) async fn stop_for_quit() {
    let span = tracing::info_span!(
        "daemon_lifecycle.stop",
        reason = "quit",
        outcome = tracing::field::Empty,
        otel.status_code = tracing::field::Empty,
    );
    async {
        let work = async {
            let _guard = LIFECYCLE.lock().await;
            stop_daemon().await
        };
        let s = tracing::Span::current();
        match tokio::time::timeout(QUIT_STOP_BUDGET, work).await {
            Ok(Ok(())) => {
                s.record("outcome", "stopped");
                tracing::info!("daemon stopped for quit");
            }
            Ok(Err(e)) => {
                s.record("outcome", "failed");
                s.record("otel.status_code", "ERROR");
                tracing::warn!(error = %e, "could not stop the daemon on quit - exiting anyway");
            }
            Err(_) => {
                s.record("outcome", "timeout");
                s.record("otel.status_code", "ERROR");
                tracing::warn!(
                    budget_s = QUIT_STOP_BUDGET.as_secs(),
                    "stopping the daemon on quit exceeded its budget - exiting anyway"
                );
            }
        }
    }
    .instrument(span)
    .await
}

/// Take the daemon down because the user paused it from the tray menu.
///
/// Unbounded, unlike [`stop_for_quit`]: nothing is waiting on this to finish,
/// and the caller reports the outcome to the user rather than swallowing it.
///
/// The paused flag is set **inside** the guard and before the OS call, so no
/// concurrent restore can observe "not paused" and start the daemon back up. A
/// failed stop rolls it back rather than leaving the watchdog blind to a daemon
/// that is, in fact, still running.
pub(crate) async fn stop_for_pause() -> Result<(), String> {
    let span = tracing::info_span!(
        "daemon_lifecycle.stop",
        reason = "pause",
        outcome = tracing::field::Empty,
        otel.status_code = tracing::field::Empty,
    );
    async {
        let s = tracing::Span::current();
        let _guard = LIFECYCLE.lock().await;
        DAEMON_PAUSED.store(true, Ordering::Relaxed);
        match stop_daemon().await {
            Ok(()) => {
                s.record("outcome", "stopped");
                tracing::info!("daemon stopped for pause");
                Ok(())
            }
            Err(e) => {
                DAEMON_PAUSED.store(false, Ordering::Relaxed);
                s.record("outcome", "failed");
                s.record("otel.status_code", "ERROR");
                tracing::warn!(error = %e, "could not pause the daemon");
                Err(e)
            }
        }
    }
    .instrument(span)
    .await
}

/// Bring a paused daemon back.
///
/// Delegates to [`crate::backend_install::ensure_daemon_running`], the same call
/// the tray makes on launch, so resuming and relaunching converge on one code
/// path rather than two that can drift. It returns early when the daemon is
/// already up, so a double-click cannot `kickstart -k` a healthy daemon
/// mid-write.
pub(crate) async fn resume_from_pause() -> Result<(), String> {
    // Both fields must be declared here, even though only one is ever set on a
    // given run: `Span::record` on a field the macro did not declare is a
    // silent no-op, so an `otel.status_code` recorded later would never reach
    // the exporter and the failure would look like a success that stopped
    // logging.
    let span = tracing::info_span!(
        "daemon_lifecycle.resume",
        outcome = tracing::field::Empty,
        otel.status_code = tracing::field::Empty,
    );
    async {
        let s = tracing::Span::current();
        // Handled long-hand rather than with `?`: this is the failure boundary,
        // and a bare `?` would return an error the span never records - leaving
        // a resume that silently did nothing indistinguishable, in telemetry,
        // from one that never ran.
        let Some(home) = meridian_core::paths::home_dir() else {
            let e = "home directory could not be resolved".to_string();
            s.record("outcome", "failed");
            s.record("otel.status_code", "ERROR");
            tracing::warn!(error = %e, "could not resume the daemon");
            return Err(e);
        };
        let _guard = LIFECYCLE.lock().await;
        DAEMON_PAUSED.store(false, Ordering::Relaxed);
        crate::backend_install::ensure_daemon_running(&home).await;

        // `ensure_daemon_running` returns `()` and swallows its own failures
        // with `tracing::warn!` - deliberately, because its other callers (the
        // installer bail-outs, the launch restore) cannot act on one. This
        // caller can: recording "resumed" on its say-so would report a success
        // nothing verified, and `toggle_daemon` clears the Paused notice
        // straight after, so the menu would read Connected over a daemon that
        // is down.
        //
        // The runtime state self-heals - the pause flag is clear, so the
        // watchdog starts it within STRIKES ticks - but telemetry does not. A
        // failed resume must not be indistinguishable from a working one in
        // the exporter.
        match verify_daemon_came_back().await {
            Some(true) => {
                s.record("outcome", "resumed");
                tracing::info!("daemon resumed from pause");
            }
            Some(false) => {
                // Not an `Err`: the resume itself did what it could, the pause
                // flag is genuinely clear, and failing the command would tell
                // the user to retry something the watchdog is already fixing.
                // The span carries the truth for whoever reads it later.
                s.record("outcome", "resume_unconfirmed");
                s.record("otel.status_code", "ERROR");
                tracing::warn!(
                    "resumed the daemon but it is not running - leaving it to the watchdog"
                );
            }
            None => {
                // Windows has no liveness check wired (`process_alive` is
                // `None` there by design), so this is "cannot tell", not a
                // failure - and must not be reported as one.
                s.record("outcome", "resumed_unverified");
                tracing::info!(
                    "daemon resume requested - liveness not verifiable on this platform"
                );
            }
        }
        Ok(())
    }
    .instrument(span)
    .await
}

/// Whether a local dev daemon owns this data dir, set by `dev-start.sh` in the
/// tabs it spawns.
///
/// An explicit opt-in env var rather than sniffing for a `target/debug/meridian`
/// process: the tray and the dev daemon race at startup (both tabs open at
/// once), so a process probe would report "no dev daemon" purely because it ran
/// a second too early — the same timing bug this exists to close. It is also
/// inert in packaged builds, which never set it, so the shipped restore
/// behaviour cannot change.
fn dev_daemon_owns_data_dir() -> bool {
    matches!(
        std::env::var("MERIDIAN_DEV_DAEMON").as_deref(),
        Ok("1") | Ok("true")
    )
}

/// The launch-time restore, refusing while the daemon is deliberately paused
/// **or** while a dev daemon owns the data dir.
///
/// Every bail-out in [`crate::backend_install::ensure_backend_installed`] calls
/// this rather than `ensure_daemon_running` directly. Both halves matter:
///
/// - **The pause check.** The installer runs off the setup hook and can still be
///   working seconds into the session, by which time the user can have paused
///   from the tray menu. Restoring then would leave a running daemon under a
///   Paused label, and the watchdog would not fix it because the pause flag is
///   what tells it to stand down.
/// - **The guard.** Checking the flag without holding [`LIFECYCLE`] would only
///   narrow that race, not close it - the pause could land between the check and
///   the `launchctl` call.
pub(crate) async fn restore_unless_paused(home: &std::path::Path) {
    let span = tracing::info_span!("daemon_lifecycle.restore", outcome = tracing::field::Empty);
    async {
        let s = tracing::Span::current();
        let _guard = LIFECYCLE.lock().await;
        if DAEMON_PAUSED.load(Ordering::Relaxed) {
            s.record("outcome", "skipped_paused");
            tracing::info!("skipping the launch-time daemon restore - the user paused it");
            return;
        }
        // A DEV DAEMON OWNS THIS DATA DIR — do not resurrect the installed one.
        //
        // `dev-start.sh` opens two Terminal tabs at once: one claims
        // ~/.meridian/daemon.sock for a `cargo run` daemon, the other starts
        // `tauri dev`. This tray then reached the dev/source bail-out in
        // `ensure_backend_installed`, which calls straight through to here, and
        // re-registered + kickstarted the INSTALLED launchd daemon seconds
        // later. That daemon took the socket back, and the dev daemon exited on
        // the single-instance guard with status 0 — so `cargo watch` printed
        // "Exit status: 0" and looked healthy while running nothing.
        //
        // Checked HERE rather than in `ensure_daemon_running`, which is the
        // shared choke point but is also what the tray menu's Resume calls: a
        // user who explicitly resumes must still get a daemon. This is only the
        // automatic launch-time restore, which is the one that must stand down.
        if dev_daemon_owns_data_dir() {
            s.record("outcome", "skipped_dev_daemon");
            tracing::info!(
                "skipping the launch-time daemon restore - MERIDIAN_DEV_DAEMON is set, \
                 so a dev daemon owns this data dir"
            );
            return;
        }
        s.record("outcome", "restored");
        crate::backend_install::ensure_daemon_running(home).await;
    }
    .instrument(span)
    .await
}

/// The OS mechanics, shared by quit and pause.
///
/// macOS: `bootout` (never `stop` - see the module docs) via the same
/// clearance-waiting helper `register_agent` and the encryption migration use.
///
/// Windows: the Task Scheduler path already written for staging an update,
/// which ends the task and then waits for the process to actually go. Note that
/// it escalates to `taskkill /F` for stragglers, which is a `TerminateProcess`
/// with no clean pool shutdown. That is reused rather than softened on purpose:
/// it is the only stop mechanism this repo has on Windows, it already runs on
/// every update, and there is no graceful signal wired to the daemon there (the
/// `SIGTERM` handler is a Unix path). SQLite survives a killed writer; the
/// corruption profile that hurt this codebase was *two* writers sharing a WAL,
/// which is the thing being removed here.
async fn stop_daemon() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        crate::backend_install::bootout_agent_and_wait(crate::backend_install::DAEMON_LABEL).await
    }
    #[cfg(target_os = "windows")]
    {
        let home = meridian_core::paths::home_dir()
            .ok_or_else(|| "home directory could not be resolved".to_string())?;
        let daemon_bin = home
            .join(".meridian")
            .join("bin")
            .join(crate::backend_install::DAEMON_FILE);
        crate::backend_install::stop_running_daemon_before_stage(&daemon_bin).await
    }
    // No service manager is wired on other targets, so there is nothing to stop
    // and nothing to report - the tray does not manage a daemon there at all.
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The staging flag must be self-clearing on EVERY exit from the install,
    /// not just the happy one.
    ///
    /// `install()` bails with `?` on a failed stop, a failed copy, and a failed
    /// registration, and it runs on a spawned task where a panic unwinds rather
    /// than aborting. A flag left set by any of those paths disables the
    /// watchdog for the rest of the session — and on Windows the watchdog is
    /// the only supervisor there is, so the daemon would stay down until the
    /// next launch with nothing reporting why. That is a strictly worse
    /// outcome than the install race this flag was added to fix, which is why
    /// it is an RAII guard rather than a set/clear pair.
    #[test]
    fn the_staging_flag_clears_on_every_exit_path() {
        assert!(!is_staging(), "must start clear");

        // Normal scope exit.
        {
            let _g = begin_staging();
            assert!(is_staging(), "the guard sets it for its lifetime");
        }
        assert!(!is_staging(), "dropping the guard clears it");

        // Early return — the `?` bail-outs in `install()`.
        fn bails_out() -> Result<(), ()> {
            let _g = begin_staging();
            assert!(is_staging());
            Err(())
        }
        assert!(bails_out().is_err());
        assert!(!is_staging(), "an early return must still clear it");

        // Panic — the install runs on a spawned task, so this unwinds. The hook
        // is muted around it so a deliberate panic doesn't print a scary
        // backtrace into an otherwise-passing test run.
        let hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let panicked = std::panic::catch_unwind(|| {
            let _g = begin_staging();
            panic!("install blew up");
        });
        std::panic::set_hook(hook);
        assert!(panicked.is_err());
        assert!(!is_staging(), "unwinding must still clear it");

        // Pause and staging both make the watchdog stand down, but they must
        // stay SEPARATE flags: staging is internal and lasts seconds, pause is
        // the user's instruction and persists. Reusing the pause flag for
        // staging would show "Paused" in the tray during every update, and
        // would leave the daemon genuinely paused if an install died mid-swap.
        //
        // Asserted in THIS test rather than its own: both touch the same
        // process-global, and `cargo test` runs test fns on parallel threads,
        // so two functions racing over `DAEMON_STAGING` would flake. One test
        // owns the flag.
        let paused_before = is_paused();
        {
            let _g = begin_staging();
            assert!(is_staging());
            assert_eq!(
                is_paused(),
                paused_before,
                "staging must not move the user-visible pause flag"
            );
        }
        assert!(!is_staging());
    }

    /// The bug this module exists for: quitting from the tray menu, or via
    /// Cmd+Q once the dashboard has switched the activation policy to
    /// `Regular`, must take the daemon down.
    #[test]
    fn a_user_quit_holds_the_exit_and_stops_the_daemon() {
        assert_eq!(
            decide_exit(None, ExitPhase::Running),
            ExitAction::HoldAndStop,
            "a user-driven quit must stop the daemon - leaving it up is what \
             made 'meridian db repair' unreachable"
        );
    }

    /// `tray::handle_menu_event`'s "quit" arm and the popover's Quit button both
    /// call `app.exit(0)`, which arrives here as `Some(0)` rather than `None`.
    /// Keying on "is it a restart" instead of "is it programmatic" is what keeps
    /// those paths covered.
    #[test]
    fn a_programmatic_exit_stops_the_daemon_too() {
        assert_eq!(
            decide_exit(Some(0), ExitPhase::Running),
            ExitAction::HoldAndStop
        );
        assert_eq!(
            decide_exit(Some(1), ExitPhase::Running),
            ExitAction::HoldAndStop
        );
    }

    /// A relaunch must leave the daemon alone AND must not be held.
    ///
    /// Holding it would be the worse bug of the two: the pending stop ends with
    /// `exit(0)`, so a held restart would silently become a plain quit - the
    /// user's app would simply not come back.
    #[test]
    fn a_restart_is_never_held_and_never_stops_the_daemon() {
        for phase in [
            ExitPhase::Running,
            ExitPhase::Stopping,
            ExitPhase::ReadyToExit,
        ] {
            assert_eq!(
                decide_exit(Some(tauri::RESTART_EXIT_CODE), phase),
                ExitAction::Proceed,
                "an auto-update or repair relaunch must pass straight through \
                 (phase {phase:?})"
            );
        }
    }

    /// **The regression test for the double-quit window.**
    ///
    /// A quit takes up to [`QUIT_STOP_BUDGET`], during which the app looks
    /// hung - which is exactly when a user hits Quit or Cmd+Q a second time.
    /// The previous single-latch version answered "already stopping, do
    /// nothing", so the handler skipped `prevent_exit` and Tauri tore the
    /// process down with the stop still in flight: the daemon survived, which
    /// is the bug this whole module exists to fix.
    #[test]
    fn a_second_quit_mid_stop_is_still_held() {
        assert_eq!(
            decide_exit(None, ExitPhase::Stopping),
            ExitAction::Hold,
            "a second quit during the stop must still be prevented, or the \
             process exits and drops the shutdown task"
        );
        assert_eq!(decide_exit(Some(0), ExitPhase::Stopping), ExitAction::Hold);
    }

    /// ...and exactly one phase lets an exit through, or the app could never
    /// close at all. The stop task sets `ReadyToExit` immediately before its own
    /// `exit(0)`.
    #[test]
    fn the_internal_exit_is_the_one_that_proceeds() {
        assert_eq!(
            decide_exit(None, ExitPhase::ReadyToExit),
            ExitAction::Proceed
        );
        assert_eq!(
            decide_exit(Some(0), ExitPhase::ReadyToExit),
            ExitAction::Proceed
        );
    }

    /// `EXIT_PHASE` is one process-global, and `cargo test` runs these in
    /// parallel threads of one process - so the tests that write it must not
    /// interleave. Without this they pass alone and flake together.
    static PHASE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// The phase round-trips through its atomic encoding. Cheap, but the
    /// encoding is hand-rolled and a wrong default would silently mean
    /// "Running" forever - i.e. every exit held and then re-stopped.
    #[test]
    fn the_exit_phase_round_trips() {
        let _serial = PHASE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        for phase in [
            ExitPhase::Stopping,
            ExitPhase::ReadyToExit,
            ExitPhase::Running,
        ] {
            set_exit_phase(phase);
            assert_eq!(exit_phase(), phase);
        }
        set_exit_phase(ExitPhase::Running);
    }

    /// A panic in the stop task must not leave the app unquittable.
    ///
    /// `Stopping` holds every subsequent `ExitRequested`, and only the stop
    /// task advances past it - so if that task dies mid-stop, nothing else
    /// ever will, and the tray can no longer be quit at all. The guard runs on
    /// the unwind path and releases the hold.
    ///
    /// Asserted on the guard's drop rather than by panicking a real stop task:
    /// no unit test can drive a Tauri event loop, and the phase transition IS
    /// the behaviour that matters.
    #[test]
    fn a_dead_stop_task_releases_the_held_exit() {
        let _serial = PHASE_LOCK.lock().unwrap_or_else(|e| e.into_inner());

        set_exit_phase(ExitPhase::Stopping);
        drop(HeldExitGuard);
        assert_eq!(
            exit_phase(),
            ExitPhase::ReadyToExit,
            "a stop task that dies while holding the exit must release it, or \
             every later quit is held forever and the tray cannot be closed"
        );
        assert_eq!(
            decide_exit(None, exit_phase()),
            ExitAction::Proceed,
            "and the next quit must actually get through"
        );

        // Conditional, not unconditional: the normal path has already stored
        // `ReadyToExit` before this drops, and a phase reset to `Running` must
        // not be promoted to an exit nobody asked for.
        set_exit_phase(ExitPhase::Running);
        drop(HeldExitGuard);
        assert_eq!(
            exit_phase(),
            ExitPhase::Running,
            "the guard must only ever advance the stuck Stopping phase"
        );

        set_exit_phase(ExitPhase::Running);
    }

    /// The policy above is worth nothing unless something consults it, and no
    /// unit test can reach `RunEvent::ExitRequested` - it needs a live Tauri
    /// event loop. So scan the source, the way `backend_install`'s `cfg` audit
    /// and the UI's `no-native-dialogs` test do for their own unreachable-in-a-
    /// unit-test invariants.
    ///
    /// This is the regression guard for the original bug: before this change
    /// `run`'s event closure handled only `RunEvent::Reopen`, so quitting the
    /// tray left the daemon running.
    #[test]
    fn the_exit_handler_is_actually_wired_into_run() {
        const LIB_SRC: &str = include_str!("lib.rs");
        assert!(
            LIB_SRC.contains("RunEvent::ExitRequested"),
            "run()'s event closure must handle ExitRequested, or quitting the \
             tray silently leaves the daemon running - the bug this module fixes"
        );
        assert!(
            LIB_SRC.contains("daemon_lifecycle::stop_for_quit"),
            "the ExitRequested handler must call stop_for_quit"
        );
        assert!(
            LIB_SRC.contains("ExitPhase::ReadyToExit"),
            "the handler must mark ReadyToExit before its internal exit, or the \
             app can never actually close"
        );
        assert!(
            LIB_SRC.contains("prevent_exit"),
            "the stop is async, so the handler must defer the exit with \
             prevent_exit rather than racing the process teardown"
        );

        // [`ExitAction::Hold`] is worthless if the handler treats it as "do
        // nothing" - that IS the single-latch bug, restated. Pin the arm
        // itself, not just that `prevent_exit` appears somewhere in the file:
        // the `HoldAndStop` arm also contains it, so a file-wide check stays
        // green while a second quit tears the process down mid-stop.
        let hold_arm = LIB_SRC
            .split_once("ExitAction::Hold =>")
            .expect("the handler must have an ExitAction::Hold arm")
            .1;
        // Bounded at the NEXT arm, not by a character count. `HoldAndStop`
        // follows immediately and calls `prevent_exit` itself, so any fixed
        // window wide enough to be useful also reads into it and passes on
        // borrowed evidence.
        let hold_arm = hold_arm
            .split_once("ExitAction::")
            .map(|(arm, _)| arm)
            .unwrap_or(hold_arm);
        assert!(
            hold_arm.contains("prevent_exit"),
            "the ExitAction::Hold arm must call prevent_exit; found: {hold_arm:?}"
        );

        // And the stop task must take the guard that releases a held exit if
        // it dies. `a_dead_stop_task_releases_the_held_exit` drops the guard
        // itself, so it proves the guard WORKS but not that anything HOLDS one
        // - deleting the binding in `lib.rs` leaves every test green and the
        // tray permanently unquittable after a panic. This is the only place
        // that can catch it, for the same reason the assertions above live
        // here: no unit test can reach `RunEvent::ExitRequested`.
        //
        // Bounded to the HoldAndStop arm rather than the whole file, on the
        // same reasoning as the Hold arm above: a file-wide check would pass
        // on the `use` line or a doc reference and never notice the binding
        // itself had gone.
        let stop_arm = LIB_SRC
            .split_once("ExitAction::HoldAndStop =>")
            .expect("the handler must have an ExitAction::HoldAndStop arm")
            .1;
        let stop_arm = stop_arm
            .split_once("\n            }")
            .map(|(arm, _)| arm)
            .unwrap_or(stop_arm);
        // Narrowed twice more, both times because a looser check was verified
        // to stay green against a real regression:
        //
        // 1. Match the BINDING, not the bare type name - the arm carries a
        //    comment naming the guard, so `contains("HeldExitGuard")` passes
        //    on the prose after the binding is deleted.
        // 2. Scan the SPAWNED TASK's body, not the whole arm. A guard bound in
        //    the arm but outside the task is not a weaker version of this, it
        //    is a broken one: it would drop when the synchronous handler
        //    returns - immediately, while the stop is still in flight -
        //    releasing the hold it exists to maintain and letting the process
        //    tear down mid-stop. That is the original bug, restored.
        let spawn_body = stop_arm
            .split_once("spawn(async move {")
            .expect("the HoldAndStop arm must spawn the stop task")
            .1;
        assert!(
            spawn_body.contains("= daemon_lifecycle::HeldExitGuard;"),
            "the spawned stop task must BIND a daemon_lifecycle::HeldExitGuard \
             INSIDE the task, or a panic mid-stop leaves the exit held forever \
             and the app cannot be quit at all; found: {spawn_body:?}"
        );

        // `stop_for_quit`'s verdict must be FLUSHED before the process exits.
        //
        // `handle.exit` is `std::process::exit`: no destructors, and whatever
        // the OTel batch processors are still holding dies with the process.
        // What they are holding at that moment is the line describing how
        // stopping the daemon went - `daemon stopped for quit`, `could not stop
        // the daemon on quit`, or `exceeded its budget`. Those are the most
        // useful records that exist for a corruption report, because quit is
        // when the tray and the daemon are most likely to overlap on
        // meridian.db, and every one of them was being discarded microseconds
        // after being emitted.
        //
        // Ordering is asserted, not mere presence: a flush placed BEFORE
        // `stop_for_quit` compiles, runs, logs nothing unusual, and preserves
        // exactly the records that were never in danger while still losing the
        // one that was.
        let stop_pos = spawn_body
            .find("stop_for_quit().await")
            .expect("the spawned task must call stop_for_quit");
        let flush_pos = spawn_body
            .find("observability::force_flush().await")
            .expect(
                "the spawned stop task must flush telemetry before exiting, or \
                 stop_for_quit's outcome never reaches the spool",
            );
        let exit_pos = spawn_body
            .find("handle.exit(")
            .expect("the spawned task must exit the app");
        assert!(
            stop_pos < flush_pos && flush_pos < exit_pos,
            "the telemetry flush must sit BETWEEN stop_for_quit and \
             handle.exit: before the stop it flushes a verdict that has not \
             been reached yet, and after the exit it does not run at all. \
             Found stop at {stop_pos}, flush at {flush_pos}, exit at {exit_pos}."
        );
    }
}
