//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Install-mode / capture-kill-switch gating for the observability pipeline.
//!
//! Split out of `observability/mod.rs` (which was over the repo's 500-line
//! cap) since these two checks are a self-contained concern: "is telemetry
//! allowed to leave this machine" and "should we capture at all", both
//! read-only env/filesystem probes with no dependency on the rest of the
//! subscriber-building logic in the parent module.
//!
//! # Who calls this
//! `observability::mod` — `is_canonical_install()` gates
//! `resolve_otlp_target`/`resolve_otlp_endpoint`/`is_otlp_configured`;
//! `capture_disabled()` gates `try_build_otel_providers`.
//!
//! # Related
//! - `tray/src-tauri/src/update.rs`'s `is_packaged()` — the same
//!   executable-path pattern, used independently by the tray (no crate
//!   dependency between the two).

/// True when THIS PROCESS is a canonical packaged install — its own
/// executable lives at the DMG installer's staged location
/// (`~/.meridian/bin/meridian`; see
/// `tray/src-tauri/src/backend_install.rs`). A packaged install must never
/// attempt live delivery to OpenObserve — telemetry capture stays fully
/// local, and the only path to a developer's OpenObserve is a user-initiated
/// export bundle imported by hand. A Dev/Bare checkout (running from
/// `target/debug` or `target/release`) may still ship live if `otlp_enabled`
/// and credentials are configured, for engineers debugging against their own
/// instance.
///
/// Deliberately checks the running executable's OWN path rather than "does
/// `~/.meridian/.env` exist anywhere on this machine" — that file is a
/// per-`$HOME` marker written by every install type that has EVER run there,
/// not a per-process one. An engineer who has ever installed the packaged
/// app on their own dev Mac (normal, e.g. for dogfooding) would otherwise
/// have shipping silently and permanently disabled for every `cargo run`
/// from source on that machine, with no override. Mirrors the same
/// executable-path check `tray/src-tauri/src/update.rs`'s `is_packaged()`
/// uses (independently — the daemon has no dependency on the tray crate).
/// Falls back to the old marker-file check only if `current_exe()` itself
/// fails (should not happen in practice).
pub fn is_canonical_install() -> bool {
    let Some(meridian_dir) = meridian_core::paths::meridian_dir() else {
        return false;
    };
    match std::env::current_exe() {
        Ok(exe) => Some(exe) == staged_daemon_path(),
        // `current_exe()` failing is rare (permissions, exotic sandboxing) —
        // fall back to the machine-wide marker file rather than guessing.
        Err(_) => meridian_dir.join(".env").exists(),
    }
}

/// Absolute path the tray stages the daemon binary at
/// (`~/.meridian/bin/meridian{EXE_SUFFIX}`).
///
/// The single source of this path. [`is_canonical_install`] compares
/// `current_exe()` against it, and `health::platform::daemon_binary_candidates`
/// probes it — those two MUST agree, and before this existed they were assembled
/// independently, from different roots (`paths::meridian_dir()` vs
/// `paths::home_dir_or_cwd()`). Drift between them means either a false CRITICAL
/// in doctor or silently-inert error reporting, which is exactly the failure
/// [`staged_daemon_file_name`] already documents having shipped once.
///
/// `None` only when the home directory cannot be resolved.
pub fn staged_daemon_path() -> Option<std::path::PathBuf> {
    meridian_core::paths::meridian_dir().map(|d| d.join("bin").join(staged_daemon_file_name()))
}

/// File name the tray stages the daemon under in `~/.meridian/bin/`.
///
/// **Must** track `backend_install::DAEMON_FILE` in the tray crate, which is
/// `meridian.exe` on Windows and `meridian` elsewhere. This was hardcoded to
/// `"meridian"`, so on Windows the comparison above could never match a real
/// packaged install: `current_exe()` ends in `.exe`, so `is_canonical_install()`
/// returned false, `resolve_otlp_target()` fell through to the DEV branch, and
/// that branch requires `otlp_enabled` + local OpenObserve credentials which a
/// packaged install never has. Net effect: central error reporting was silently
/// inert on every Windows install, with no warning on either side.
///
/// Derived from `EXE_SUFFIX` rather than a `cfg`-selected constant so it cannot
/// drift again if another platform is added.
fn staged_daemon_file_name() -> String {
    format!("meridian{}", std::env::consts::EXE_SUFFIX)
}

/// Hard kill switch for OTel capture (spans/logs to the local spool), read from
/// `MERIDIAN_TELEMETRY_DISABLED`.
pub(super) fn capture_disabled() -> bool {
    std::env::var("MERIDIAN_TELEMETRY_DISABLED")
        .is_ok_and(|v| v == "1" || v.eq_ignore_ascii_case("true"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The real cross-crate pin.
    ///
    /// `staged_daemon_file_name` is a hand-maintained mirror of the tray's
    /// `backend_install::DAEMON_FILE`, and the daemon crate cannot depend on the
    /// tray crate to compare them. Every test that recomputes the name from
    /// `EXE_SUFFIX` therefore drifts along with the production copy and stays
    /// green — which is worthless against the one failure this mirror has
    /// already suffered (hardcoded `"meridian"`, so Windows packaged installs
    /// silently shipped no telemetry at all).
    ///
    /// So read the tray's source and assert on the constant itself. Source
    /// scanning is the repo's sanctioned tactic where a unit test cannot reach
    /// (the tray cfg audit, the UI's `no-native-dialogs` test).
    #[test]
    fn staged_daemon_file_name_matches_the_trays_daemon_file_constant() {
        const TRAY_SRC: &str = include_str!("../../tray/src-tauri/src/backend_install.rs");

        // Both cfg arms, so this fails on whichever platform the suite runs.
        let declared: Vec<&str> = TRAY_SRC
            .lines()
            .filter_map(|l| {
                l.trim()
                    .strip_prefix("pub(crate) const DAEMON_FILE: &str = ")
            })
            .map(|v| v.trim().trim_end_matches(';').trim_matches('"'))
            .collect();

        assert_eq!(
            declared.len(),
            2,
            "expected both cfg arms of DAEMON_FILE in the tray source; found {declared:?} \
             — if the tray changed shape, update this scan rather than deleting it"
        );
        assert!(
            declared.contains(&"meridian") && declared.contains(&"meridian.exe"),
            "the tray's DAEMON_FILE values changed to {declared:?}; \
             staged_daemon_file_name() must be updated to match"
        );
        assert!(
            declared.contains(&staged_daemon_file_name().as_str()),
            "staged_daemon_file_name() produced {:?}, which is not one of the tray's \
             DAEMON_FILE values {declared:?} — central error reporting and doctor's \
             daemon-binary probe both break silently when these drift",
            staged_daemon_file_name()
        );
    }

    /// Complements the cross-crate pin above: that one proves the name matches
    /// one of the tray's two constants, this one proves it is the RIGHT one for
    /// the platform the suite is running on.
    #[test]
    fn staged_daemon_file_name_carries_the_platform_exe_suffix() {
        let name = staged_daemon_file_name();
        assert!(name.starts_with("meridian"), "unexpected stem: {name}");
        assert!(
            name.ends_with(std::env::consts::EXE_SUFFIX),
            "{name} lacks this platform's executable suffix"
        );
        if cfg!(target_os = "windows") {
            assert_eq!(name, "meridian.exe");
        } else {
            assert_eq!(name, "meridian");
        }
    }
}
