//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Cross-process deduplication for PM task sync.
//!
//! PM sync is on-demand (see [`crate::intelligence::run_pm_sync`]): the tray
//! triggers it by spawning `meridian pm-sync` when the user connects a tracker
//! or opens the dashboard, and the daemon triggers it before a worklog drafting
//! sweep. Several of those can land within a second of each other - opening the
//! dashboard while a drafting sweep is starting is the ordinary case, not a
//! corner one - and each would independently read `pm_sync_state`, independently
//! decide the cache is stale, and fetch the same board again.
//!
//! This is the ONE mechanism that stops that. It is deliberately a **file** lock
//! and not a table:
//!
//! * The reverted `pm_sync_requests` outbox (PRs #909/#910) coordinated the tray
//!   and the daemon through `meridian.db`, and the tray's read-back of the
//!   outcome is what failed in production with `SQLITE_IOERR_SHORT_READ` (522) -
//!   a short read while the daemon truncated the WAL on shutdown. Coordination
//!   state that lives outside the database cannot fail that way, and needs no
//!   migration, so shipping it cannot damage anyone's data.
//! * A crashed holder releases automatically: the lock dies with its fd. No
//!   stale-row cleanup, no `claimed_at` timestamps to reap.
//!
//! # Who calls this
//! [`crate::intelligence::run_pm_sync`] (gated - skips on contention) and
//! [`crate::intelligence::run_pm_force_sync`] (forced - waits, then proceeds).
//!
//! # Related
//! - [`meridian_oauth::store::lock_provider`] - the same advisory-file-lock
//!   pattern, guarding the rotating OAuth refresh token. That one serialises
//!   *token* writes and is unaffected by this module; a duplicate fetch is
//!   wasteful, a duplicate token refresh is destructive, so they stay separate
//!   locks with different contention policies.

use std::path::PathBuf;

use anyhow::{Context, Result};

/// Held for the duration of one sync. Releasing is `Drop` (the fd closes), so a
/// panic or a killed process frees it without any cleanup path.
#[derive(Debug)]
pub struct SyncLock {
    _file: std::fs::File,
}

/// `~/.meridian/pm-sync.lock`. Alongside the OAuth store's lock files rather
/// than in a temp dir: a per-user path that survives reboots and is not shared
/// between accounts on one machine.
fn lock_path() -> Result<PathBuf> {
    let home = meridian_core::paths::home_dir()
        .context("resolving the home directory for the PM sync lock")?;
    Ok(home.join(".meridian").join("pm-sync.lock"))
}

/// Open (creating if needed) the lock file. Split out so both acquire paths
/// share it and neither can drift on flags - `truncate(false)` matters: the file
/// is a lock, never a payload, and truncating it would be a pointless write on
/// every sync.
fn open_lock_file() -> Result<std::fs::File> {
    let path = lock_path()?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("creating {} for the PM sync lock", dir.display()))?;
    }
    std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
        .with_context(|| format!("opening PM sync lock {}", path.display()))
}

/// Try to take the lock without waiting.
///
/// `Ok(Some(_))` - acquired, this process owns the sync.
/// `Ok(None)`   - another process is syncing right now; the caller should SKIP,
///                because that run is already doing the work this one wanted.
/// `Err(_)`     - the lock could not be evaluated at all (no home dir,
///                unwritable `~/.meridian`).
///
/// Used by the gated path. Skipping on contention is the whole point: two
/// concurrent gated syncs would fetch the same board twice.
pub fn try_acquire() -> Result<Option<SyncLock>> {
    let file = open_lock_file()?;
    match file.try_lock() {
        Ok(()) => Ok(Some(SyncLock { _file: file })),
        Err(std::fs::TryLockError::WouldBlock) => Ok(None),
        Err(std::fs::TryLockError::Error(e)) => {
            Err(anyhow::Error::new(e).context("evaluating the PM sync lock"))
        }
    }
}

/// Wait up to `timeout` for the lock, then give up.
///
/// `Ok(Some(_))` - acquired. `Ok(None)` - still held when the budget ran out.
///
/// Used by the FORCED path, which must not silently skip: a user pressing "Sync
/// now" has to get a sync. Waiting is nearly always brief (a gated sync is one
/// HTTP fetch per provider), and a caller that times out is told so rather than
/// racing the holder - see [`crate::intelligence::run_pm_force_sync`].
///
/// Polls a non-blocking try-lock rather than blocking on `flock`, so the async
/// executor is never parked - identical reasoning to
/// [`meridian_oauth::store::lock_provider`].
pub async fn acquire_waiting(timeout: std::time::Duration) -> Result<Option<SyncLock>> {
    let step = std::time::Duration::from_millis(100);
    let mut waited = std::time::Duration::ZERO;
    loop {
        if let Some(guard) = try_acquire()? {
            return Ok(Some(guard));
        }
        if waited >= timeout {
            return Ok(None);
        }
        tokio::time::sleep(step).await;
        waited += step;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Serialises the tests below. Both mutate `HOME` (process-global) to point
    /// `lock_path` at a scratch dir, and cargo runs tests in parallel threads -
    /// so without this one test's `set_var` lands mid-flight in the other and it
    /// contends on the WRONG lock file. That is not hypothetical: it is exactly
    /// how these two first failed. Mirrors `meridian_oauth::env_test_guard`.
    fn env_guard() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        LOCK.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Point `HOME` at a fresh scratch dir for the duration of a test, restoring
    /// it afterwards so a failure cannot leak a bogus `HOME` into later tests.
    struct ScratchHome {
        dir: std::path::PathBuf,
        prev: Option<std::ffi::OsString>,
        _guard: std::sync::MutexGuard<'static, ()>,
    }

    impl ScratchHome {
        fn new(tag: &str) -> Self {
            let guard = env_guard();
            let dir = std::env::temp_dir()
                .join(format!("meridian_synclock_{tag}_{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let prev = std::env::var_os("HOME");
            std::env::set_var("HOME", &dir);
            Self {
                dir,
                prev,
                _guard: guard,
            }
        }
    }

    impl Drop for ScratchHome {
        fn drop(&mut self) {
            match self.prev.take() {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
            std::fs::remove_dir_all(&self.dir).ok();
        }
    }

    /// Two acquisitions contend and the second is refused, then succeeds once
    /// the first is dropped. `flock` is per-open-file-description, so two opens
    /// in ONE process contend exactly as two processes would - which is what
    /// makes this testable without spawning anything.
    #[test]
    fn a_second_acquire_is_refused_until_the_first_drops() {
        let _home = ScratchHome::new("basic");

        let held = try_acquire().unwrap();
        assert!(held.is_some(), "the first acquire must succeed");

        let contended = try_acquire().unwrap();
        assert!(
            contended.is_none(),
            "a gated caller must be refused while another sync holds the lock, \
             not granted a duplicate"
        );

        drop(held);
        let reacquired = try_acquire().unwrap();
        assert!(
            reacquired.is_some(),
            "the lock must be free again once the holder drops it"
        );
    }

    /// The waiting path returns `Ok(None)` rather than erroring or hanging when
    /// the budget expires - the forced caller needs to distinguish "I have the
    /// lock" from "someone else still does" to report honestly.
    #[test]
    fn waiting_gives_up_with_none_when_the_budget_expires() {
        let _home = ScratchHome::new("waiting");

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        let held = try_acquire().unwrap().expect("first acquire");
        let timed_out = rt.block_on(acquire_waiting(std::time::Duration::from_millis(250)));
        assert!(
            matches!(timed_out, Ok(None)),
            "a contended wait must expire as Ok(None), got {timed_out:?}"
        );
        drop(held);
    }

    /// The waiting path takes a FREE lock immediately rather than sleeping out
    /// its budget - the forced path runs on a user's click, so a 20 s wait for
    /// an uncontended lock would be a visible regression.
    #[test]
    fn waiting_acquires_immediately_when_uncontended() {
        let _home = ScratchHome::new("free");

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        let started = std::time::Instant::now();
        let got = rt.block_on(acquire_waiting(std::time::Duration::from_secs(20)));
        assert!(
            matches!(got, Ok(Some(_))),
            "an uncontended wait must acquire, got {got:?}"
        );
        assert!(
            started.elapsed() < std::time::Duration::from_secs(1),
            "an uncontended wait must not sleep - took {:?}",
            started.elapsed()
        );
    }
}
