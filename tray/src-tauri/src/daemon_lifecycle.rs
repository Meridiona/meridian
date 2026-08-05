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
//! Pause state is process-local ([`crate::state::AppState::daemon_paused`]) and
//! deliberately not persisted, matching the capture pause in
//! [`crate::commands::pause`]: relaunching the tray resumes the daemon. The
//! alternative - a pause that outlives the process that set it - is the failure
//! mode `repair::marker` had to grow an expiry to escape.
//!
//! # Who calls this
//!
//! - [`stop_for_quit`] - [`crate::run`]'s `RunEvent::ExitRequested` handler.
//! - [`stop_for_pause`] / [`resume_from_pause`] - [`crate::commands::daemon::toggle_daemon`],
//!   behind the tray menu's Connected / Disconnected item.
//!
//! # Related
//!
//! - [`crate::backend_install`] - owns the OS mechanics and the restore half.
//! - [`crate::poll::watchdog`] - must not fight a pause; its `decide` takes
//!   `daemon_paused` for that reason.
//! - [`crate::commands::daemon_control`] - the probe and the launchd verbs.

use std::time::Duration;

/// How long a quit waits for the daemon to go down before giving up and exiting
/// anyway.
///
/// The bootout itself is near-instant; this covers launchd taking its time to
/// clear the entry. Bounded because a quit that hangs is worse than a daemon
/// that outlives it by a few seconds: the user asked for the app to close, and
/// the next launch reconciles the daemon regardless.
const QUIT_STOP_BUDGET: Duration = Duration::from_secs(5);

/// Whether an exit should take the daemon down with it.
///
/// Pure and free of `cfg` so the policy is pinned by tests on every platform -
/// the same discipline as [`crate::poll::watchdog::decide`] and
/// [`crate::commands::daemon_control`]'s `status_running`, and for the same
/// reason: everything around it shells out to the OS and cannot be tested at
/// all.
///
/// - `code` is `RunEvent::ExitRequested`'s: `None` for a user-driven quit
///   (the tray menu item, macOS Cmd+Q), `Some` when something called
///   `AppHandle::exit` or `AppHandle::restart`.
/// - `already_stopping` is the re-entry latch. The handler exits by calling
///   `exit()` again, which fires a second `ExitRequested`; without the latch
///   that would spawn another stop and never terminate.
///
/// [`tauri::RESTART_EXIT_CODE`] is the one code that must NOT stop the daemon.
/// Both `update.rs`'s post-install relaunch and `commands::repair`'s
/// restart-into-`repair_boot` call `AppHandle::restart`, which routes through
/// this event from a command thread. The app is coming straight back in both
/// cases, so a bootout would be pure latency on the update path - and on the
/// repair path it would fight the marker handshake that is already holding the
/// daemon off.
pub(crate) fn should_stop_daemon(code: Option<i32>, already_stopping: bool) -> bool {
    if already_stopping {
        return false;
    }
    code != Some(tauri::RESTART_EXIT_CODE)
}

/// Take the daemon down because the app is quitting.
///
/// Bounded by [`QUIT_STOP_BUDGET`] and **infallible by design**: every failure
/// path logs and returns, because the caller's next act is to exit and a quit
/// the user cannot complete is a worse bug than a daemon that outlives it. The
/// next launch reconciles either way.
pub(crate) async fn stop_for_quit() {
    match tokio::time::timeout(QUIT_STOP_BUDGET, stop_daemon()).await {
        Ok(Ok(())) => tracing::info!("daemon stopped for quit"),
        Ok(Err(e)) => {
            tracing::warn!(error = %e, "could not stop the daemon on quit - exiting anyway")
        }
        Err(_) => tracing::warn!(
            budget_s = QUIT_STOP_BUDGET.as_secs(),
            "stopping the daemon on quit exceeded its budget - exiting anyway"
        ),
    }
}

/// Take the daemon down because the user paused it from the tray menu.
///
/// Unbounded, unlike [`stop_for_quit`]: nothing is waiting on this to finish,
/// and the caller reports the outcome to the user rather than swallowing it.
pub(crate) async fn stop_for_pause() -> Result<(), String> {
    stop_daemon().await.inspect(|()| {
        tracing::info!("daemon stopped for pause");
    })
}

/// Bring a paused daemon back.
///
/// Delegates to [`crate::backend_install::ensure_daemon_running`], which is the
/// same call the tray makes on launch - so resuming and relaunching converge on
/// one code path rather than two that can drift. It returns early when the
/// daemon is already up, so a double-click cannot `kickstart -k` a healthy
/// daemon mid-write.
pub(crate) async fn resume_from_pause() -> Result<(), String> {
    let home = meridian_core::paths::home_dir()
        .ok_or_else(|| "home directory could not be resolved".to_string())?;
    crate::backend_install::ensure_daemon_running(&home).await;
    tracing::info!("daemon resumed from pause");
    Ok(())
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
    fn a_user_quit_stops_the_daemon() {
        assert!(
            should_stop_daemon(None, false),
            "a user-driven quit must stop the daemon - leaving it up is what \
             made 'meridian db repair' unreachable"
        );
    }

    /// `tray::handle_menu_event`'s "quit" arm calls `app.exit(0)`, which arrives
    /// here as `Some(0)` rather than `None`. Keying on "is it a restart" instead
    /// of "is it programmatic" is what keeps that path covered.
    #[test]
    fn a_programmatic_exit_stops_the_daemon_too() {
        assert!(should_stop_daemon(Some(0), false));
        assert!(should_stop_daemon(Some(1), false));
    }

    /// A relaunch must leave the daemon alone. `update.rs` restarts into the
    /// newly installed version and `commands::repair` restarts into
    /// `repair_boot`; the app is returning immediately in both cases, and the
    /// repair path is relying on its marker rather than on launchd state.
    #[test]
    fn a_restart_does_not_stop_the_daemon() {
        assert!(
            !should_stop_daemon(Some(tauri::RESTART_EXIT_CODE), false),
            "an auto-update or repair relaunch must not bootout the daemon"
        );
    }

    /// The handler re-enters by calling `exit()` again. Without the latch that
    /// second pass would spawn another stop, which would exit again, forever.
    #[test]
    fn the_second_pass_is_a_no_op() {
        assert!(!should_stop_daemon(None, true));
        assert!(!should_stop_daemon(Some(0), true));
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
            LIB_SRC.contains("prevent_exit"),
            "the stop is async, so the handler must defer the exit with \
             prevent_exit rather than racing the process teardown"
        );
    }
}
