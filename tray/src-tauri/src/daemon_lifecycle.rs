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
        s.record("outcome", "resumed");
        tracing::info!("daemon resumed from pause");
        Ok(())
    }
    .instrument(span)
    .await
}

/// The launch-time restore, refusing while the daemon is deliberately paused.
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

    /// The phase round-trips through its atomic encoding. Cheap, but the
    /// encoding is hand-rolled and a wrong default would silently mean
    /// "Running" forever - i.e. every exit held and then re-stopped.
    #[test]
    fn the_exit_phase_round_trips() {
        for phase in [
            ExitPhase::Stopping,
            ExitPhase::ReadyToExit,
            ExitPhase::Running,
        ] {
            set_exit_phase(phase);
            assert_eq!(exit_phase(), phase);
        }
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
    }
}
