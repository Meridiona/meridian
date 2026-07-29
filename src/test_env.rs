//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Test-only coordination for `MERIDIAN_SETTINGS_PATH`.
//!
//! # Why this is crate-level rather than per-module
//! The variable is PROCESS-global, and `cargo test` runs every test in this
//! crate inside ONE binary on parallel threads. So the scope that has to agree
//! on it is the whole crate - but the coordination used to be per-module: both
//! [`crate::llm::resolver`] and [`crate::telemetry_spool::redact`] declared
//! their own `ENV_LOCK`, each correctly serialising its own tests and neither
//! aware of the other.
//!
//! Two separate locks over one global is the same as no lock. The failure was
//! real, deterministic, and reproducible on `pre-main`:
//!
//! ```text
//! cargo test --lib -- telemetry_spool::redact llm::resolver
//! ---- displayed_pseudonym_matches_shipped ----
//!   left: "mac_970e5ebf236cb0ce"    <- account-scoped
//!  right: "mac_9790585e54e6cdf6"    <- machine-scoped
//! ```
//!
//! A resolver test's `remove_var` lands between two settings reads inside one
//! redact assertion. The var is then UNSET, so the read falls through to the
//! developer's real `~/.meridian/settings.json` - and on a signed-in machine
//! that file carries an `account_pseudonym`, flipping the pseudonym to the
//! account-scoped branch mid-test.
//!
//! Note what that means for verification: the collision needs tests from BOTH
//! modules in the same run, so a filter naming only one (`--lib pseudonym`)
//! reports green, and so does the full suite, on scheduling luck. Silence here
//! is not evidence.
//!
//! # Who calls this
//! [`crate::llm::resolver`]'s and [`crate::telemetry_spool::redact`]'s test
//! modules. Any future test that sets `MERIDIAN_SETTINGS_PATH`, or that calls
//! anything reading settings (`local_host_pseudonym`, `resolve`), must take
//! [`SETTINGS_ENV_LOCK`] through one of these guards - not a lock of its own.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

/// The single lock arbitrating `MERIDIAN_SETTINGS_PATH` for this whole test
/// binary. Mirrors `meridian_core::settings`'s own `ENV_LOCK`, which cannot be
/// reused here because that is a different crate and so a different process
/// image at test time.
///
/// It is in practice the crate's serialisation point for PROCESS-GLOBAL test
/// state generally, not only the env var. `llm::resolver`'s tests take it to
/// serialise the global backoff map as well - its predecessor lock was doing
/// that job silently, and a test excused from the lock on the grounds that it
/// read no settings started failing about 1 run in 15. If a test mutates
/// anything process-wide, take this lock even when settings are not involved.
pub(crate) static SETTINGS_ENV_LOCK: Mutex<()> = Mutex::new(());

/// Acquire [`SETTINGS_ENV_LOCK`], ignoring poisoning.
///
/// A panicking test poisons the mutex, and propagating that would turn one real
/// failure into a cascade of unrelated ones - exactly the burying effect this
/// module exists to stop.
pub(crate) fn lock_settings_env() -> MutexGuard<'static, ()> {
    SETTINGS_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// RAII restore for `MERIDIAN_SETTINGS_PATH`.
///
/// Restores the PREVIOUS value rather than unconditionally removing, so it
/// composes if an outer harness sets its own. Restoring on unwind is the point:
/// a manual `remove_var` at the end of a test is skipped by a failing
/// assertion, leaving a deleted temp path installed for every test that follows.
pub(crate) struct SettingsPathGuard(Option<OsString>);

impl SettingsPathGuard {
    /// Point settings at `path` until this guard drops.
    ///
    /// Callers must already hold [`lock_settings_env`] - this type does not
    /// take the lock itself, because a test typically wants one lock guard
    /// spanning several path changes (see `resolve_rereads_settings_every_call`,
    /// which rewrites the file three times inside one critical section).
    pub(crate) fn set(path: &Path) -> Self {
        let previous = std::env::var_os("MERIDIAN_SETTINGS_PATH");
        std::env::set_var("MERIDIAN_SETTINGS_PATH", path);
        Self(previous)
    }
}

impl Drop for SettingsPathGuard {
    fn drop(&mut self) {
        match self.0.take() {
            Some(previous) => std::env::set_var("MERIDIAN_SETTINGS_PATH", previous),
            None => std::env::remove_var("MERIDIAN_SETTINGS_PATH"),
        }
    }
}

/// A locked, hermetic settings file for the length of a test.
///
/// Bundles the lock, the env guard, and a temp directory, so a test gets
/// isolation by declaring one binding. `contents` is written verbatim, letting
/// a caller choose the branch under test - `"{}"` for machine-scoped
/// pseudonyms, an `account_pseudonym` key for account-scoped.
///
/// Hermetic rather than merely locked, because holding the lock only excludes
/// OTHER tests; it does nothing about the developer's real `settings.json`,
/// which a signed-in machine populates with an `account_pseudonym` and which
/// would otherwise be what an unset variable resolves to.
pub(crate) struct ScopedSettings {
    _env: SettingsPathGuard,
    _lock: MutexGuard<'static, ()>,
    dir: PathBuf,
    path: PathBuf,
}

impl ScopedSettings {
    /// Install a settings file containing `contents`, tagged into a unique
    /// temp dir. Takes the lock; holds it until dropped.
    pub(crate) fn install(tag: &str, contents: &str) -> Self {
        let lock = lock_settings_env();
        let dir = std::env::temp_dir().join(format!(
            "meridian-test-env-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).expect("create temp settings dir");
        let path = dir.join("settings.json");
        std::fs::write(&path, contents).expect("write temp settings.json");
        Self {
            _env: SettingsPathGuard::set(&path),
            _lock: lock,
            dir,
            path,
        }
    }

    /// A settings file with no `account_pseudonym`, i.e. the per-MACHINE
    /// pseudonym branch.
    pub(crate) fn machine_scoped(tag: &str) -> Self {
        Self::install(tag, "{}")
    }

    /// Rewrite the settings file in place, keeping the same lock and path.
    /// For tests asserting that a value is re-read rather than cached.
    pub(crate) fn rewrite(&self, contents: &str) {
        std::fs::write(&self.path, contents).expect("rewrite temp settings.json");
    }

    /// The settings file's path, for tests that assert on its contents.
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for ScopedSettings {
    fn drop(&mut self) {
        // Runs before the fields, so the env var still points at `dir` here -
        // safe, because `_lock` (dropped after this) is still held, so no other
        // test can observe the window.
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
