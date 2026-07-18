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
pub(super) fn is_canonical_install() -> bool {
    match std::env::current_exe() {
        Ok(exe) => std::env::var("HOME").is_ok_and(|home| {
            let home = std::path::Path::new(&home);
            exe == home.join(".meridian/bin/meridian")
        }),
        // `current_exe()` failing is rare (permissions, exotic sandboxing) —
        // fall back to the machine-wide marker file rather than guessing.
        Err(_) => std::env::var("HOME")
            .map(|home| std::path::Path::new(&home).join(".meridian/.env").exists())
            .unwrap_or(false),
    }
}

/// Hard kill switch for OTel capture (spans/logs to the local spool), read from
/// `MERIDIAN_TELEMETRY_DISABLED`.
pub(super) fn capture_disabled() -> bool {
    std::env::var("MERIDIAN_TELEMETRY_DISABLED")
        .is_ok_and(|v| v == "1" || v.eq_ignore_ascii_case("true"))
}
