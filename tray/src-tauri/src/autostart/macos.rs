//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! macOS side of [`crate::autostart`] — a per-user launchd LaunchAgent carrying
//! both the login trigger and the morning relaunch.
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

/// Darwin notifications that mean "the device just became usable again".
///
/// See [`plist_body`] for why this is a SET rather than one name, and for where
/// each came from. Order is irrelevant; launchd keys the dict by name.
pub(crate) const WAKE_NOTIFICATIONS: [&str; 7] = [
    // Unlock. macOS requires a password after sleep by default, so this is the
    // common path for "opened the laptop". Apple triggers its own
    // com.apple.contextstored on this exact name.
    "com.apple.sessionagent.screenIsUnlocked",
    // Desktop up after login / fast user switch.
    "com.apple.system.loginwindow.desktopUp",
    // The display coming back on - the closest thing to "the screen is usable
    // again" for a machine woken without a password prompt.
    "com.apple.iokit.hid.displayStatus",
    // System power state change (sleep/wake). BOTH spellings are listed on
    // purpose: Apple's own plists use the `com.apple.powermanagement.*` form
    // while the community-collected list uses `com.apple.system.powermanagement.*`.
    // One of them is wrong on any given release and neither costs anything -
    // see `plist_body` for why an unknown name is inert.
    "com.apple.system.powermanagement.systempowerstate",
    "com.apple.powermanagement.systempowerstate",
    // A power event the user actually sees, i.e. a real wake rather than a dark
    // wake for maintenance.
    "com.apple.system.powermanagement.uservisiblepowerevent",
    // The laptop lid - literally "device opened". Fires on close too, which is a
    // harmless no-op thanks to the single-instance guard.
    "com.apple.system.powermanagement.clamshellstate",
];

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
/// # There is no clock in here, deliberately
/// The requirement is "Meridian is running after the device is turned on or
/// woken" — an EVENT, not a time. This used to be a `StartCalendarInterval` at a
/// hardcoded hour, which is the wrong shape for that: it fires when the user
/// isn't there, and misses them when they are.
///
/// launchd has no `wake` or `unlock` trigger among its start keys (`RunAtLoad`,
/// `StartInterval`, `StartCalendarInterval`, `WatchPaths`, `QueueDirectories`,
/// `StartOnMount`, `KeepAlive`, `MachServices`, `Sockets`) — but `LaunchEvents`
/// with the `com.apple.notifyd.matching` stream starts a job on an arbitrary
/// Darwin notification, which is how Apple's own agents do this. The shape here
/// is copied from `/System/Library/LaunchDaemons/com.apple.contextstored.plist`,
/// which triggers on the same unlock notification, and the mechanism was
/// verified end-to-end on macOS 26: a probe agent did not run on bootstrap and
/// ran the instant its notification was posted.
///
/// # Why SEVERAL notifications rather than the one right one
/// These names are private, so Apple documents none of them and no single one
/// can be relied on across releases. That would normally be a reason for
/// caution; here it is free, for two reasons:
///
/// - A notification launchd never receives is simply **inert** — an unknown name
///   costs nothing and fails silently in the safe direction.
/// - A notification that fires while the tray is ALREADY running costs a process
///   that exits in milliseconds, because `tauri-plugin-single-instance` is
///   registered first (see [`crate::run`]).
///
/// So the set is chosen for coverage, not minimality, and no single guess has to
/// be correct:
///
/// See [`WAKE_NOTIFICATIONS`] for the list and what each one covers. Both
/// spellings of the power-state notification are included because Apple's plists
/// and the community-collected list disagree, and being wrong about one is free.
///
/// **Measured, not assumed** (macOS 26): a plist carrying a deliberately bogus
/// notification name alongside real ones still bootstrapped cleanly, and the
/// real notification still started the job. So an unknown or renamed name
/// degrades to "never fires" rather than breaking the whole registration.
///
/// Login itself is NOT here: on macOS 13+ it belongs to `SMAppService`
/// ([`super::login_item`]), which is what produces a named, user-togglable
/// "Meridian" entry in Login Items & Extensions.
///
/// # The rest
/// - `run_at_load` is the coordination with SMAppService. FALSE when SMAppService
///   owns login, or both would start a tray at login. Below macOS 13 there is no
///   SMAppService, so it is true and this plist carries login too.
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
    // Rendered from the table rather than hand-written, so adding a trigger is
    // one entry and the plist cannot drift from the list.
    let events: String = WAKE_NOTIFICATIONS
        .iter()
        .map(|n| {
            format!(
                "            <key>{n}</key>\n                             <dict>\n                <key>Notification</key>\n                                 <string>{n}</string>\n            </dict>\n"
            )
        })
        .collect();
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

    <key>LaunchEvents</key>
    <dict>
        <key>com.apple.notifyd.matching</key>
        <dict>
{events}        </dict>
    </dict>

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
    // Best-effort and first: an upgraded install has the plugin's plist too,
    // and leaving it would mean two jobs both starting a tray at login. Done
    // even when the decision below turns out to be a skip, because the legacy
    // job is wrong in every one of those cases as well.
    migrate_off_plugin().await;

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
    let skip = super::decide(
        super::disabled_by_user(),
        crate::sys::running_from_stable_location(),
        // `Some("")` rather than `None`: this call is only being asked about the
        // two skip conditions, and a `None` here would answer
        // `RegisteredMissing` before they were consulted.
        Some(""),
        "",
        "",
    );
    if matches!(
        skip,
        RegistrationAction::SkippedDisabledByUser | RegistrationAction::SkippedTransientPath
    ) {
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

    // The MORNING half. `run_at_load` is the inverse of who owns login: if
    // SMAppService took it, this plist must NOT also start a tray at login, or
    // both fire and two processes write one SQLite file.
    let run_at_load_owned_by_plist = !owns_login;
    let expected = plist_body(&exe, &home, run_at_load_owned_by_plist);
    let existing = read_registration().await;

    // Exact-body comparison, not a substring probe.
    //
    // The substring version (path present? morning trigger present?) cannot see
    // a change to a field it does not name — and `RunAtLoad` is exactly such a
    // field. An install carrying the previous build's `RunAtLoad true` plist
    // would have passed every substring check while double-launching alongside
    // the new login item. Comparing the whole rendered body makes every future
    // field change self-healing for free, and the cost of a false "differs" is
    // one idempotent file write.
    let action = match existing.as_deref() {
        None => RegistrationAction::RegisteredMissing,
        Some(cur) if cur == expected => RegistrationAction::AlreadyCorrect,
        // Distinguish the two repairs that matter for the fleet: a moved app is
        // a different problem from a definition this build simply renders
        // differently.
        Some(cur) if !cur.contains(exe.to_string_lossy().as_ref()) => {
            RegistrationAction::RepairedPathDrift
        }
        Some(_) => RegistrationAction::RepairedStaleDefinition,
    };

    if action == RegistrationAction::AlreadyCorrect {
        tracing::debug!(
            login_item = login.as_str(),
            "autostart: login item and morning trigger both already correct"
        );
        return action;
    }

    if let Err(e) = tokio::fs::create_dir_all(&dir).await {
        tracing::warn!(error = %e, dir = %dir.display(), "autostart: could not create LaunchAgents");
        return RegistrationAction::Failed;
    }
    let dest = dir.join(PLIST_FILE);
    if let Err(e) = tokio::fs::write(&dest, &expected).await {
        tracing::warn!(error = %e, plist = %dest.display(), "autostart: could not write the plist");
        return RegistrationAction::Failed;
    }

    // NOW bootstrap it, which is safe for the first time.
    //
    // This used to be skipped, on the grounds that `bootstrap` honours
    // `RunAtLoad` and would start a second tray. Two things changed and both
    // matter:
    //
    // 1. On macOS 13+ this plist carries `RunAtLoad false` (SMAppService owns
    //    login), so bootstrapping loads the calendar trigger WITHOUT launching
    //    anything at all.
    // 2. `tauri-plugin-single-instance` now guarantees that even if something
    //    did launch a second process, it exits before touching the tray or the
    //    database.
    //
    // The gain is not cosmetic: without bootstrapping, launchd only picks the
    // job up at the next login, so a user who installed and then quit the same
    // day would NOT come back on the new-day trigger. Bootstrapping closes that gap, so the
    // morning relaunch works from the moment of install.
    //
    // Best-effort: `bootout` first so a re-registration replaces cleanly, and a
    // failure here only costs the current session (the next login loads it from
    // disk regardless).
    let target = format!("gui/{}/{LABEL}", crate::sys::uid_str());
    if run_at_load_owned_by_plist {
        // Below macOS 13 this plist DOES carry `RunAtLoad true`, so bootstrapping
        // it here would start a second tray. The single-instance guard would kill
        // that duplicate, but relying on a guard to undo a launch we chose to
        // make is worse than not making it: skip, and let the next login load it.
        tracing::debug!("autostart: not bootstrapping a RunAtLoad plist from inside the app");
    } else {
        let _ = crate::backend_install::launchctl(&["bootout", &target]).await;
        let _ = crate::backend_install::launchctl(&[
            "bootstrap",
            &format!("gui/{}", crate::sys::uid_str()),
            &dest.to_string_lossy(),
        ])
        .await;
    }

    tracing::info!(
        plist = %dest.display(),
        action = action.as_str(),
        login_item = login.as_str(),
        run_at_load = run_at_load_owned_by_plist,
        "autostart: morning-relaunch LaunchAgent written"
    );
    action
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
/// The accepted cost is narrow. Our plist DOES carry a morning trigger, and it
/// stays live until logout, so a user who turns autostart off AND quits later
/// the same day could still be relaunched once by the new-day trigger. From the
/// next login on
/// there is no plist to load and the setting is fully honoured. Relaunching
/// once beats quitting the app out from under someone who was changing a
/// preference.
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

    /// THE double-launch guard. On macOS 13+ SMAppService owns login, so this
    /// plist must carry `RunAtLoad false` and nothing else may start a tray at
    /// login. Both firing means two processes writing one SQLite file - the
    /// double-writer condition behind `database disk image is malformed`.
    #[test]
    fn run_at_load_is_false_when_smappservice_owns_login() {
        let with_login_item = body(false);
        assert!(with_login_item.contains("<key>RunAtLoad</key>\n    <false/>"));
        assert!(!with_login_item.contains("<true/>"));

        // And true on the fallback path, where nothing else covers login.
        let fallback = body(true);
        assert!(fallback.contains("<key>RunAtLoad</key>\n    <true/>"));
    }

    /// The wake triggers are this plist's reason to exist in BOTH modes -
    /// SMAppService cannot express them at all, and there is no clock left.
    #[test]
    fn both_modes_carry_every_wake_notification_and_no_clock() {
        for run_at_load in [true, false] {
            let b = body(run_at_load);
            assert!(
                b.contains("<key>LaunchEvents</key>"),
                "run_at_load={run_at_load}"
            );
            assert!(
                b.contains("com.apple.notifyd.matching"),
                "run_at_load={run_at_load}"
            );
            for n in WAKE_NOTIFICATIONS {
                assert!(b.contains(n), "missing {n} (run_at_load={run_at_load})");
            }
            assert!(b.contains(super::super::AUTOSTART_FLAG));
            // The whole point of the rewrite: no hardcoded time survives.
            assert!(
                !b.contains("StartCalendarInterval") && !b.contains("StartInterval"),
                "a clock came back - the requirement is an event, not a time"
            );
        }
    }

    /// Each notification must appear as its own `{{ Notification: <name> }}` dict
    /// under `com.apple.notifyd.matching`, which is the shape Apple's own
    /// `com.apple.contextstored.plist` uses. A flat array silently never fires.
    #[test]
    fn each_notification_is_rendered_in_apples_dict_shape() {
        let b = body(false);
        for n in WAKE_NOTIFICATIONS {
            assert!(
                b.contains(&format!("<key>{n}</key>")),
                "{n} is not a key in the matching dict"
            );
            assert!(
                b.contains(&format!("<string>{n}</string>")),
                "{n} has no Notification value"
            );
        }
        assert_eq!(
            b.matches("<key>Notification</key>").count(),
            WAKE_NOTIFICATIONS.len(),
            "one Notification key per notification"
        );
    }

    /// `KeepAlive` would make Quit impossible - the user explicitly wants
    /// quitting to stick until the next morning, which is the entire reason
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
