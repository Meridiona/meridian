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

use super::{RegistrationAction, Status};
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

/// The legacy plist's launchd label — the plugin used the app name for both.
const LEGACY_LABEL: &str = "Meridian";

/// Element that proves a plist carries the morning relaunch. See
/// [`super::decide`] for why this is a text check.
const MORNING_TRIGGER_MARKER: &str = "StartCalendarInterval";

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
/// - `RunAtLoad` covers login (and therefore reboot).
/// - `StartCalendarInterval` covers "the user quit; bring it back tomorrow
///   morning". launchd runs a missed calendar job when the machine wakes, so a
///   laptop asleep at [`super::MORNING_HOUR`] still gets it.
/// - **No `KeepAlive`**, deliberately: with it, Quit would be undone within
///   seconds and there would be no way to stop Meridian for the afternoon. The
///   daemon's plist makes the opposite choice because it is headless and has no
///   Quit.
/// - [`super::AUTOSTART_FLAG`] is passed so the tray knows this launch was
///   unattended and must not open a window.
/// - `ProcessType` `Interactive` keeps launchd from throttling a job that owns
///   UI.
/// - The stdout/stderr redirects are the OS-level crash safety net described in
///   `CLAUDE.md`'s observability section — the one thing the OTel spool cannot
///   capture. `src/telemetry_spool/launchd_log_cap.rs` already size-caps these
///   exact paths.
pub(crate) fn plist_body(exe: &Path, home: &Path) -> String {
    let exe = super::xml_escape(&exe.to_string_lossy());
    let home = super::xml_escape(&home.to_string_lossy());
    let flag = super::AUTOSTART_FLAG;
    let hour = super::MORNING_HOUR;
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
    <true/>

    <key>StartCalendarInterval</key>
    <array>
        <dict>
            <key>Hour</key>
            <integer>{hour}</integer>
            <key>Minute</key>
            <integer>0</integer>
        </dict>
    </array>

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
fn read_registration() -> Option<String> {
    std::fs::read_to_string(launch_agents_dir()?.join(PLIST_FILE)).ok()
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

    let existing = read_registration();
    let action = super::decide(
        super::disabled_by_user(),
        crate::sys::running_from_stable_location(),
        existing.as_deref(),
        &exe.to_string_lossy(),
        MORNING_TRIGGER_MARKER,
    );
    if !matches!(
        action,
        RegistrationAction::RegisteredMissing
            | RegistrationAction::RepairedPathDrift
            | RegistrationAction::RepairedMissingMorningTrigger
    ) {
        return action;
    }

    if let Err(e) = tokio::fs::create_dir_all(&dir).await {
        tracing::warn!(error = %e, dir = %dir.display(), "autostart: could not create LaunchAgents");
        return RegistrationAction::Failed;
    }
    let dest = dir.join(PLIST_FILE);
    if let Err(e) = tokio::fs::write(&dest, plist_body(&exe, &home)).await {
        tracing::warn!(error = %e, plist = %dest.display(), "autostart: could not write the plist");
        return RegistrationAction::Failed;
    }

    // NOT bootstrapped - see the module docs on `crate::autostart`. launchd
    // loads this directory at session start, so the job is live from the next
    // login onward; bootstrapping it here would honour `RunAtLoad` and start a
    // second tray against the same SQLite file.
    tracing::info!(
        plist = %dest.display(),
        action = action.as_str(),
        "autostart: LaunchAgent written - live from the next login"
    );
    action
}

/// Boot out and delete the plugin-era `Meridian.plist`.
///
/// Idempotent and silent: on all but one launch in an install's life there is
/// nothing there. `bootout` before the delete because removing the file alone
/// leaves the job loaded for the rest of the session, still able to start a
/// duplicate tray at the plugin's `RunAtLoad`.
async fn migrate_off_plugin() {
    let Some(legacy) = launch_agents_dir().map(|d| d.join(LEGACY_PLIST_FILE)) else {
        return;
    };
    if !legacy.exists() {
        return;
    }
    let target = format!("gui/{}/{LEGACY_LABEL}", crate::sys::uid_str());
    let _ = crate::backend_install::launchctl(&["bootout", &target]).await;
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
/// `bootout` before the delete: deleting the file alone leaves the job loaded
/// for the rest of the login session, so launchd would still honour the morning
/// trigger today — the user's "no" would appear to have been ignored.
pub(crate) async fn unregister() {
    let Some(plist) = launch_agents_dir().map(|d| d.join(PLIST_FILE)) else {
        return;
    };
    let target = format!("gui/{}/{LABEL}", crate::sys::uid_str());
    let _ = crate::backend_install::launchctl(&["bootout", &target]).await;
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
pub(crate) fn status() -> Status {
    super::status_from(
        read_registration().as_deref(),
        current_exe()
            .map(|p| p.to_string_lossy().into_owned())
            .as_deref(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body() -> String {
        plist_body(
            Path::new("/Applications/Meridian.app/Contents/MacOS/Meridian"),
            Path::new("/Users/tester"),
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

    #[test]
    fn plist_carries_both_triggers_and_the_autostart_flag() {
        let b = body();
        assert!(b.contains("<key>RunAtLoad</key>"), "login trigger missing");
        assert!(
            b.contains(MORNING_TRIGGER_MARKER),
            "morning trigger missing"
        );
        assert!(
            b.contains(&format!(
                "<integer>{}</integer>",
                super::super::MORNING_HOUR
            )),
            "morning hour missing"
        );
        assert!(b.contains(super::super::AUTOSTART_FLAG));
        assert!(b.contains("/Applications/Meridian.app/Contents/MacOS/Meridian"));
    }

    /// `KeepAlive` would make Quit impossible - the user explicitly wants
    /// quitting to stick until the next morning, which is the entire reason
    /// this plist differs from the daemon's.
    #[test]
    fn plist_must_not_carry_keepalive() {
        assert!(!body().contains("KeepAlive"));
    }

    /// The log paths are the crash safety net `telemetry_spool::launchd_log_cap`
    /// size-caps by exact name; a rename here would silently uncap them.
    #[test]
    fn plist_redirects_to_the_capped_log_paths() {
        let b = body();
        assert!(b.contains("/Users/tester/.meridian/logs/tray.log"));
        assert!(b.contains("/Users/tester/.meridian/logs/tray-error.log"));
    }

    /// A plist this module wrote must be recognised as correct by the very
    /// decision function that decides whether to rewrite it - otherwise every
    /// launch would rewrite and report a repair forever.
    #[test]
    fn a_freshly_written_plist_reads_back_as_already_correct() {
        let exe = "/Applications/Meridian.app/Contents/MacOS/Meridian";
        let b = plist_body(Path::new(exe), Path::new("/Users/tester"));
        assert_eq!(
            super::super::decide(false, true, Some(&b), exe, MORNING_TRIGGER_MARKER),
            RegistrationAction::AlreadyCorrect
        );
    }

    /// An `&` in a user's home directory name would otherwise produce a plist
    /// `plutil` rejects, and the failure would surface as "autostart silently
    /// never works" for exactly those users.
    #[test]
    fn paths_are_xml_escaped() {
        let b = plist_body(
            Path::new("/Users/a&b/Meridian.app/Contents/MacOS/Meridian"),
            Path::new("/Users/a&b"),
        );
        assert!(b.contains("/Users/a&amp;b/Meridian.app"));
        assert!(!b.contains("/Users/a&b"));
    }
}
