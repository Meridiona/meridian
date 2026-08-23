//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Windows side of [`crate::autostart`] — one per-user scheduled task carrying
//! both a logon trigger and a daily trigger.
//!
//! # The triggers are EVENTS, not a clock
//! The requirement is "Meridian is running after the device is turned on or
//! woken". Windows expresses that natively: `SessionStateChangeTrigger` fires on
//! `SessionUnlock` (the user unlocked the workstation) and `ConsoleConnect` (the
//! user connected to the session), alongside `LogonTrigger` for sign-in. There
//! is no hardcoded time anywhere.
//!
//! This replaced a daily `CalendarTrigger` at a fixed hour, which was the wrong
//! shape for the requirement: it fired when the user was not there and missed
//! them when they were. macOS gets the same treatment through launchd's
//! `LaunchEvents` (see [`super::macos`]); the two platforms now use each OS's
//! own event system rather than a clock.
//!
//! Firing on every unlock is safe because `tauri-plugin-single-instance` is
//! registered first, so a launch into an already-running tray exits in
//! milliseconds.
//!
//! # Why XML, not `schtasks /SC`
//! The command form takes exactly one `/SC`, so a login task and a daily task
//! would have to be two separate tasks — and two tasks are two independent
//! instance policies, so the daily trigger would start a second tray on top of the
//! one the logon task already started. Registering from XML allows one task
//! with two triggers, and therefore one `MultipleInstancesPolicy` of
//! `IgnoreNew`, which is what makes the morning trigger a no-op while the tray
//! is already running. That is the same property launchd gives for free on
//! macOS by treating a job as a singleton.
//!
//! # Three ways to register, in descending order of fidelity
//! Managed machines can forbid a standard user from creating scheduled tasks at
//! all (the same policy [`crate::backend_install`] already works around for the
//! daemon), so this degrades rather than giving up:
//!
//! 1. `schtasks /Create /XML` — logon + unlock + console-connect, `IgnoreNew`.
//! 2. `schtasks /Create /SC ONLOGON` — logon only. Loses the wake triggers, keeps
//!    the restart case. The command form takes one `/SC` and cannot express a
//!    session-state trigger at all.
//! 3. A hidden VBScript in the Startup folder — logon only, no Task Scheduler
//!    involvement at all.
//!
//! Each fallback is logged at WARN, so "how many Windows installs cannot wake
//! Meridian on unlock" is answerable from the fleet rather than guessed at.
//!
//! # Who calls this
//! [`crate::autostart::ensure_registered`] and [`crate::autostart::status`].

use super::{may_drop_legacy, RegistrationAction, Status};
use meridian_core::proc_ext::NoWindow;
use std::path::{Path, PathBuf};

/// Task Scheduler name. Deliberately distinct from the daemon's
/// `Meridian Daemon` ([`crate::backend_install`]): they are separate processes
/// with separate lifecycles, and `src/uninstall.rs` deletes them by name.
pub(crate) const TASK_NAME: &str = "Meridian Tray";

/// Element that proves the registered task carries the wake triggers. See
/// [`super::decide`] for why this is a text check.
///
/// It is also the upgrade detector for installs registered by an earlier build,
/// whose task carried a `CalendarTrigger` at a hardcoded hour instead.
const MORNING_TRIGGER_MARKER: &str = "SessionStateChangeTrigger";

/// The `HKCU\...\Run` value `tauri-plugin-autostart` used to write, named after
/// `productName`. Removed on first run so an upgraded install does not start
/// the tray twice at logon.
const LEGACY_RUN_VALUE: &str = "Meridian";

/// Startup-folder fallback launcher. `.vbs` rather than `.bat` so it runs
/// through `wscript.exe` with no console flash — the same reasoning as
/// [`crate::backend_install`]'s daemon launcher, which this deliberately does
/// not share a file name with.
const STARTUP_LAUNCHER_NAME: &str = "MeridianTray.vbs";

/// Where the generated task definition is written before `schtasks` reads it.
/// Kept (not deleted) after a successful registration: it is the only
/// human-readable record of what was registered, and it is what you want to
/// look at first when a Windows install is not coming back.
const TASK_XML_FILE: &str = "meridian-tray-task.xml";

/// The running executable, canonicalised for the same reason as on macOS: the
/// recorded path has to be byte-comparable against a later launch's, or every
/// startup reports path drift and rewrites.
fn current_exe() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    Some(exe.canonicalize().unwrap_or(exe))
}

/// The Task Scheduler XML.
///
/// Pure, so every element below is unit-testable without a Windows host — this
/// file cannot be exercised on the macOS/Linux machines the repo is developed
/// and CI'd on, which makes the tests the only guard it has.
///
/// The non-obvious settings, each of which has a specific failure it prevents:
/// - `MultipleInstancesPolicy` `IgnoreNew` — the whole reason for using XML.
/// - `DisallowStartIfOnBatteries` / `StopIfGoingOnBatteries` `false` — both
///   default to TRUE in Task Scheduler, which on a laptop means the tray never
///   starts on battery and is KILLED when the machine unplugs. That default
///   would silently make Meridian a desktop-only product.
/// - `StartWhenAvailable` `true` — runs a missed new-day trigger once the machine
///   is next awake, matching launchd's behaviour on macOS.
/// - `ExecutionTimeLimit` `PT0S` — no limit. The default (72 hours) would
///   terminate a long-running tray.
/// - `RunLevel` `LeastPrivilege` — never elevated. An elevated tray would write
///   files under `~/.meridian` that the un-elevated one could not then touch.
pub(crate) fn task_xml(exe: &Path) -> String {
    let exe = super::xml_escape(&exe.to_string_lossy());
    let flag = super::xml_escape(super::AUTOSTART_FLAG);
    format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo>
    <Description>Starts Meridian in the background at sign-in, and again whenever you unlock or reconnect to this session.</Description>
  </RegistrationInfo>
  <Triggers>
    <LogonTrigger>
      <Enabled>true</Enabled>
    </LogonTrigger>
    <SessionStateChangeTrigger>
      <Enabled>true</Enabled>
      <StateChange>SessionUnlock</StateChange>
    </SessionStateChangeTrigger>
    <SessionStateChangeTrigger>
      <Enabled>true</Enabled>
      <StateChange>ConsoleConnect</StateChange>
    </SessionStateChangeTrigger>
  </Triggers>
  <Principals>
    <Principal id="Author">
      <LogonType>InteractiveToken</LogonType>
      <RunLevel>LeastPrivilege</RunLevel>
    </Principal>
  </Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <AllowHardTerminate>true</AllowHardTerminate>
    <StartWhenAvailable>true</StartWhenAvailable>
    <RunOnlyIfNetworkAvailable>false</RunOnlyIfNetworkAvailable>
    <IdleSettings>
      <StopOnIdleEnd>false</StopOnIdleEnd>
      <RestartOnIdle>false</RestartOnIdle>
    </IdleSettings>
    <AllowStartOnDemand>true</AllowStartOnDemand>
    <Enabled>true</Enabled>
    <Hidden>false</Hidden>
    <RunOnlyIfIdle>false</RunOnlyIfIdle>
    <WakeToRun>false</WakeToRun>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <Priority>7</Priority>
  </Settings>
  <Actions Context="Author">
    <Exec>
      <Command>{exe}</Command>
      <Arguments>{flag}</Arguments>
    </Exec>
  </Actions>
</Task>
"#
    )
}

/// Encode as UTF-16LE with a BOM.
///
/// `schtasks /Create /XML` rejects a UTF-8 task definition on some Windows
/// builds with a bare "The task XML is malformed", which is indistinguishable
/// from a real schema error — so the encoding is not a detail that can be left
/// to chance. UTF-16LE with a BOM is accepted everywhere.
pub(crate) fn utf16le_with_bom(s: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(2 + s.len() * 2);
    out.extend_from_slice(&[0xFF, 0xFE]);
    for unit in s.encode_utf16() {
        out.extend_from_slice(&unit.to_le_bytes());
    }
    out
}

/// Decode `schtasks` stdout, which may be UTF-16LE (with or without a BOM) or
/// the console code page depending on the Windows build and how the process was
/// launched.
///
/// This only ever feeds a `contains` check ([`super::decide`]), so a
/// best-effort decode is sufficient and a wrong guess costs one redundant
/// rewrite. Guessing by counting NUL bytes rather than trusting the BOM
/// alone: BOM-less UTF-16 output is real, and decoding it as UTF-8 yields a
/// string with a NUL between every character, which would match no needle at
/// all and so report a permanent false "needs repair".
pub(crate) fn decode_console_output(bytes: &[u8]) -> String {
    let has_bom = bytes.starts_with(&[0xFF, 0xFE]);
    let nul_heavy =
        !bytes.is_empty() && bytes.iter().filter(|b| **b == 0).count() * 3 > bytes.len();
    if has_bom || nul_heavy {
        let body = if has_bom { &bytes[2..] } else { bytes };
        let units: Vec<u16> = body
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        return String::from_utf16_lossy(&units);
    }
    String::from_utf8_lossy(bytes).into_owned()
}

/// The registered task definition, or `None` when no task is registered.
async fn read_registration() -> Option<String> {
    let out = tokio::process::Command::new("schtasks")
        .args(["/Query", "/TN", TASK_NAME, "/XML"])
        .no_window()
        .output()
        .await
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(decode_console_output(&out.stdout))
}

/// A task definition that exists but will never fire — `<Enabled>false</Enabled>`.
///
/// Treated as "nothing usable is registered" by [`ensure_registered`], which is
/// both accurate and the thing that makes the Settings switch reversible:
/// [`unregister`] disables the task rather than deleting it (see its docs for
/// why), so without this a user who turned autostart off and then back on again
/// would get `already_correct` — the task is there, the path matches, the
/// triggers are there — and a permanently disabled task. Silently.
///
/// It also self-heals a task someone disabled by hand in Task Scheduler, which
/// is consistent with the rest of this module: `settings.json` is the source of
/// truth for whether autostart is wanted, and OS-level drift gets repaired.
fn is_disabled(xml: &str) -> bool {
    xml.contains("<Enabled>false</Enabled>")
}

/// Verify and, if needed, re-register the task. See
/// [`crate::autostart::ensure_registered`] for the contract.
pub(crate) async fn ensure_registered() -> RegistrationAction {
    let Some(exe) = current_exe() else {
        tracing::warn!("autostart: could not resolve our own executable path");
        return RegistrationAction::Failed;
    };

    // A disabled task counts as absent — see `is_disabled` for why that is what
    // makes the Settings switch reversible rather than one-way.
    let existing = read_registration().await.filter(|xml| !is_disabled(xml));
    let action = super::decide(
        super::disabled_by_user(),
        crate::sys::running_from_stable_location(),
        existing.as_deref(),
        &exe.to_string_lossy(),
        MORNING_TRIGGER_MARKER,
    );
    // ORDER MATTERS HERE, and getting it wrong costs an upgraded user their
    // autostart entirely.
    //
    // `migrate_off_plugin` deletes the plugin-era `HKCU\…\Run` value — which on
    // an install upgrading across this change is the WORKING autostart. It used
    // to run first, unconditionally, before anything knew whether a replacement
    // registration would succeed. Two paths then ended with the user having
    // none at all: an early return on a non-repair verdict, and all three
    // registration attempts failing on a locked-down machine.
    //
    // So the legacy value is now dropped only once something is confirmed to
    // have taken over from it:
    //
    // - the user turned autostart OFF — then removing it is the point, because
    //   the stale value would otherwise keep starting the tray and the setting
    //   would read as ignored;
    // - a correct task is already registered, so the value is redundant;
    // - we just registered one successfully.
    //
    // On every other path it is deliberately left in place. A duplicate launcher
    // is a far smaller problem than no launcher, and the next launch retries.
    if !action.is_repair() {
        if may_drop_legacy(action, false) {
            migrate_off_plugin().await;
        }
        return action;
    }

    let registered = match register_from_xml(&exe).await {
        Ok(()) => {
            // A previous run may have fallen back when policy was stricter than
            // it is now; clear it so logon does not start two trays.
            let _ = remove_startup_folder_launcher().await;
            tracing::info!(
                task = TASK_NAME,
                action = action.as_str(),
                "autostart: scheduled task registered with logon and morning triggers"
            );
            true
        }
        Err(e) => {
            tracing::warn!(
                error = %e,
                "autostart: XML task registration failed - falling back to a logon-only task (no morning relaunch)"
            );
            fallback_register(&exe).await
        }
    };

    if may_drop_legacy(action, registered) {
        migrate_off_plugin().await;
    } else {
        tracing::warn!(
            "autostart: could not register by any means - KEEPING the legacy Run value so an \
             upgraded install is not left with no autostart at all"
        );
    }
    action
}

/// `schtasks /Create /F /XML` — the full-fidelity path.
async fn register_from_xml(exe: &Path) -> Result<(), String> {
    let dir = meridian_core::paths::meridian_dir().ok_or("could not resolve ~/.meridian")?;
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| format!("mkdir {}: {e}", dir.display()))?;
    let xml_path = dir.join(TASK_XML_FILE);
    tokio::fs::write(&xml_path, utf16le_with_bom(&task_xml(exe)))
        .await
        .map_err(|e| format!("write {}: {e}", xml_path.display()))?;

    // `/F` replaces an existing task, which is what makes re-registration
    // idempotent - the counterpart of `bootout` + `bootstrap` on macOS.
    let out = tokio::process::Command::new("schtasks")
        .args(["/Create", "/F", "/TN", TASK_NAME, "/XML"])
        .arg(&xml_path)
        .no_window()
        .output()
        .await
        .map_err(|e| format!("run schtasks: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "schtasks /Create /XML failed: {}",
            decode_console_output(&out.stderr).trim()
        ));
    }
    Ok(())
}

/// Logon-only registration, then the Startup folder if even that is refused.
/// Both lose the morning relaunch; both keep the restart case, which is
/// strictly better than nothing.
///
/// Returns whether ANYTHING will now start the tray at logon. The caller needs
/// that answer, not just a log line: it decides whether the plugin-era `Run`
/// value — the only autostart an upgrading install currently has — may safely be
/// deleted. Reporting success here when nothing was registered is how a
/// locked-down machine would end up with no autostart at all.
async fn fallback_register(exe: &Path) -> bool {
    let out = tokio::process::Command::new("schtasks")
        .args([
            "/Create", "/F", "/TN", TASK_NAME, "/SC", "ONLOGON", "/RL", "LIMITED", "/TR",
        ])
        .arg(format!("\"{}\" {}", exe.display(), super::AUTOSTART_FLAG))
        .no_window()
        .output()
        .await;
    if out.is_ok_and(|o| o.status.success()) {
        let _ = remove_startup_folder_launcher().await;
        tracing::warn!(
            task = TASK_NAME,
            "autostart: registered a logon-only task - the morning relaunch is unavailable on this machine"
        );
        return true;
    }
    match install_startup_folder_launcher(exe).await {
        Ok(()) => {
            tracing::warn!(
                "autostart: Task Scheduler refused - installed a Startup-folder launcher (logon only)"
            );
            true
        }
        Err(e) => {
            tracing::warn!(
                error = %e,
                "autostart: could not register the tray to start at logon by any means"
            );
            false
        }
    }
}

/// `%APPDATA%\Microsoft\Windows\Start Menu\Programs\Startup` — the one
/// "run at logon" location a policy-restricted standard user can still write.
fn startup_folder() -> Result<PathBuf, String> {
    let appdata = std::env::var("APPDATA").map_err(|_| "%APPDATA% is not set".to_string())?;
    Ok(PathBuf::from(appdata).join(r"Microsoft\Windows\Start Menu\Programs\Startup"))
}

/// The hidden-launch VBScript body. `WScript.Shell.Run`'s `0` is the hidden
/// window style, so there is no console flash at logon. Only `"` needs doubling
/// to embed a literal quote in VBScript; the path is quoted so a space anywhere
/// in it cannot split `Run`'s argument.
pub(crate) fn startup_launcher_body(exe: &Path) -> String {
    format!(
        "Set sh = CreateObject(\"WScript.Shell\")\r\n\
         sh.Run \"\"\"{}\"\" {}\", 0, False\r\n",
        exe.display(),
        super::AUTOSTART_FLAG
    )
}

async fn install_startup_folder_launcher(exe: &Path) -> Result<(), String> {
    let dir = startup_folder()?;
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| format!("mkdir {}: {e}", dir.display()))?;
    let script = dir.join(STARTUP_LAUNCHER_NAME);
    tokio::fs::write(&script, startup_launcher_body(exe))
        .await
        .map_err(|e| format!("write {}: {e}", script.display()))
}

/// Remove a previously-installed fallback launcher. A missing file is success.
async fn remove_startup_folder_launcher() -> Result<(), String> {
    let path = startup_folder()?.join(STARTUP_LAUNCHER_NAME);
    match tokio::fs::remove_file(&path).await {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("remove {}: {e}", path.display())),
    }
}

/// Delete the plugin-era `HKCU\...\Run` value.
///
/// `reg delete` exits non-zero when the value is absent, which is the normal
/// case on every launch after the first, so the result is deliberately ignored.
/// `src/uninstall.rs` removes the same value on uninstall; this is the upgrade
/// path.
async fn migrate_off_plugin() {
    let _ = tokio::process::Command::new("reg")
        .args([
            "delete",
            r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run",
            "/V",
            LEGACY_RUN_VALUE,
            "/F",
        ])
        .no_window()
        .output()
        .await;
}

/// Stop the tray starting itself, honouring a user who turned autostart off.
///
/// # `/Change /DISABLE`, not `/Delete`
/// The tray the user is looking at when they untick the setting was started BY
/// this task, so it is the task's running instance — and `/Delete` terminates
/// running instances. Deleting here would quit the app out from under someone
/// who was changing a preference. `/Change /DISABLE` leaves the definition and
/// the running process alone while stopping every future trigger, which is
/// exactly what "don't start yourself" means.
///
/// (`src/uninstall.rs` still uses `/Delete`, correctly: there the app is meant
/// to stop, and leaving a task pointed at a deleted executable is the bug.)
///
/// The Startup-folder fallback is removed too, unconditionally: a machine that
/// fell back under a stricter policy and later got a real task can legitimately
/// have had both, and leaving it behind means the "no" is ignored at next logon.
/// Deleting a script cannot kill a running process, so there is no equivalent
/// hazard.
///
/// `schtasks` exits non-zero when the task is absent, the normal case when
/// autostart was never on, so the result is deliberately not an error.
pub(crate) async fn unregister() {
    let _ = tokio::process::Command::new("schtasks")
        .args(["/Change", "/TN", TASK_NAME, "/DISABLE"])
        .no_window()
        .output()
        .await;
    if let Err(e) = remove_startup_folder_launcher().await {
        tracing::warn!(error = %e, "autostart: could not remove the Startup-folder launcher");
    }
}

/// Live registration state for analytics. See [`super::Status`].
///
/// Reuses [`read_registration`] rather than shelling out again, so the read
/// path and the repair path can never disagree about what "registered" means —
/// and so this stays `async`, because its caller runs on the tray's poll loop
/// and a blocking process spawn there would stall a runtime worker.
pub(crate) async fn status() -> Status {
    super::status_from(
        // Same `is_disabled` filter as the repair path: a task that will never
        // fire must not be reported to the fleet as a healthy registration.
        read_registration()
            .await
            .filter(|xml| !is_disabled(xml))
            .as_deref(),
        current_exe()
            .map(|p| p.to_string_lossy().into_owned())
            .as_deref(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXE: &str = r"C:\Users\tester\AppData\Local\Meridian\Meridian.exe";

    /// Two triggers on ONE task is the entire reason this uses XML: two tasks
    /// would have two instance policies, and the morning one would start a
    /// second tray on top of the running one.
    #[test]
    fn task_carries_logon_and_wake_triggers_on_one_task() {
        let x = task_xml(Path::new(EXE));
        assert!(x.contains("<LogonTrigger>"));
        assert!(x.contains(MORNING_TRIGGER_MARKER));
        assert_eq!(x.matches("<Actions").count(), 1, "one action, one task");
        // Both native wake triggers, and NO clock.
        assert!(x.contains("<StateChange>SessionUnlock</StateChange>"));
        assert!(x.contains("<StateChange>ConsoleConnect</StateChange>"));
        assert!(
            !x.contains("CalendarTrigger"),
            "a hardcoded daily trigger came back - the requirement is an event, not a time"
        );
    }

    /// `IgnoreNew` is what makes the morning trigger a no-op while the tray is
    /// already running. Without it this design produces duplicate trays writing
    /// to one SQLite file.
    #[test]
    fn task_ignores_a_trigger_while_already_running() {
        assert!(task_xml(Path::new(EXE))
            .contains("<MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>"));
    }

    /// Task Scheduler defaults both battery settings to TRUE, which would mean
    /// the tray never starts on battery and is killed when a laptop unplugs -
    /// silently making Meridian desktop-only.
    #[test]
    fn task_overrides_the_battery_defaults() {
        let x = task_xml(Path::new(EXE));
        assert!(x.contains("<DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>"));
        assert!(x.contains("<StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>"));
    }

    /// The default 72-hour execution limit would terminate a long-running tray.
    #[test]
    fn task_has_no_execution_time_limit() {
        assert!(task_xml(Path::new(EXE)).contains("<ExecutionTimeLimit>PT0S</ExecutionTimeLimit>"));
    }

    /// An elevated tray would write files under ~/.meridian that the
    /// un-elevated one could not read back.
    #[test]
    fn task_runs_unelevated() {
        assert!(task_xml(Path::new(EXE)).contains("<RunLevel>LeastPrivilege</RunLevel>"));
    }

    #[test]
    fn task_passes_the_autostart_flag() {
        let x = task_xml(Path::new(EXE));
        assert!(x.contains(&format!(
            "<Arguments>{}</Arguments>",
            super::super::AUTOSTART_FLAG
        )));
        assert!(x.contains(EXE));
    }

    /// A task this module wrote must read back as correct, or every launch
    /// re-registers and reports a repair forever.
    #[test]
    fn a_freshly_written_task_reads_back_as_already_correct() {
        let x = task_xml(Path::new(EXE));
        assert_eq!(
            super::super::decide(false, true, Some(&x), EXE, MORNING_TRIGGER_MARKER),
            RegistrationAction::AlreadyCorrect
        );
    }

    #[test]
    fn xml_is_encoded_as_utf16le_with_a_bom() {
        let bytes = utf16le_with_bom("AB");
        assert_eq!(bytes, vec![0xFF, 0xFE, b'A', 0x00, b'B', 0x00]);
    }

    /// BOM-less UTF-16 stdout is real, and decoding it as UTF-8 interleaves
    /// NULs - which matches no needle, so every query would report "not
    /// registered" and re-register on every launch.
    #[test]
    fn console_output_decodes_utf16_with_and_without_a_bom() {
        let text = "<Task><CalendarTrigger/></Task>";
        assert_eq!(decode_console_output(&utf16le_with_bom(text)), text);

        let bomless: Vec<u8> = text.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
        assert_eq!(decode_console_output(&bomless), text);

        assert_eq!(decode_console_output(text.as_bytes()), text);
        assert_eq!(decode_console_output(&[]), "");
    }

    /// The fallback launcher must quote the path (Windows profiles contain
    /// spaces) and still pass the flag, or a fallback install would look like a
    /// manual launch and pop a window at every logon.
    #[test]
    fn startup_launcher_quotes_the_path_and_passes_the_flag() {
        let body = startup_launcher_body(Path::new(r"C:\Program Files\Meridian\Meridian.exe"));
        assert!(body.contains(r#""""C:\Program Files\Meridian\Meridian.exe"" --autostart""#));
        assert!(body.contains(", 0, False"));
    }

    /// The task this module writes must not read back as disabled, or every
    /// launch would tear it down and rebuild it.
    #[test]
    fn a_freshly_written_task_is_not_disabled() {
        assert!(!is_disabled(&task_xml(Path::new(EXE))));
    }

    /// `unregister` disables rather than deletes (it would otherwise kill the
    /// tray, which IS the task's running instance). This is what stops that
    /// choice making the Settings switch one-way: a disabled task must read as
    /// absent, so turning autostart back on re-creates it enabled.
    #[test]
    fn a_disabled_task_reads_as_absent_so_the_switch_is_reversible() {
        let disabled = task_xml(Path::new(EXE)).replace(
            "<Enabled>true</Enabled>\n    <Hidden>false</Hidden>",
            "<Enabled>false</Enabled>\n    <Hidden>false</Hidden>",
        );
        assert!(is_disabled(&disabled), "the disabled marker must be found");
        // What `ensure_registered` does with it: filter to None, so `decide`
        // reports a fresh registration rather than `already_correct`.
        let existing = Some(disabled).filter(|xml| !is_disabled(xml));
        assert_eq!(
            super::super::decide(
                false,
                true,
                existing.as_deref(),
                EXE,
                MORNING_TRIGGER_MARKER
            ),
            RegistrationAction::RegisteredMissing
        );
    }

    // `the_legacy_registration_survives_until_something_replaces_it` moved to
    // `super::super`'s tests when `may_drop_legacy` became shared - the rule is
    // not Windows-specific, and macOS was the platform breaking it.

    /// The tray's task and launcher names must not collide with the daemon's
    /// (`Meridian Daemon` / `MeridianDaemon.vbs`) - registering over each other
    /// would leave one of the two processes permanently unstarted.
    #[test]
    fn tray_names_do_not_collide_with_the_daemon() {
        assert_ne!(TASK_NAME, "Meridian Daemon");
        assert_ne!(STARTUP_LAUNCHER_NAME, "MeridianDaemon.vbs");
    }
}
