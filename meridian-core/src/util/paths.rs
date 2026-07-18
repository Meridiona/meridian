//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! The single source of truth for "where is the user's home directory" and for
//! expanding a leading `~/` in a configured path.
//!
//! Before this module there were **two** hand-rolled `expand_tilde`
//! implementations — one in the daemon (`meridian::config`) and one in
//! `meridian_core::settings` — both reading `$HOME` directly and both
//! formatting with a hardcoded `/` separator. That is correct on macOS and
//! wrong on Windows, where `$HOME` is typically unset and `\` is the
//! separator. Consolidating here means a path fix lands once, for every
//! caller, instead of being applied to one copy and silently missed on the
//! other.
//!
//! # Resolution order
//!
//! `$HOME` is consulted **first on every platform**, then the OS's own notion
//! of the profile directory. That ordering is deliberate:
//!
//! - On macOS/Linux it reproduces the previous behaviour exactly — `$HOME` is
//!   always set for a logged-in user, so this module returns precisely what the
//!   old `std::env::var("HOME")` calls returned. No behaviour change.
//! - The existing tests set `$HOME` to a temp dir to isolate themselves
//!   (see `meridian::config`'s tests); honouring it keeps that working.
//! - On Windows `$HOME` is normally unset, so the fallback
//!   (`%USERPROFILE%`, via the known-folder API) supplies the right answer —
//!   while still respecting `$HOME` when a Git-Bash/MSYS shell has set it, which
//!   is what git and ssh themselves do on Windows.
//!
//! # Who calls this
//! [`crate::settings`] (settings.json resolution) and the daemon's
//! `meridian::config` (`MERIDIAN_DB` and friends), plus anything else that
//! needs to resolve a `~/`-prefixed configured path.

use std::path::{Path, PathBuf};

/// The current user's home directory, or `None` when it cannot be determined.
///
/// Prefers `$HOME` (see the module docs for why), falling back to the
/// platform's own profile-directory lookup. An empty `$HOME` is treated as
/// unset rather than as the relative path `""`.
pub fn home_dir() -> Option<PathBuf> {
    if let Some(home) = std::env::var_os("HOME").filter(|h| !h.is_empty()) {
        return Some(PathBuf::from(home));
    }
    dirs::home_dir()
}

/// Expand a leading `~/` to the user's home directory.
///
/// `~/` is the only tilde form accepted (no `~user/`) — this mirrors the
/// dashboard's `ui/lib/settings.ts`, which the daemon must agree with or an
/// "Apply" from Settings never reaches it. A path with no `~/` prefix, or a
/// `~/` path that cannot be resolved because the home directory is unknown, is
/// returned unchanged.
///
/// Joining via [`Path::join`] rather than `format!("{home}/{rest}")` is what
/// makes this correct on Windows: the separator comes from the platform
/// instead of being hardcoded.
pub fn expand_tilde(path: &str) -> PathBuf {
    match path.strip_prefix("~/") {
        Some(rest) => match home_dir() {
            Some(home) => home.join(rest),
            None => PathBuf::from(path),
        },
        None => PathBuf::from(path),
    }
}

/// [`home_dir`] with the `.` fallback that most callers in the daemon already
/// applied by hand (`var_os("HOME").map(PathBuf::from).unwrap_or_else(|| ".".into())`).
///
/// Falling back to the working directory is a degradation, not a correct
/// answer — it exists so a missing home directory produces a wrong-but-inert
/// relative path rather than a panic. Prefer [`home_dir`] and handle `None`
/// explicitly wherever the caller can report the failure.
pub fn home_dir_or_cwd() -> PathBuf {
    home_dir().unwrap_or_else(|| PathBuf::from("."))
}

/// [`expand_tilde`] for callers that need a `String` back rather than a
/// `PathBuf` (config fields that are stored and logged as strings).
///
/// Lossy conversion is acceptable here: these values are filesystem paths that
/// came from UTF-8 config in the first place, so a non-UTF-8 round trip is not
/// reachable in practice.
pub fn expand_tilde_string(path: &str) -> String {
    expand_tilde(path).to_string_lossy().into_owned()
}

/// `~/.meridian` — the canonical per-user data directory holding
/// `meridian.db`, `settings.json`, `.env`, `logs/`, and the telemetry spool.
///
/// Returns `None` only when the home directory itself cannot be resolved.
pub fn meridian_dir() -> Option<PathBuf> {
    home_dir().map(|h| h.join(".meridian"))
}

/// Whether `path` contains `needle` as a **whole path component**, compared
/// with the platform's own separator rules.
///
/// This exists to replace substring checks like `path_str.contains("/.claude/")`,
/// which silently evaluate to `false` on Windows (where the separator is `\`)
/// and therefore misroute rather than fail loudly. Matching on
/// [`std::path::Component`]s is separator-agnostic and additionally avoids
/// false positives from a partial match inside a longer directory name
/// (`/.claude-backup/` no longer matches `.claude`).
pub fn has_component(path: &Path, needle: &str) -> bool {
    path.components()
        .any(|c| c.as_os_str().to_string_lossy() == needle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, MutexGuard, OnceLock};

    /// Env vars are process-global and Rust runs tests in parallel threads, so
    /// every test that mutates `HOME` must hold this lock — otherwise one
    /// test's `set_var` is observed by another mid-assertion. Same pattern as
    /// `meridian::config`'s test module.
    fn env_lock() -> MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        // A poisoned lock only means some other test panicked while holding
        // it; the env var is restored below regardless, so recovering is safe
        // and avoids one failure cascading into all the others.
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    /// Set `HOME` for the duration of `f`, restoring the previous value after —
    /// including on panic, so a failing assertion cannot leak state into the
    /// rest of the suite.
    fn with_home<R>(value: Option<&str>, f: impl FnOnce() -> R) -> R {
        let _guard = env_lock();
        let prev = std::env::var_os("HOME");
        match value {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
        let out = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
        match prev {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
        match out {
            Ok(r) => r,
            Err(p) => std::panic::resume_unwind(p),
        }
    }

    /// Guards the load-bearing property: `$HOME` wins, so the existing
    /// `$HOME`-setting tests elsewhere in the tree keep isolating correctly and
    /// macOS behaviour is unchanged from the two implementations this replaced.
    #[test]
    fn home_dir_prefers_env_home() {
        with_home(Some("/tmp/meridian-paths-test-home"), || {
            assert_eq!(
                home_dir(),
                Some(PathBuf::from("/tmp/meridian-paths-test-home"))
            );
        });
    }

    /// An empty `$HOME` must not resolve `~/x` to the relative path `x` — that
    /// would silently write the DB into the process's working directory.
    #[test]
    fn empty_home_is_treated_as_unset() {
        with_home(Some(""), || {
            // Falls through to the platform lookup; whatever it returns, the
            // one thing that must never happen is treating "" as the home dir.
            assert_ne!(home_dir(), Some(PathBuf::from("")));
        });
    }

    #[test]
    fn expand_tilde_leaves_non_tilde_paths_alone() {
        assert_eq!(expand_tilde("/abs/path"), PathBuf::from("/abs/path"));
        assert_eq!(
            expand_tilde("relative/path"),
            PathBuf::from("relative/path")
        );
        // Bare "~" and "~user/" are deliberately NOT expanded.
        assert_eq!(expand_tilde("~"), PathBuf::from("~"));
        assert_eq!(expand_tilde("~other/x"), PathBuf::from("~other/x"));
    }

    #[test]
    fn expand_tilde_joins_with_the_platform_separator() {
        with_home(Some("/tmp/meridian-expand-test"), || {
            assert_eq!(
                expand_tilde("~/.meridian/meridian.db"),
                PathBuf::from("/tmp/meridian-expand-test").join(".meridian/meridian.db")
            );
        });
    }

    /// The whole point of [`has_component`]: it must match regardless of which
    /// separator built the path, and must not fire on a partial name match.
    #[test]
    fn has_component_is_separator_agnostic_and_exact() {
        let p = Path::new("/Users/me/.claude/projects/abc.jsonl");
        assert!(has_component(p, ".claude"));
        assert!(has_component(p, "projects"));
        // Partial matches must not count — the substring check this replaced
        // would have matched ".claude" inside ".claude-backup".
        assert!(!has_component(
            Path::new("/Users/me/.claude-backup/x.jsonl"),
            ".claude"
        ));
        assert!(!has_component(p, ".codex"));
    }
}
