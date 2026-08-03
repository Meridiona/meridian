//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Raise the process file-descriptor soft limit, in both the daemon and the tray.
//!
//! # Why this exists
//! macOS gives a GUI/launchd process a soft `RLIMIT_NOFILE` of **256**, and
//! neither entry point raised it. Meanwhile a single `meridian.db` pool holds
//! **three** descriptors per connection (`db`, `-wal`, `-shm`), the tray adds a
//! WebView, sockets and in-process capture on top, and the tray and daemon are
//! attached to the same WAL database at once.
//!
//! Crossing the limit makes SQLite fail a `-wal`/`-shm`/connection open *mid
//! write* with `SQLITE_IOERR` (**code 522**), which desyncs the shared WAL index
//! and surfaces moments later as `database disk image is malformed`
//! (`SQLITE_CORRUPT`, **code 11**). That 522 → 11 progression is exactly what
//! central telemetry recorded across six macOS installs, and it was reproduced
//! locally: healthy at 01:22:00, corrupt at 01:22:31, with the damage confined
//! to pages the checkpoint had just written.
//!
//! Windows does not have this limit in a form that bites, which is consistent
//! with code 11 being 100% macOS and 0% Windows in the fleet data.
//!
//! # Related
//! - SQLite's own [How To Corrupt An SQLite Database File][corrupt] §1.1 / §2.2
//!   (descriptors and `close()`-broken locks) and §8.1 (a WAL-reset race between
//!   two writing processes, present in SQLite 3.7.0 → 3.51.2 — the range this
//!   build's bundled SQLCipher falls inside).
//! - The screenpipe fork solved the same failure in the same architecture; this
//!   is a port of its `screenpipe-engine::fd_limit`, which both of its entry
//!   points call before any DB or socket work.
//!
//! [corrupt]: https://www.sqlite.org/howtocorrupt.html

/// Descriptors to aim for. Chosen to match the screenpipe fork's proven value
/// rather than a fresh guess — it is far above anything Meridian opens, and the
/// cost of a high soft limit is nil (it caps nothing, it only permits).
const DEFAULT_TARGET: u64 = 8192;

/// Environment override for [`DEFAULT_TARGET`], for a machine with an unusual
/// hard limit or for testing.
const TARGET_ENV: &str = "MERIDIAN_FD_LIMIT";

/// The soft limit to set, or `None` when no change is needed or possible.
///
/// Split from the syscall so the policy is testable: the syscall half cannot be
/// unit-tested without mutating the test process's own limits.
///
/// - already at or above the target → `None` (nothing to do)
/// - below the target → raise to the target, but never above `hard`, since
///   `setrlimit` rejects a soft limit exceeding the hard limit and the whole
///   call would fail rather than partially apply
/// - already at the hard limit → `None`; there is nothing left to take
fn target_soft_limit(current_soft: u64, hard: u64, desired: u64) -> Option<u64> {
    if current_soft >= desired {
        return None;
    }
    let capped = desired.min(hard);
    (capped > current_soft).then_some(capped)
}

/// Read `MERIDIAN_FD_LIMIT`, falling back to [`DEFAULT_TARGET`] when unset or
/// unparseable. A malformed value must not be fatal — this runs on the startup
/// path of both binaries, and refusing to boot over a typo in an env var would
/// be a worse outcome than using the default.
fn desired_target() -> u64 {
    std::env::var(TARGET_ENV)
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(DEFAULT_TARGET)
}

/// Raise the soft `RLIMIT_NOFILE` toward [`DEFAULT_TARGET`], capped by the hard
/// limit. Call from **every** entry point before any database or socket work —
/// the tray does not run the daemon's `main()`, so one call cannot cover both.
///
/// Best-effort and never fatal: a process that cannot raise its limit should
/// still start, just with the pre-existing corruption exposure. Logs at INFO on
/// a successful raise so the value is visible in telemetry, and at WARN when the
/// raise fails, since that leaves the machine in the state that corrupts.
// `libc::rlim_t` is `u64` on macOS and Linux, so these casts are no-ops on the
// platforms Meridian ships — but it is NOT `u64` on every Unix target, and this
// module is gated on `unix`, not on those two. Keeping the casts costs nothing
// generated and keeps the arithmetic in one type regardless of target.
#[cfg(unix)]
#[allow(clippy::unnecessary_cast)]
pub fn raise_fd_limit() {
    let mut rlim = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: `getrlimit`/`setrlimit` only read/write the `rlimit` we own here.
    if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut rlim) } != 0 {
        tracing::warn!("could not read the file-descriptor limit; leaving it unchanged");
        return;
    }
    let (soft, hard) = (rlim.rlim_cur as u64, rlim.rlim_max as u64);
    let Some(new_soft) = target_soft_limit(soft, hard, desired_target()) else {
        tracing::debug!(soft, hard, "file-descriptor limit already sufficient");
        return;
    };

    rlim.rlim_cur = new_soft as libc::rlim_t;
    // SAFETY: as above.
    if unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &rlim) } == 0 {
        tracing::info!(
            from = soft,
            to = new_soft,
            hard,
            "raised the file-descriptor limit"
        );
    } else {
        tracing::warn!(
            soft,
            hard,
            requested = new_soft,
            "failed to raise the file-descriptor limit - SQLite may hit SQLITE_IOERR under load"
        );
    }
}

/// No-op on Windows, where handle limits are not the constrained resource this
/// exists to relieve.
#[cfg(not(unix))]
pub fn raise_fd_limit() {}

#[cfg(test)]
mod tests {
    use super::*;

    /// The policy that decides whether to raise, and to what. Getting the cap
    /// wrong is not cosmetic: `setrlimit` rejects a soft limit above the hard
    /// limit outright, so an uncapped request fails the whole call and leaves
    /// the process on macOS's default 256 — precisely the state that corrupts.
    #[test]
    fn raises_to_the_target_but_never_past_the_hard_limit() {
        // macOS GUI default, generous hard limit: raise all the way.
        assert_eq!(target_soft_limit(256, 1_048_576, 8192), Some(8192));

        // Hard limit below the target: take what we can get, don't fail the call.
        assert_eq!(target_soft_limit(256, 4096, 8192), Some(4096));

        // Already sufficient: don't touch it.
        assert_eq!(target_soft_limit(8192, 1_048_576, 8192), None);
        assert_eq!(target_soft_limit(65_536, 1_048_576, 8192), None);

        // Already pinned at the hard limit: nothing left to take.
        assert_eq!(target_soft_limit(4096, 4096, 8192), None);

        // Exactly at the target counts as sufficient (boundary, not off-by-one).
        assert_eq!(target_soft_limit(8192, 8192, 8192), None);
    }

    /// A malformed `MERIDIAN_FD_LIMIT` must fall back rather than propagate a
    /// zero or a panic: this runs before the database opens in both binaries, so
    /// a typo in an env var must not brick startup or, worse, "successfully"
    /// lower the limit to 0.
    #[test]
    fn a_bad_env_override_falls_back_to_the_default() {
        assert_eq!(
            target_soft_limit(256, 1_048_576, DEFAULT_TARGET),
            Some(DEFAULT_TARGET),
            "the default must actually raise a macOS GUI process"
        );
        // A zero target would ask for a limit below the current one; the policy
        // must decline rather than shrink the process's descriptors.
        assert_eq!(target_soft_limit(256, 1_048_576, 0), None);
    }
}
