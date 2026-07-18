//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Install-mode detection and the file paths that depend on it.
//!
//! The tray runs in three install shapes — a bundled `.app`, a source/dev
//! checkout, or a bare launch — and the credential `.env` + `meridian.db` live
//! in different places in each. This module is the single resolver for "where
//! does my data live"; it is NOT a set of Tauri commands (it's plumbing the
//! commands and startup consume).
//!
//! # Who calls this
//! - [`crate::commands::integrations`] — reads tracker keys from [`InstallMode::env_path`].
//! - `lib.rs` startup — opens the DB at [`meridian_db_path`].
//!
//! # Related
//! - [`crate::sys`] — other shared runtime helpers (uid, notify, ui_base).
//! - [`crate::backend_install`] — stages the daemon to `~/.meridian/bin/meridian`
//!   on the DMG path and points its WorkingDirectory at `~/.meridian`, so the
//!   daemon self-loads the canonical `~/.meridian/.env` ([`InstallMode::Canonical`]).
//! - The daemon's env layering differs by install type: DMG → `~/.meridian/.env`,
//!   source → repo `.env`.

/// Which install mode the tray is running in, inferred from the user's `.env` location.
///
/// - `Canonical`: `~/.meridian/.env` exists — user credentials, install-independent.
///   **Release builds only** (see [`detect_install_mode`]).
/// - `Dev`: the checkout's own `.env` — the only credential source a debug build has.
/// - `Bare`: neither present — process-env overrides and hardcoded defaults only.
#[derive(Debug)]
pub(crate) enum InstallMode {
    /// Unreachable in a debug build by design: a dev run never reads the installed
    /// package's `.env`. Kept (rather than `cfg`'d out) so the release and debug
    /// paths share one type, and the doc links above stay resolvable.
    #[cfg_attr(debug_assertions, allow(dead_code))]
    Canonical(std::path::PathBuf),
    Dev(std::path::PathBuf),
    Bare,
}

impl InstallMode {
    pub(crate) fn env_path(&self) -> Option<&std::path::Path> {
        match self {
            Self::Canonical(p) | Self::Dev(p) => Some(p),
            Self::Bare => None,
        }
    }
}

/// Detect the install mode from the file system.
///
/// **A debug build is always `Dev` or `Bare`, never `Canonical`** — it answers
/// from its own checkout and never consults `~/.meridian/.env`, even when one
/// exists. Preferring the canonical file meant a contributor's dev tray read the
/// installed package's tracker credentials instead of the repo `.env` sitting
/// beside the daemon it was actually running, so Jira reads as unconfigured while
/// `JIRA_URL` is right there in the checkout. A fresh clone with no `.env` is
/// `Bare` — nothing configured yet, which is the honest answer and keeps dev
/// independent of the package.
///
/// In a **release** build `~/.meridian/.env` is the canonical credential location
/// for every install type — install-independent, next to `meridian.db` and
/// `settings.json` — with a cwd walk as the fallback.
pub(crate) fn detect_install_mode() -> InstallMode {
    #[cfg(debug_assertions)]
    {
        let p = dev_env(&dev_root());
        if p.exists() {
            InstallMode::Dev(p)
        } else {
            InstallMode::Bare
        }
    }
    #[cfg(not(debug_assertions))]
    {
        let home = std::env::var("HOME").ok().map(std::path::PathBuf::from);
        if let Some(p) = home.as_ref().map(|h| h.join(".meridian/.env")) {
            if p.exists() {
                return InstallMode::Canonical(p);
            }
        }
        if let Ok(mut dir) = std::env::current_dir() {
            for _ in 0..8 {
                let candidate = dir.join(".env");
                if candidate.exists() {
                    return InstallMode::Dev(candidate);
                }
                if !dir.pop() {
                    break;
                }
            }
        }
        InstallMode::Bare
    }
}

/// The credential `.env` **write** target when none exists yet — a fresh
/// `.app`/DMG install (nothing pre-creates it) or a fresh clone. Saving
/// credentials must work before any `.env` exists, so this defaults rather than
/// failing in [`InstallMode::Bare`] (whose [`InstallMode::env_path`] is `None`).
///
/// A **debug** build writes to its checkout's `.env`, matching what
/// [`detect_install_mode`] reads back. Writing to `~/.meridian/.env` from a dev
/// tray would strand the credential: saved to a file the repo daemon never loads
/// and the next read never looks at, so the UI would still show it unconfigured.
///
/// `None` only when `$HOME` is unset (release path only — a debug build always
/// knows its own checkout).
pub(crate) fn canonical_env_path() -> Option<std::path::PathBuf> {
    #[cfg(debug_assertions)]
    {
        Some(dev_env(&dev_root()))
    }
    #[cfg(not(debug_assertions))]
    {
        std::env::var("HOME")
            .ok()
            .map(|h| std::path::PathBuf::from(h).join(".meridian/.env"))
    }
}

/// The checkout this tray was built from, baked in at compile time.
///
/// **In a debug build the checkout is the only source of truth.** A dev run must
/// depend on nothing in `~/.meridian` — not the CLI, not the `.env`, not the
/// tracker credentials — because a machine that also has Meridian installed
/// otherwise mixes the two: the tray reads the *package's* credentials, spawns
/// the *package's* (older) CLI, and saves new credentials to a file the repo
/// daemon never loads. `~/.meridian/meridian.db` is the single deliberate
/// exception (see [`meridian_db_path`]) — dev and installed share one database.
///
/// Every use is gated on `debug_assertions`, so a shipped `.app` can never be
/// redirected at a checkout that happens to be on disk.
#[cfg(debug_assertions)]
const REPO_ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");

/// The checkout root, canonicalized so children get a real path rather than one
/// with `/../..` in it.
#[cfg(debug_assertions)]
fn dev_root() -> std::path::PathBuf {
    let p = std::path::PathBuf::from(REPO_ROOT);
    p.canonicalize().unwrap_or(p)
}

/// The CLI built from `repo` — returned **unconditionally, existence unchecked**.
///
/// A missing `target/debug/meridian` must fail loudly (spawn: "No such file or
/// directory", with the path right there in the log), never fall back to the
/// installed binary. That fallback is what hid this bug: the tray silently drove
/// the packaged CLI against a DB the dev daemon had already migrated ahead.
/// `dev-start.sh` builds this within seconds of launch anyway.
#[cfg(debug_assertions)]
fn dev_bin(repo: &std::path::Path) -> std::path::PathBuf {
    repo.join("target/debug/meridian")
}

/// The checkout's `.env` — the dev credential file, whether or not it exists yet.
/// A fresh clone has none, and that must read as "nothing configured"
/// ([`InstallMode::Bare`]), never as licence to borrow the package's credentials.
#[cfg(debug_assertions)]
fn dev_env(repo: &std::path::Path) -> std::path::PathBuf {
    repo.join(".env")
}

/// Resolve the `meridian` CLI binary, **dev build first, then native** — the
/// shared resolver for every command that shells out to it (`tasks-sync`,
/// `ticket-update`, `ticket-parents`, …).
///
/// **`MERIDIAN_BIN` overrides everything, in either profile** — the one explicit
/// opt-in, mirroring `meridian_db_path()`. It lets a `tauri dev` tray spawn a
/// workspace build the debug rule below wouldn't pick (e.g. `target/release/meridian`,
/// worth it for a long LLM-experiment run) without restaging `~/.meridian/bin` —
/// which would also point the REAL tray/daemon at the branch build (see the
/// overlapping-installs migration trap). It is deliberate, per-process, and named
/// in the log, so it is not the silent fallback the debug rule exists to prevent;
/// a value that doesn't exist is ignored with a warning rather than honoured.
///
/// **Debug: otherwise the checkout's own `target/debug/meridian`, full stop** — no
/// probing, no fallback to the installed binary. `dev-start.sh` stops the installed
/// *daemon* but cannot stop a shell-out, so a fallback here meant a dev tray
/// silently drove the packaged CLI against a DB its own daemon had already
/// migrated ahead; that CLI refuses to open it ("migration N was previously
/// applied but is missing in the resolved migrations") and the tray surfaced only
/// a bare `status=Some(1)`. Failing loudly on a not-yet-built checkout beats
/// succeeding against the wrong binary.
///
/// **Release**: mirrors the Node `selectMeridianBinary(meridianCandidates())`.
/// The native binary (`~/.meridian/bin/meridian`, staged by `backend_install`) has
/// NO runtime deps, so it works under launchd's minimal PATH; the user-local
/// `~/.local/bin/meridian` is a `#!/usr/bin/env node` wrapper that dies when
/// launchd's PATH lacks `node`, so it's only the fallback; bare `meridian`
/// (relies on `$PATH`) is the last resort.
pub(crate) fn meridian_bin() -> String {
    // Checked in BOTH profiles, before anything else: an explicit, logged opt-in
    // beats every rule below. Ignored (with a warning) when it points at nothing,
    // so a stale export degrades to the normal resolution rather than a spawn
    // failure.
    if let Ok(p) = std::env::var("MERIDIAN_BIN") {
        if std::path::Path::new(&p).exists() {
            tracing::info!(source = "process_env", bin = %p, "meridian bin resolved");
            return p;
        }
        tracing::warn!(bin = %p, "MERIDIAN_BIN set but missing - falling back to the default binary");
    }
    #[cfg(debug_assertions)]
    {
        dev_bin(&dev_root()).to_string_lossy().into_owned()
    }
    #[cfg(not(debug_assertions))]
    {
        if let Ok(home) = std::env::var("HOME") {
            // `~/.meridian/bin/meridian` is the DMG path (staged by `backend_install`) —
            // native, no runtime deps, so it works under launchd's minimal PATH; the
            // `~/.local/bin` node wrapper is the last resort.
            for rel in ["/.meridian/bin/meridian", "/.local/bin/meridian"] {
                let p = std::path::PathBuf::from(format!("{home}{rel}"));
                if p.exists() {
                    return p.to_string_lossy().into_owned();
                }
            }
        }
        "meridian".to_string()
    }
}

/// The working directory to spawn the `meridian` CLI from — the companion to
/// [`meridian_bin`], and it must be resolved with it, never independently.
///
/// The CLI loads its env with `dotenvy`, which walks UP from its cwd and stops at
/// the first `.env`. So the cwd chooses the credentials, and it must choose the
/// same file [`detect_install_mode`] reads:
/// - **Debug**: the checkout — unconditionally, `.env` or not. A fresh clone then
///   finds none and the CLI reports the provider unconfigured, which is true.
///   Reaching into `~/.meridian` to make it "work" would silently run a dev tray
///   on the installed package's credentials.
/// - **Release**: `~/.meridian`, the canonical `.env` the tray writes tracker
///   creds to. Every install type converges here (see the daemon config gotcha in
///   CLAUDE.md).
pub(crate) fn cli_cwd() -> Result<std::path::PathBuf, String> {
    #[cfg(debug_assertions)]
    {
        Ok(dev_root())
    }
    #[cfg(not(debug_assertions))]
    {
        let home = std::env::var("HOME")
            .map_err(|_| "HOME env var not set - cannot locate ~/.meridian".to_string())?;
        let cwd = std::path::PathBuf::from(&home).join(".meridian");
        if !cwd.exists() {
            std::fs::create_dir_all(&cwd)
                .map_err(|e| format!("could not create ~/.meridian: {e}"))?;
        }
        Ok(cwd)
    }
}

/// Read `key` from a single line of a .env file, stripping surrounding quotes.
fn dotenv_line_value(line: &str, key: &str) -> Option<String> {
    let prefix = format!("{key}=");
    let t = line.trim();
    if t.starts_with('#') || !t.starts_with(prefix.as_str()) {
        return None;
    }
    let raw = t[prefix.len()..].trim();
    let v = raw.trim_matches('"').trim_matches('\'').trim();
    if v.is_empty() {
        None
    } else {
        Some(v.to_string())
    }
}

/// Read a single `key=value` line out of a `.env` file — the tray's targeted
/// alternative to the daemon's whole-file `dotenvy::dotenv_override()` (the
/// tray does NOT auto-load env into its own process; see the crate-level
/// gotcha in CLAUDE.md). Used wherever a command needs one credential from
/// [`InstallMode::env_path`] — e.g. `commands::integrations`'s tracker keys,
/// `commands::account::clerk_publishable_key`.
pub(crate) fn env_key_from_path(path: &std::path::Path, key: &str) -> Option<String> {
    let contents = std::fs::read_to_string(path).ok()?;
    contents.lines().find_map(|l| dotenv_line_value(l, key))
}

/// Resolve meridian.db path: process env first (launchd plist / shell export),
/// then the daemon's .env (keyed by install mode), then the hardcoded default.
/// Logs at `info!` so the install mode is visible in OpenObserve on every startup.
pub(crate) fn meridian_db_path() -> String {
    if let Ok(p) = std::env::var("MERIDIAN_DB") {
        tracing::info!(source = "process_env", path = %p, "meridian_db resolved");
        return p;
    }
    let mode = detect_install_mode();
    if let Some(env_file) = mode.env_path() {
        if let Some(p) = env_key_from_path(env_file, "MERIDIAN_DB") {
            tracing::info!(
                source = ?mode,
                env_file = %env_file.display(),
                path = %p,
                "meridian_db resolved"
            );
            return p;
        }
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let p = format!("{}/.meridian/meridian.db", home);
    tracing::info!(source = ?mode, path = %p, "meridian_db resolved (default)");
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The rule itself, asserted end-to-end: these tests ARE a debug build, so
    /// every resolver must answer from the checkout and name `~/.meridian` for
    /// nothing. The DB is the one sanctioned exception, pinned separately below.
    #[test]
    fn a_dev_build_reaches_into_the_installed_package_for_nothing() {
        let repo = dev_root();
        let installed = "/.meridian/";

        let bin = meridian_bin();
        assert!(
            bin.starts_with(&*repo.to_string_lossy()) && bin.ends_with("target/debug/meridian"),
            "dev must spawn the CLI it was built beside, got: {bin}"
        );
        assert!(
            !bin.contains(installed),
            "dev must not spawn the package CLI"
        );

        let cwd = cli_cwd().expect("the dev cwd is the checkout and cannot fail");
        assert_eq!(
            cwd, repo,
            "the cwd picks the .env - it must be the checkout"
        );
        assert!(!cwd.to_string_lossy().contains(installed));
        assert!(
            !cwd.to_string_lossy().contains(".."),
            "hand the child a real path, not one with /../.. in it"
        );

        let write_to = canonical_env_path().expect("dev always knows its own checkout");
        assert_eq!(write_to, repo.join(".env"), "creds must save into the repo");

        // The read-back must agree with the write target, or a saved credential is
        // stranded: written to one file, looked for in another.
        if let Some(read_from) = detect_install_mode().env_path() {
            assert_eq!(read_from, write_to, "dev reads the file dev writes");
        }
    }

    /// `~/.meridian/meridian.db` is the single sanctioned link between a dev run
    /// and the installed package - they share one database. Pinned so the
    /// independence rule above never gets over-applied to the DB as well.
    #[test]
    fn the_database_is_the_one_thing_a_dev_run_still_shares() {
        // Only meaningful with no override; MERIDIAN_DB wins by design.
        if std::env::var("MERIDIAN_DB").is_ok() {
            return;
        }
        assert!(
            meridian_db_path().ends_with("/.meridian/meridian.db"),
            "the shared DB stays in ~/.meridian"
        );
    }

    /// An unbuilt checkout still resolves to its own path, so the spawn fails
    /// naming that path. The old existence-check fell back to the installed CLI
    /// instead, turning this into a silent success against the wrong binary.
    #[test]
    fn an_unbuilt_checkout_names_itself_rather_than_falling_back() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();
        assert_eq!(dev_bin(repo), repo.join("target/debug/meridian"));
        assert!(!dev_bin(repo).exists(), "nothing built - must not pretend");
    }

    /// A fresh clone has no `.env`, and that must read as "nothing configured
    /// yet", never as licence to borrow the installed package's credentials.
    #[test]
    fn a_fresh_clone_is_bare_not_borrowed() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();
        assert_eq!(dev_env(repo), repo.join(".env"));
        assert!(!dev_env(repo).exists(), "a fresh clone carries no creds");
    }
}
