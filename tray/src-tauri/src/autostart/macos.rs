//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! macOS side of [`crate::autostart`] — a per-user launchd LaunchAgent that
//! covers the LOGIN trigger below macOS 13, where `SMAppService`
//! ([`super::login_item`]) does not exist. On macOS 13+ this module writes
//! nothing at all: SMAppService owns login by itself, and there is no
//! separate wake/morning relaunch any more (see [`plist_body`]'s docs for why
//! that was removed — it is what made Quit not stick).
//!
//! # Who calls this
//! [`crate::autostart::ensure_registered`] (write path) and
//! [`crate::autostart::status`] (read path, for analytics).
//!
//! # Related
//! - [`crate::backend_install`] — the daemon's LaunchAgent, whose
//!   [`crate::backend_install::launchctl`] wrapper this reuses for the
//!   plugin-era `bootout`.
//! - `scripts/com.meridiona.tray.plist` — the source-install template this
//!   mirrors, minus `KeepAlive`. Note it uses the SAME label, so a machine that
//!   ran `scripts/install-tray-daemon.sh` and then launches a packaged build
//!   will have its script-installed plist overwritten by this one. That is the
//!   right winner (the packaged app is the one the user actually clicks) and it
//!   cannot happen by accident on a user's machine: this module only runs from a
//!   bundled `.app`, while the script exists to launch an unbundled `target/`
//!   binary. It is worth knowing about on a developer machine, where the visible
//!   effect is that `KeepAlive` stops resurrecting the tray.

use super::{login_item, RegistrationAction, Status};
use std::path::{Path, PathBuf};

/// launchd label, matching the `com.meridiona.*` convention that
/// `src/uninstall.rs`'s `meridiona_agent_plists` glob already sweeps. Choosing
/// this over the plugin's `Meridian` is what makes uninstall correct without a
/// special case.
pub(crate) const LABEL: &str = "com.meridiona.tray";

/// Plist file name — launchd requires it to match [`LABEL`].
const PLIST_FILE: &str = "com.meridiona.tray.plist";

/// The plist `tauri-plugin-autostart` used to write, named after `productName`
/// (see `auto-launch-0.5.0/src/macos.rs`). Removed on first run of this module
/// so an upgraded install does not end up with two jobs racing to start the
/// tray at login.
const LEGACY_PLIST_FILE: &str = "Meridian.plist";

// NOTE: there is deliberately no LEGACY_LABEL constant. The label would only be
// needed to `launchctl bootout` the plugin-era job, and doing that from inside
// the running tray kills the tray — see `migrate_off_plugin`. `src/uninstall.rs`
// owns the one place a bootout of that label is safe, because there the app is
// meant to stop.

// NOTE: there is no MORNING_TRIGGER_MARKER here any more. `ensure_registered`
// compares the rendered plist body EXACTLY rather than probing for substrings,
// because a substring check cannot see a change to a field it does not name —
// and `RunAtLoad` is precisely such a field now that SMAppService owns the login
// half on macOS 13+. Windows still uses the substring form (`super::decide`),
// because Task Scheduler reformats the XML it hands back, so an exact compare
// there would report drift on every launch.

/// `~/Library/LaunchAgents`, where per-user agents live. `None` when the home
/// directory cannot be resolved, in which case there is nothing this module can
/// do.
fn launch_agents_dir() -> Option<PathBuf> {
    meridian_core::paths::home_dir().map(|h| h.join("Library/LaunchAgents"))
}

/// The running executable, canonicalised so the path recorded in the plist is
/// the same string a later launch will compare against — without this, a launch
/// through a symlinked `/Applications` would look like path drift on every
/// single startup and rewrite the plist forever.
fn current_exe() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    Some(exe.canonicalize().unwrap_or(exe))
}

/// The plist body.
///
/// Pure and separate from the write so every field below is unit-testable
/// without touching `~/Library/LaunchAgents` on a developer machine.
///
/// # Login only — there is no wake/morning relaunch any more
/// An earlier version of this plist also carried `LaunchEvents` on a set of
/// Darwin notifications (unlock, display-on, power-state, clamshell), so
/// Meridian came back on the OS's own wake events rather than a clock. That
/// was a deliberate choice (see the removed `WAKE_NOTIFICATIONS`, still in
/// git history) and it was wrong: those notifications fire many times in a
/// single day — every screen lock/unlock, every display sleep/wake, every lid
/// open/close — not once "in the morning". The practical effect was that Quit
/// did not stick: a user closed Meridian and it was back within minutes, the
/// next time their screen so much as locked and unlocked, which reads as "I
/// cannot quit this app".
///
/// The requirement is narrower than "always running": Meridian must come back
/// at **login, restart, or a manual start** — never resurrect itself after a
/// deliberate Quit before then. On macOS 13+ that is exactly what
/// `SMAppService` ([`super::login_item`]) already provides, so this plist is
/// only written at all below macOS 13, where SMAppService does not exist and
/// nothing else expresses "start Meridian at login". See
/// [`ensure_registered`] for how the two are coordinated, and
/// [`disarm_relaunch`] for clearing an already-armed wake job left by an
/// older build.
///
/// # The rest
/// - `run_at_load` is true whenever this plist is written (only reached below
///   macOS 13, where it is the sole login mechanism).
/// - **No `KeepAlive`**, deliberately: with it Quit would be undone within
///   seconds and there would be no way to stop Meridian at all. The daemon's
///   plist makes the opposite choice because it is headless and has no Quit.
/// - [`super::AUTOSTART_FLAG`] tells the tray this launch was unattended, so it
///   opens no window (`crate::poll::whats_new_auto_open`).
/// - `ProcessType` `Interactive` keeps launchd from throttling a job that owns UI.
/// - The stdout/stderr redirects are the OS-level crash safety net described in
///   `CLAUDE.md`'s observability section — the one thing the OTel spool cannot
///   capture. `src/telemetry_spool/launchd_log_cap.rs` size-caps these exact paths.
pub(crate) fn plist_body(exe: &Path, home: &Path, run_at_load: bool) -> String {
    let exe = super::xml_escape(&exe.to_string_lossy());
    let home = super::xml_escape(&home.to_string_lossy());
    let flag = super::AUTOSTART_FLAG;
    let run_at_load = if run_at_load { "<true/>" } else { "<false/>" };
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{LABEL}</string>

    <key>ProgramArguments</key>
    <array>
        <string>{exe}</string>
        <string>{flag}</string>
    </array>

    <key>RunAtLoad</key>
    {run_at_load}

    <key>StandardOutPath</key>
    <string>{home}/.meridian/logs/tray.log</string>
    <key>StandardErrorPath</key>
    <string>{home}/.meridian/logs/tray-error.log</string>

    <key>ProcessType</key>
    <string>Interactive</string>
</dict>
</plist>
"#
    )
}

/// The plist currently on disk, or `None` if absent/unreadable.
async fn read_registration() -> Option<String> {
    tokio::fs::read_to_string(launch_agents_dir()?.join(PLIST_FILE))
        .await
        .ok()
}

/// Verify and, if needed, rewrite the LaunchAgent. See
/// [`crate::autostart::ensure_registered`] for the contract.
pub(crate) async fn ensure_registered() -> RegistrationAction {
    // ORDER MATTERS — see [`super::may_drop_legacy`].
    //
    // This used to call `migrate_off_plugin()` unconditionally and FIRST, on
    // the reasoning that the legacy job is wrong in every case anyway. It is
    // not: on an install upgrading across this change the plugin's plist IS
    // the working autostart, and three paths below return WITHOUT writing a
    // replacement — the transient-path skip, and the two hard failures
    // (`create_dir_all`, the plist write). Deleting it first meant the user
    // ended those launches with no macOS autostart at all, and capture runs
    // in-process in the tray, so that is every day's data until they open the
    // app by hand.
    //
    // Windows already gated exactly this behind `may_drop_legacy`; its doc
    // warned that hoisting the call back to the top would reintroduce the bug.
    // macOS was the platform still doing it. The delete now happens only where
    // that shared table says it is safe.
    let (Some(dir), Some(home), Some(exe)) = (
        launch_agents_dir(),
        meridian_core::paths::home_dir(),
        current_exe(),
    ) else {
        tracing::warn!("autostart: could not resolve the home directory or our own path");
        return RegistrationAction::Failed;
    };

    // Skip decisions first, and they apply to BOTH mechanisms: a user who
    // turned autostart off must not get a login item either, and a transient
    // (DMG / translocated) path must not be pinned by anything.
    if let Some(skip) = super::decide_skip(
        super::disabled_by_user(),
        crate::sys::running_from_stable_location(),
    ) {
        // Disabled-by-user: dropping the legacy job IS honouring the "no".
        // Transient path: nothing replaced it, so it stays.
        if super::may_drop_legacy(skip, false) {
            migrate_off_plugin().await;
        }
        return skip;
    }

    // The LOGIN half. On macOS 13+ this is SMAppService, which is the only way
    // to appear in Login Items & Extensions as "Meridian" rather than as an
    // anonymous legacy agent. Below 13 it reports `Unavailable` and the plist
    // below carries the login trigger instead.
    let login = login_item::register();
    let owns_login = !matches!(
        login,
        login_item::RegisterOutcome::Unavailable | login_item::RegisterOutcome::Failed
    );

    if owns_login {
        // SMAppService alone covers login on macOS 13+, and there is no wake
        // relaunch to express any more (see `plist_body`'s docs — quitting
        // must stick until the next login, restart, or manual start). This
        // plist would therefore have nothing left to do, so it is not written
        // at all here: remove any copy left by an older build, both the file
        // (so a future login has nothing to load) and any definition already
        // loaded into launchd's runtime state (`disarm_relaunch` — a deleted
        // file alone does NOT stop an already-armed `LaunchEvents` trigger
        // from firing again before the next logout).
        let had_stale_file = read_registration().await.is_some();
        let dest = dir.join(PLIST_FILE);
        if had_stale_file {
            if let Err(e) = tokio::fs::remove_file(&dest).await {
                tracing::warn!(error = %e, plist = %dest.display(), "autostart: could not remove the stale relaunch plist");
            }
        }
        disarm_relaunch().await;
        // SMAppService already owns login, so the plugin-era job is safe to
        // drop unconditionally here — nothing further needs to "take over"
        // first.
        migrate_off_plugin().await;

        let action = if had_stale_file {
            RegistrationAction::RepairedStaleDefinition
        } else {
            RegistrationAction::AlreadyCorrect
        };
        tracing::debug!(
            login_item = login.as_str(),
            action = action.as_str(),
            "autostart: SMAppService owns login - no separate relaunch job needed"
        );
        return action;
    }

    // Below macOS 13: SMAppService is unavailable, so this plist is the only
    // login mechanism there is, with `RunAtLoad` true.
    let expected = plist_body(&exe, &home, true);
    let existing = read_registration().await;

    // Exact-body comparison, not a substring probe.
    //
    // The substring version (path present? marker present?) cannot see a
    // change to a field it does not name. Comparing the whole rendered body
    // makes every future field change self-healing for free, and the cost of
    // a false "differs" is one idempotent file write.
    let action = match existing.as_deref() {
        None => RegistrationAction::RegisteredMissing,
        Some(cur) if cur == expected => RegistrationAction::AlreadyCorrect,
        // Distinguish the two repairs that matter for the fleet: a moved app is
        // a different problem from a definition this build simply renders
        // differently.
        Some(cur) if !super::job_references_exe(cur, exe.to_string_lossy().as_ref()) => {
            RegistrationAction::RepairedPathDrift
        }
        Some(_) => RegistrationAction::RepairedStaleDefinition,
    };

    if action == RegistrationAction::AlreadyCorrect {
        tracing::debug!(
            login_item = login.as_str(),
            "autostart: login LaunchAgent already correct"
        );
        // Something correct is already registered, so the legacy job is now
        // redundant AND would double-launch alongside it.
        if super::may_drop_legacy(action, false) {
            migrate_off_plugin().await;
        }
        return action;
    }

    if let Err(e) = tokio::fs::create_dir_all(&dir).await {
        tracing::warn!(
            error = %e,
            dir = %dir.display(),
            "autostart: could not create LaunchAgents - KEEPING the plugin-era plist so an \
             upgraded install is not left with no autostart at all"
        );
        return RegistrationAction::Failed;
    }
    let dest = dir.join(PLIST_FILE);
    if let Err(e) = tokio::fs::write(&dest, &expected).await {
        tracing::warn!(
            error = %e,
            plist = %dest.display(),
            "autostart: could not write the plist - KEEPING the plugin-era plist so an \
             upgraded install is not left with no autostart at all"
        );
        return RegistrationAction::Failed;
    }

    // The replacement is on disk, so the legacy job is now safe to remove -
    // and MUST be, or both would start a tray at the next login.
    if super::may_drop_legacy(action, true) {
        migrate_off_plugin().await;
    }

    // Deliberately NOT bootstrapped from here: this plist carries
    // `RunAtLoad true` (it is the sole login mechanism below macOS 13), so
    // bootstrapping it now would start a second tray immediately. The
    // single-instance guard would kill that duplicate, but relying on a guard
    // to undo a launch we chose to make is worse than not making it — skip,
    // and let the next login load it.
    tracing::info!(
        plist = %dest.display(),
        action = action.as_str(),
        login_item = login.as_str(),
        "autostart: login LaunchAgent written (below macOS 13 fallback)"
    );
    action
}

/// Clear any relaunch job already loaded into launchd's runtime state.
///
/// Deleting [`PLIST_FILE`] on disk is not enough by itself: once a job with
/// `LaunchEvents` has been bootstrapped, its triggers stay armed in launchd's
/// in-memory state until an explicit `bootout` (or logout), regardless of
/// whether the file that defined them still exists. A machine that had an
/// older build's
/// wake-relaunch job loaded before upgrading would otherwise keep coming back
/// on the next unlock or display wake for the rest of that login session,
/// even though the new build never re-registers it — which is the exact "I
/// cannot quit this app" symptom this change exists to fix.
///
/// Best-effort and unawaited for confirmation, unlike
/// [`crate::backend_install::bootout_agent_and_wait`]: this runs on every
/// launch's startup path and must not add latency waiting for launchd to
/// confirm the label cleared, since the job is either already gone (the
/// common case, nothing to do) or about to be — bootout itself is
/// synchronous with the daemon side effects that matter here.
///
/// Safe to call even if the running process happens to be that job's own
/// child (which can only happen right after upgrading past this change,
/// mid-relaunch): the process is either not that job's instance at all
/// (the overwhelmingly common case — a user launch, an updater relaunch, or
/// SMAppService's own login item, none of which go through this label), or
/// it is, and `tauri-plugin-single-instance` combined with the app already
/// starting up makes an unexpected exit here no worse than the exit the user
/// was trying to cause anyway.
pub(crate) async fn disarm_relaunch() {
    let target = format!("gui/{}/{LABEL}", crate::sys::uid_str());
    // REFUSE to boot ourselves out. `bootout` unloads a job AND terminates its
    // processes, our plist deliberately carries no `KeepAlive`, and SMAppService
    // only supplies a FUTURE login launch - so if this process IS that job's
    // instance, the bootout makes the app vanish for the rest of the session
    // with nothing to bring it back.
    //
    // `migrate_off_plugin` already documents this hazard for the plugin-era
    // label and avoids it by never booting out at all. The same hazard applies
    // here, and the justification originally written above ("no worse than the
    // exit the user was trying to cause") only holds on the DISABLE path -
    // the macOS 13+ startup path calls this while the user has asked for
    // nothing of the sort.
    //
    // Skipping is cheap. The bootout only exists to stop an already-armed
    // `LaunchEvents` trigger firing once more before the next logout; if it
    // does fire, `tauri-plugin-single-instance` turns the second launch into a
    // no-op. A spurious relaunch attempt is strictly better than a dead tray.
    if job_is_running_this_process(&target).await {
        tracing::info!(
            label = LABEL,
            "autostart: skipping bootout - this process IS that job's instance, and \
             booting it out would terminate the tray with nothing to restart it"
        );
        return;
    }
    let _ = crate::backend_install::launchctl(&["bootout", &target]).await;
}

/// Does `target`'s launchd job currently own THIS process?
async fn job_is_running_this_process(target: &str) -> bool {
    let Ok(out) = tokio::process::Command::new("launchctl")
        .args(["print", target])
        .output()
        .await
    else {
        // Cannot tell. Fail SAFE - assume it might be us and skip the bootout,
        // because the cost of a wrong "no" (a dead tray) far exceeds the cost of
        // a wrong "yes" (one `LaunchEvents` trigger fires and single-instance
        // absorbs it).
        return true;
    };
    let text = String::from_utf8_lossy(&out.stdout);
    job_pid_from_print(&text) == Some(std::process::id())
}

/// The `pid = N` a `launchctl print` block reports for a running job.
///
/// `None` when the job is loaded but not running (no `pid` line), when the label
/// is unknown (launchctl errors and prints nothing), or when the field is not a
/// number.
fn job_pid_from_print(text: &str) -> Option<u32> {
    text.lines()
        .map(str::trim)
        .find_map(|l| l.strip_prefix("pid = "))
        .and_then(|v| v.trim().parse().ok())
}

/// Delete the plugin-era `Meridian.plist`, so an upgraded install does not end
/// up with two jobs both starting a tray at login.
///
/// Idempotent and silent: on all but one launch in an install's life there is
/// nothing there.
///
/// # It must NOT `launchctl bootout` first
/// This is the tempting version and it is a self-inflicted crash. `bootout`
/// unloads a job AND terminates its processes — and on an existing install the
/// tray IS that job's process, because the plugin's login item is what started
/// it. Booting the label out from inside the running tray therefore kills the
/// tray, seconds after an update, with no `KeepAlive` to bring it back. The
/// user's app would simply vanish at login.
///
/// Deleting the file alone is both safe and sufficient: the plugin's plist
/// carries only `RunAtLoad`, which fires at load time (session start, already
/// past) and never again. A loaded-but-deleted job spontaneously starts
/// nothing, and at the next login launchd has no file to load. The only cost is
/// that the stale label lingers in `launchctl print` until logout, which is
/// cosmetic.
async fn migrate_off_plugin() {
    let Some(legacy) = launch_agents_dir().map(|d| d.join(LEGACY_PLIST_FILE)) else {
        return;
    };
    if !legacy.exists() {
        return;
    }
    match tokio::fs::remove_file(&legacy).await {
        Ok(()) => tracing::info!(
            plist = %legacy.display(),
            "autostart: removed the plugin-era login item"
        ),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            tracing::warn!(error = %e, plist = %legacy.display(), "autostart: could not remove the plugin-era login item")
        }
    }
}

/// Remove the LaunchAgent, honouring a user who turned autostart off.
///
/// # Also deliberately does not `bootout`
/// Same reason as [`migrate_off_plugin`], and here the consequence would be
/// even more obviously wrong: the user unticks "Start Meridian automatically"
/// in Settings and the app they are looking at quits, because they are running
/// as the job being booted out.
///
/// Below macOS 13 this plist carries only `RunAtLoad`, which already fired at
/// this session's login and never fires again — so deleting the file is
/// enough to fully honour the setting from this point on, not just from the
/// next login. (On 13+ this module never wrote a plist at all, so there is
/// nothing here to remove; [`login_item::unregister`] above is the whole
/// story.)
pub(crate) async fn unregister() {
    // The login half first. Unlike `launchctl bootout` this does NOT terminate
    // the running app, so it is safe to call from the Settings toggle while the
    // user is looking at the window — which is the whole reason the plist below
    // is deleted rather than booted out.
    login_item::unregister();

    let Some(plist) = launch_agents_dir().map(|d| d.join(PLIST_FILE)) else {
        return;
    };
    match tokio::fs::remove_file(&plist).await {
        Ok(()) => tracing::info!(plist = %plist.display(), "autostart: LaunchAgent removed"),
        // Already gone - the normal case when autostart was never on.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            tracing::warn!(error = %e, plist = %plist.display(), "autostart: could not remove the LaunchAgent")
        }
    }
}

/// Live registration state for analytics. See [`super::Status`].
pub(crate) async fn status() -> Status {
    // The plist half, then the login half folded on top: they are independent
    // mechanisms and either can be broken while the other is fine.
    let mut status = super::status_from(
        read_registration().await.as_deref(),
        current_exe()
            .map(|p| p.to_string_lossy().into_owned())
            .as_deref(),
    );
    status.login_item = Some(login_item::status().as_str());
    status
}

#[cfg(test)]
mod tests {

    /// `disarm_relaunch` must never terminate the tray it is running inside.
    ///
    /// `bootout` unloads a job AND kills its processes; our plist carries no
    /// `KeepAlive`; SMAppService only arranges a FUTURE login launch. So booting
    /// out our own label makes the app vanish for the session. The macOS 13+
    /// startup path calls this while the user has asked for nothing, which is
    /// where the original justification ("no worse than the exit the user was
    /// trying to cause") stops applying.
    #[test]
    fn the_running_jobs_pid_is_read_from_launchctl_print() {
        let sample = "\
com.meridiona.tray = {
\tactive count = 1
\tpath = /Users/t/Library/LaunchAgents/com.meridiona.tray.plist
\tstate = running
\tpid = 4711
\tprogram = /Applications/Meridian.app/Contents/MacOS/Meridian
}";
        assert_eq!(job_pid_from_print(sample), Some(4711));
    }

    /// A loaded-but-not-running job, an unknown label, and a malformed field
    /// must all read as "no pid" rather than a wrong one.
    #[test]
    fn a_job_with_no_running_instance_reports_no_pid() {
        assert_eq!(job_pid_from_print("state = not running\n"), None);
        assert_eq!(job_pid_from_print(""), None);
        assert_eq!(job_pid_from_print("pid = notanumber"), None);
        assert_eq!(
            job_pid_from_print("last exit code = 0\nactive count = 0"),
            None
        );
    }
    use super::*;

    fn body(run_at_load: bool) -> String {
        plist_body(
            Path::new("/Applications/Meridian.app/Contents/MacOS/Meridian"),
            Path::new("/Users/tester"),
            run_at_load,
        )
    }

    /// The label must match the file name AND the `com.meridiona.*` shape that
    /// `src/uninstall.rs` sweeps - getting either wrong is how the plugin's
    /// login item ended up surviving every uninstall.
    #[test]
    fn label_matches_the_file_name_and_the_uninstall_glob() {
        assert_eq!(PLIST_FILE, format!("{LABEL}.plist"));
        assert!(LABEL.starts_with("com.meridiona."));
    }

    /// `run_at_load` still varies as a property of this pure function, even
    /// though [`ensure_registered`] only ever calls it with `true` now (the
    /// `false`/SMAppService-owns-login case no longer writes a plist at all -
    /// see its docs). Pinned here so a `false` plist can never silently start
    /// a second tray alongside SMAppService's login item if that changes -
    /// two processes writing one SQLite file is the double-writer condition
    /// behind `database disk image is malformed`.
    #[test]
    fn run_at_load_is_false_when_smappservice_owns_login() {
        let with_login_item = body(false);
        assert!(with_login_item.contains("<key>RunAtLoad</key>\n    <false/>"));
        assert!(!with_login_item.contains("<true/>"));

        // And true on the fallback path, where nothing else covers login.
        let fallback = body(true);
        assert!(fallback.contains("<key>RunAtLoad</key>\n    <true/>"));
    }

    /// **The regression test for "I cannot quit this app".** An earlier
    /// version of this plist carried `LaunchEvents` on a set of Darwin
    /// notifications (unlock, display wake, power state, clamshell) so
    /// Meridian relaunched itself on those events rather than at a fixed
    /// hour. Those notifications fire many times in an ordinary day - every
    /// screen lock/unlock, every display sleep/wake - so a user who quit the
    /// app watched it come back within minutes. The requirement is login,
    /// restart, or a manual start, never an unattended resurrection before
    /// then, so this plist must carry neither a clock NOR a wake trigger in
    /// either mode - it is `RunAtLoad` and nothing else.
    #[test]
    fn plist_carries_no_wake_relaunch_trigger_and_no_clock() {
        for run_at_load in [true, false] {
            let b = body(run_at_load);
            assert!(
                !b.contains("LaunchEvents") && !b.contains("notifyd.matching"),
                "a wake-relaunch trigger came back (run_at_load={run_at_load}) - \
                 quitting must stick until the next login, not the next unlock"
            );
            assert!(
                !b.contains("StartCalendarInterval") && !b.contains("StartInterval"),
                "a clock came back - the requirement is login/restart/start, not a time"
            );
            assert!(b.contains(super::super::AUTOSTART_FLAG));
        }
    }

    /// `KeepAlive` would make Quit impossible - the user explicitly wants
    /// quitting to stick until the next login, which is the entire reason
    /// this plist differs from the daemon's.
    #[test]
    fn plist_must_not_carry_keepalive() {
        assert!(!body(true).contains("KeepAlive"));
        assert!(!body(false).contains("KeepAlive"));
    }

    /// The log paths are the crash safety net `telemetry_spool::launchd_log_cap`
    /// size-caps by exact name; a rename here would silently uncap them.
    #[test]
    fn plist_redirects_to_the_capped_log_paths() {
        let b = body(true);
        assert!(b.contains("/Users/tester/.meridian/logs/tray.log"));
        assert!(b.contains("/Users/tester/.meridian/logs/tray-error.log"));
    }

    /// An `&` in a user's home directory name would otherwise produce a plist
    /// `plutil` rejects, and the failure would surface as "autostart silently
    /// never works" for exactly those users.
    #[test]
    fn paths_are_xml_escaped() {
        let b = plist_body(
            Path::new("/Users/a&b/Meridian.app/Contents/MacOS/Meridian"),
            Path::new("/Users/a&b"),
            true,
        );
        assert!(b.contains("/Users/a&amp;b/Meridian.app"));
        assert!(!b.contains("/Users/a&b"));
    }

    /// The two modes must render DIFFERENTLY, which is what makes the exact-body
    /// comparison in `ensure_registered` able to detect a stale definition. If
    /// they were byte-identical, an install carrying the old `RunAtLoad true`
    /// plist would never be repaired and would double-launch forever.
    #[test]
    fn the_two_modes_are_distinguishable_by_an_exact_compare() {
        assert_ne!(body(true), body(false));
    }
}
