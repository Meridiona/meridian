//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! `meridian restart` - stop and restart the background daemon.
//!
//! # Why this had to exist
//!
//! Five places in this binary already told users to run `meridian restart`,
//! and on a packaged install the command did not exist:
//!
//! - `health::daemon`'s `etl freshness` remedy (the one a production user hit
//!   on 2026-08-27, staring at a stalled ETL while `doctor` recommended a
//!   command that answered `unknown subcommand "restart"`)
//! - `main.rs`'s Jira and Trello OAuth handlers, printed on the **success**
//!   path: "Tokens saved ... run `meridian restart` to pick them up"
//! - `health::diagnose`'s expired-token remedy
//! - `health::capture`'s persistent-failure remedy
//!
//! It worked for whoever wrote it, which is exactly why it survived: the
//! **source** install symlinks `~/.local/bin/meridian` at
//! `scripts/meridian-cli.sh`, whose bash `cmd_restart` has always existed. A
//! **DMG** install has no wrapper - `~/.meridian/bin/meridian` is this native
//! binary - so the same advice was a dead end for every packaged user.
//!
//! The OAuth case is the damaging one and had nothing to do with `doctor`: a
//! user connects Jira, is told to run a command that does not exist, doesn't
//! run it, and their tracker integration silently never picks up the tokens
//! until something else happens to restart the daemon.
//!
//! # Why `kickstart -k` and not `bootout` + `bootstrap`
//!
//! `-k` is precisely what kills a running instance, and here that is what was
//! asked for - unlike the tray's daemon watchdog, where `-k` on a daemon that
//! was merely slow to answer an 800 ms probe corrupted `meridian.db` on every
//! macOS install for days (see `tray/src-tauri/src/poll/watchdog.rs`, which
//! now drops `-k` for exactly that reason).
//! Two things make it correct in this module and not there: the user typed the
//! command, and this runs in a **separate short-lived process**, so it is not
//! the daemon signalling itself.
//!
//! `bootout` + `bootstrap` is deliberately NOT used. It is heavier, and the
//! `launchctl disable` that tends to travel with it has already wedged a plist
//! in this repo badly enough that `bootstrap` returned EIO and the agent had to
//! be reinstalled by hand - see `crate::db::repair::marker`'s module docs.
//! Recovery tooling must not be able to brick the thing it is recovering.
//!
//! # Who calls this
//! `main.rs`'s subcommand dispatch, and every remedy string listed above.

/// The daemon's launchd label on macOS. Must match the plist that
/// `tray/src-tauri/src/backend_install.rs` renders.
#[cfg(target_os = "macos")]
const DAEMON_LABEL: &str = "com.meridiona.daemon";

/// The daemon's Task Scheduler name on Windows. Must match
/// `backend_install::WINDOWS_TASK_NAME`.
#[cfg(target_os = "windows")]
const WINDOWS_TASK_NAME: &str = "Meridian Daemon";

/// What `meridian restart` did.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The service manager was asked to restart the daemon.
    Restarted,
    /// No service is registered for this user - a source/dev checkout, where
    /// the daemon is run by hand or by `cargo watch`. Reported rather than
    /// silently "succeeding", because a bare exit 0 here would look like the
    /// restart happened.
    NotRegistered,
    /// The service manager refused.
    Failed(String),
}

/// Human-readable result, ready to print. Separated from [`run`] so the
/// wording is testable without a service manager.
pub fn describe(outcome: &Outcome) -> String {
    match outcome {
        Outcome::Restarted => "Meridian daemon restarted.".to_string(),
        Outcome::NotRegistered => {
            // Plain hyphens: user-facing text (see the repo's hard rule).
            "No Meridian daemon service is registered for your user.\n\
             This is expected on a source checkout - start it the way you \
             normally do (for example 'cargo run --bin meridian'), or install \
             the app to get a managed daemon."
                .to_string()
        }
        Outcome::Failed(why) => format!("Could not restart the Meridian daemon: {why}"),
    }
}

/// Restart the managed daemon for the current user.
pub fn run() -> Outcome {
    #[cfg(target_os = "macos")]
    {
        let target = format!("gui/{}/{}", uid_str(), DAEMON_LABEL);
        // Ask first, so a source checkout gets an explanation instead of
        // launchctl's "Could not find service" spelled as a raw error.
        if !service_is_loaded(&target) {
            return Outcome::NotRegistered;
        }
        match std::process::Command::new("launchctl")
            .args(["kickstart", "-k", &target])
            .output()
        {
            Ok(o) if o.status.success() => Outcome::Restarted,
            Ok(o) => Outcome::Failed(format!(
                "launchctl kickstart exited {}: {}",
                o.status,
                String::from_utf8_lossy(&o.stderr).trim()
            )),
            Err(e) => Outcome::Failed(format!("could not run launchctl: {e}")),
        }
    }
    #[cfg(target_os = "windows")]
    {
        // No atomic restart verb in Task Scheduler: end it, then run it. `/End`
        // failing is not fatal - the task may simply not have been running -
        // so only `/Run` decides the outcome.
        let _ = std::process::Command::new("schtasks")
            .args(["/End", "/TN", WINDOWS_TASK_NAME])
            .output();
        match std::process::Command::new("schtasks")
            .args(["/Run", "/TN", WINDOWS_TASK_NAME])
            .output()
        {
            Ok(o) if o.status.success() => Outcome::Restarted,
            // schtasks reports a missing task on stderr with a non-zero exit;
            // that is the Windows shape of "no service registered".
            Ok(o) => {
                let err = String::from_utf8_lossy(&o.stderr).trim().to_string();
                if err.contains("cannot find the file specified") || err.contains("does not exist")
                {
                    Outcome::NotRegistered
                } else {
                    Outcome::Failed(format!("schtasks /Run exited {}: {err}", o.status))
                }
            }
            Err(e) => Outcome::Failed(format!("could not run schtasks: {e}")),
        }
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        Outcome::NotRegistered
    }
}

/// Whether launchd has the label loaded for this user.
///
/// `launchctl print` exits non-zero for a label that is not loaded at all,
/// which is the source/dev case; it exits 0 both for "loaded and running" and
/// "loaded but stopped", and `kickstart` is correct for either.
#[cfg(target_os = "macos")]
fn service_is_loaded(target: &str) -> bool {
    std::process::Command::new("launchctl")
        .args(["print", target])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// The current user's uid as launchd wants it in a `gui/<uid>/<label>` target.
///
/// Falls back to `501` (the first console user on macOS) rather than failing:
/// a wrong uid produces a clean "not registered" message, whereas refusing to
/// run produces nothing useful at all.
#[cfg(target_os = "macos")]
fn uid_str() -> String {
    std::process::Command::new("id")
        .arg("-u")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "501".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every message this prints is user-facing app text, so it takes plain
    /// hyphens only - no em-dash or en-dash (repo hard rule).
    #[test]
    fn messages_use_plain_hyphens() {
        for o in [
            Outcome::Restarted,
            Outcome::NotRegistered,
            Outcome::Failed("boom".into()),
        ] {
            let s = describe(&o);
            for bad in ['\u{2014}', '\u{2013}'] {
                assert!(
                    !s.contains(bad),
                    "{s:?} contains {bad:?} - user-facing text takes a plain hyphen"
                );
            }
        }
    }

    /// A source checkout must be told what happened rather than getting a
    /// silent exit 0 that looks like the restart worked.
    #[test]
    fn an_unregistered_service_is_explained_not_faked() {
        let s = describe(&Outcome::NotRegistered);
        assert!(
            s.contains("source checkout"),
            "the dev case must say why there is no service: {s:?}"
        );
        assert!(
            !s.contains("restarted"),
            "must not imply a restart happened: {s:?}"
        );
    }

    /// A failure has to carry the reason - the whole point is that the
    /// previous behaviour ("unknown subcommand") told the operator nothing.
    #[test]
    fn a_failure_names_its_cause() {
        assert!(
            describe(&Outcome::Failed("launchctl exited 1".into())).contains("launchctl exited 1")
        );
    }

    /// THE regression this module exists for. Every in-binary string that
    /// tells a user to run `meridian restart` is only correct while the
    /// subcommand is actually dispatched. Source-scanned because the dispatch
    /// chain is `if` statements, not data.
    #[test]
    fn the_restart_subcommand_is_actually_dispatched() {
        let main_rs = include_str!("main.rs");
        assert!(
            main_rs.contains("nth(1).as_deref() == Some(\"restart\")"),
            "main.rs no longer dispatches `restart`, but health remedies and the \
             OAuth success paths still tell users to run it - which is exactly the \
             dead end reported from production on 2026-08-27"
        );
    }

    /// The remedy strings and the subcommand must not drift apart again: if
    /// something still recommends `meridian restart`, the command must exist
    /// (asserted above), and if nothing recommends it any more this test
    /// should be revisited rather than silently passing.
    #[test]
    fn something_still_recommends_the_command_this_module_implements() {
        let health = include_str!("health/daemon.rs");
        assert!(
            health.contains("meridian restart"),
            "health::daemon no longer recommends `meridian restart`; if that was \
             deliberate, re-check whether this subcommand is still needed"
        );
    }
}
