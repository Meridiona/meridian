//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! "Move to Applications" self-relocation for a DMG-mount or App Translocation
//! launch. Without this, a user who double-clicks the app straight out of the
//! mounted disk image (instead of dragging it to `/Applications` first) gets
//! a tray that works for the current session but can never re-open itself
//! after a reboot — [`crate::autostart::ensure_enabled_once`] correctly
//! refuses to pin a login item to a path that's about to vanish, and just
//! keeps deferring forever because nothing ever moves the app out of the way.
//!
//! This module is the active fix: on a transient launch it prompts the user,
//! copies the bundle to `/Applications`, relaunches from there, and exits the
//! transient process — so relocation plus autostart registration both
//! complete on the very first launch instead of requiring the user to know
//! to drag-then-reopen.
//!
//! # Who calls this
//! [`crate::run`]'s `setup()` hook — first thing, before the DB pool opens or
//! the poll loop spawns, so a relocate-and-exit never leaves half-started
//! state behind (bundled runs only, see [`crate::sys::is_bundled`]).
//!
//! # Related
//! - [`crate::sys::running_from_stable_location`] — the shared transient-path
//!   detector both this module and [`crate::autostart`] key off.
//! - [`crate::autostart`] — registers the login item; only reachable once a
//!   launch is no longer transient, whether that's because of this module or
//!   because the user manually moved the app themselves.

use std::path::{Path, PathBuf};

/// Entry point. Returns `true` only when the app was successfully relaunched
/// from `/Applications` — the caller MUST treat that as "a replacement
/// process now exists; exit immediately" rather than continuing startup.
/// Every other outcome (stable launch, declined prompt, copy/relaunch
/// failure) returns `false` and logs why; normal startup should continue,
/// and [`crate::autostart::ensure_enabled_once`]'s existing deferral already
/// handles a still-transient launch safely on the next run.
#[cfg(target_os = "macos")]
pub fn maybe_relocate_to_applications() -> bool {
    if crate::sys::running_from_stable_location() {
        return false;
    }
    let Ok(exe) = std::env::current_exe() else {
        tracing::warn!("relocate: could not resolve current_exe, skipping");
        return false;
    };
    let Some(bundle) = bundle_root(&exe) else {
        tracing::warn!(
            exe = %exe.display(),
            "relocate: transient launch but couldn't resolve a .app bundle root, skipping"
        );
        return false;
    };

    if !prompt_move_to_applications() {
        tracing::info!("relocate: user declined the move-to-Applications prompt");
        return false;
    }

    let dest = match copy_bundle_to_applications(&bundle) {
        Ok(dest) => dest,
        Err(e) => {
            tracing::warn!(error = %e, "relocate: copy to /Applications failed");
            return false;
        }
    };

    let Some(exe_name) = exe.file_name() else {
        tracing::warn!("relocate: running exe has no file name, skipping relaunch");
        return false;
    };
    let new_exe = dest.join("Contents/MacOS").join(exe_name);
    match std::process::Command::new(&new_exe).spawn() {
        Ok(_) => {
            tracing::info!(
                dest = %dest.display(),
                "relocate: moved to Applications and relaunched — exiting transient instance"
            );
            true
        }
        Err(e) => {
            tracing::warn!(
                error = %e,
                new_exe = %new_exe.display(),
                "relocate: copied to /Applications but failed to relaunch from there"
            );
            false
        }
    }
}

#[cfg(not(target_os = "macos"))]
pub fn maybe_relocate_to_applications() -> bool {
    false
}

/// The bundle root (`.../Foo.app`) containing `exe`, if `exe` sits in the
/// standard `Contents/MacOS/` layout. Split out as a pure function so the
/// path logic is unit-testable without a real `current_exe()`.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn bundle_root(exe: &Path) -> Option<PathBuf> {
    let macos = exe.parent()?;
    let contents = macos.parent()?;
    let bundle = contents.parent()?;
    (macos.ends_with("MacOS")
        && contents.ends_with("Contents")
        && bundle.extension().is_some_and(|e| e == "app"))
    .then(|| bundle.to_path_buf())
}

/// Native "Move to Applications Folder?" alert via `osascript` — the same
/// shell-out pattern already used for AppleScript in
/// `capture::drm_detector::run_osascript`. Returns `true` only on the
/// explicit "Move to Applications" button; a dismissed dialog, "Not Now", or
/// any `osascript` failure (e.g. running headless in CI) all count as "no".
#[cfg(target_os = "macos")]
fn prompt_move_to_applications() -> bool {
    let script = r#"display dialog "Meridian needs to be in your Applications folder to start automatically when you log in. Move it there now?" with title "Move Meridian to Applications?" buttons {"Not Now", "Move to Applications"} default button "Move to Applications""#;
    std::process::Command::new("osascript")
        .arg("-e")
        .arg(script)
        .output()
        .is_ok_and(|o| {
            o.status.success()
                && String::from_utf8_lossy(&o.stdout).contains("Move to Applications")
        })
}

/// Copy `src` (the `.app` bundle) into `/Applications`, replacing any
/// existing copy there, and return the destination path.
///
/// Uses `ditto` — the macOS tool for copying bundles with resource
/// forks/extended attributes intact, which a plain recursive copy can
/// silently drop. Tries a plain (non-privileged) copy first since
/// `/Applications` is group-writable for admin accounts by default; only
/// falls back to an admin-privileged shell command if that fails (e.g. a
/// standard, non-admin account).
#[cfg(target_os = "macos")]
fn copy_bundle_to_applications(src: &Path) -> Result<PathBuf, String> {
    let name = src
        .file_name()
        .ok_or_else(|| "bundle path has no file name".to_string())?;
    let dest = Path::new("/Applications").join(name);

    if run_ditto(src, &dest).is_ok_and(|o| o.status.success()) {
        return Ok(dest);
    }
    match run_ditto_privileged(src, &dest) {
        Ok(o) if o.status.success() => Ok(dest),
        Ok(o) => Err(format!(
            "ditto (admin) failed: {}",
            String::from_utf8_lossy(&o.stderr)
        )),
        Err(e) => Err(format!("ditto (admin) spawn failed: {e}")),
    }
}

#[cfg(target_os = "macos")]
fn run_ditto(src: &Path, dest: &Path) -> std::io::Result<std::process::Output> {
    if dest.exists() {
        std::fs::remove_dir_all(dest)?;
    }
    std::process::Command::new("ditto")
        .arg(src)
        .arg(dest)
        .output()
}

/// Runs `rm -rf <dest>; ditto <src> <dest>` as administrator via
/// `osascript … with administrator privileges`. The shell command is written
/// to a temp script and invoked by path rather than inlined into the
/// AppleScript source, so path contents never need nested shell/AppleScript
/// quote-escaping.
#[cfg(target_os = "macos")]
fn run_ditto_privileged(src: &Path, dest: &Path) -> std::io::Result<std::process::Output> {
    use std::os::unix::fs::PermissionsExt;

    let script_path =
        std::env::temp_dir().join(format!("meridian-relocate-{}.sh", std::process::id()));
    let shell_cmd = format!(
        "rm -rf {dest} && ditto {src} {dest}",
        src = shell_quote(src),
        dest = shell_quote(dest)
    );
    std::fs::write(&script_path, format!("#!/bin/sh\nset -e\n{shell_cmd}\n"))?;
    std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o700))?;

    let quoted_script = script_path
        .display()
        .to_string()
        .replace('\\', "\\\\")
        .replace('"', "\\\"");
    let out = std::process::Command::new("osascript")
        .arg("-e")
        .arg(format!(
            "do shell script \"{quoted_script}\" with administrator privileges"
        ))
        .output();
    let _ = std::fs::remove_file(&script_path);
    out
}

/// Single-quote a path for embedding in a `/bin/sh` command, escaping any
/// literal single quotes it contains.
#[cfg(target_os = "macos")]
fn shell_quote(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', r"'\''"))
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::bundle_root;
    use std::path::Path;

    #[test]
    fn resolves_bundle_root_from_dmg_mount() {
        assert_eq!(
            bundle_root(Path::new(
                "/Volumes/Meridian/Meridian.app/Contents/MacOS/Meridian"
            )),
            Some(std::path::PathBuf::from("/Volumes/Meridian/Meridian.app"))
        );
    }

    #[test]
    fn resolves_bundle_root_from_translocation() {
        assert_eq!(
            bundle_root(Path::new(
                "/private/var/folders/ab/xyz/T/AppTranslocation/ABC-123/d/Meridian.app/Contents/MacOS/Meridian"
            )),
            Some(std::path::PathBuf::from(
                "/private/var/folders/ab/xyz/T/AppTranslocation/ABC-123/d/Meridian.app"
            ))
        );
    }

    #[test]
    fn rejects_non_bundle_layout() {
        assert_eq!(bundle_root(Path::new("/usr/local/bin/meridian-tray")), None);
    }
}
