//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Crash-safe and suspend-safe record of a refresh-token spend.
//!
//! # The bug this exists to close
//!
//! Atlassian ROTATES the refresh token on every use: a successful refresh
//! returns a new pair and invalidates the one you sent. So the exchange has a
//! window in which the token has already been consumed server-side and we do not
//! yet know the replacement. If the response is lost in that window, the grant is
//! gone forever and the user must re-authenticate by hand.
//!
//! Production users hit this repeatedly. The mechanism, measured on one install:
//! a refresh POST was in flight when the Mac suspended for 28 minutes. Atlassian
//! rotated the token and replied; the reply went nowhere. `reqwest`'s timeout
//! could not cancel the request, because that timeout is `Instant`-based and the
//! monotonic clock does not advance while macOS sleeps. On wake the socket was
//! dead, and the retry went out with the OLD refresh token - by then 28 minutes
//! stale, well outside Atlassian's reuse leeway. `invalid_grant`, terminal, grant
//! dead.
//!
//! Nothing in the old code recorded that a spend had been attempted, so there was
//! no way to distinguish "the token was never consumed" from "it was consumed and
//! we lost the answer" - and those need opposite handling.
//!
//! # The rule that makes recovery possible
//!
//! Atlassian permits the PREVIOUS refresh token to be presented again within a
//! grace period after rotation, returning the current pair rather than an error.
//! That is the recovery mechanism, and it is time-bounded, so the only thing that
//! matters is replaying the spend QUICKLY - which is impossible without knowing a
//! spend happened. Hence this journal.
//!
//! # Ordering, which is the whole correctness argument
//!
//! 1. Journal the token we are about to spend, and `fsync` it. Only then POST.
//! 2. On success: persist the new pair, `fsync`, and only THEN clear the journal.
//! 3. On any indeterminate outcome (transport error, timeout, killed process,
//!    suspend), the journal survives and the next attempt replays it.
//!
//! Steps 1 and 2 are deliberately ordered so that every crash point is
//! recoverable, and the recovery is decided by COMPARING TOKENS rather than by
//! trusting a flag:
//!
//! * journalled token == stored token -> the save never happened. The outcome is
//!   unknown; replay with the journalled token.
//! * journalled token != stored token -> the save DID happen and we died before
//!   clearing. The stored pair is newer and valid; just clear the journal.
//!
//! That comparison is why a crash between step 2's write and its journal-clear is
//! harmless, and why this needs no "in progress" boolean that could itself be
//! stale.
//!
//! # Why a file and not the database
//!
//! The OAuth token store is already a per-provider file (see [`crate::store`]);
//! the journal belongs beside it. It is also deliberately NOT in `meridian.db`:
//! two processes write that database, and coordination state living there is what
//! produced a `SQLITE_IOERR_SHORT_READ` in production when a reader hit it during
//! the daemon's WAL checkpoint. A `0600` file written temp-then-rename cannot
//! corrupt a database, needs no migration, and cannot fail a sync.
//!
//! # Who calls this
//! [`crate::jira::ensure_fresh`], under the same locks that serialise the spend
//! itself ([`crate::store::lock_provider`] plus the in-process mutex).

use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// How long after a spend a replay is still worth attempting.
///
/// Atlassian's documented reuse grace for a rotated refresh token is 10 minutes.
/// This is deliberately SHORTER: the clock we compare against is the wall clock,
/// which can be adjusted by NTP after a wake, and a replay attempted a second
/// after the real deadline is indistinguishable from a genuinely dead grant. 8
/// minutes keeps a margin for that skew and for the request's own duration.
///
/// Past this window a replay is still attempted ONCE (it costs one HTTP call and
/// the alternative is a guaranteed re-authentication), but it is expected to fail
/// and is logged as such rather than as a surprise.
pub const REPLAY_WINDOW_SECS: i64 = 8 * 60;

/// A refresh-token spend that was started and whose outcome may be unknown.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PendingSpend {
    /// The refresh token that was sent. Compared against the stored token to
    /// decide whether the save landed - see the module header.
    pub refresh_token: String,
    /// Wall-clock unix seconds at which the POST was issued.
    ///
    /// WALL clock, not monotonic, on purpose: the entire point is to measure a
    /// span that may include a system suspend, and a monotonic clock does not
    /// advance across one. This is the same property whose absence let the
    /// original 8-second request timeout sleep through a 28-minute suspend.
    pub started_at_unix: i64,
    /// The client id the spend was made with, so a replay reconstructs the exact
    /// same request even if configuration changed in between.
    pub client_id: String,
}

impl PendingSpend {
    /// Seconds elapsed since the spend was issued, per the wall clock. Clamped at
    /// zero so a backwards clock adjustment cannot produce a negative age that
    /// would read as "in the future" and be treated as fresh forever.
    pub fn age_secs(&self, now_unix: i64) -> i64 {
        (now_unix - self.started_at_unix).max(0)
    }

    /// Whether a replay is still inside the window where the provider is expected
    /// to honour the previous token.
    pub fn within_replay_window(&self, now_unix: i64) -> bool {
        self.age_secs(now_unix) <= REPLAY_WINDOW_SECS
    }
}

fn journal_path(provider: &str) -> Result<PathBuf> {
    // Reuses `store`'s provider validation and directory resolution, so a journal
    // path can never escape the oauth dir and never disagrees with where the
    // token it describes lives.
    let token_path = crate::store::path(provider)?;
    let dir = token_path
        .parent()
        .context("resolving the OAuth directory for the refresh journal")?;
    Ok(dir.join(format!(".{provider}.refresh-journal.json")))
}

/// Record that `refresh_token` is about to be spent, durably, before the POST.
///
/// `fsync`s the file AND its directory before returning. Both matter: without the
/// file sync the record can be in the page cache only, and without the directory
/// sync the rename itself can be lost - either way a crash leaves no journal,
/// which is exactly the state this function exists to prevent.
pub fn record_spend(provider: &str, spend: &PendingSpend) -> Result<()> {
    let path = journal_path(provider)?;
    let dir = path
        .parent()
        .context("resolving the OAuth directory for the refresh journal")?;
    std::fs::create_dir_all(dir)
        .with_context(|| format!("creating {} for the refresh journal", dir.display()))?;
    let tmp = dir.join(format!(".{provider}.refresh-journal.tmp"));
    let json = serde_json::to_vec_pretty(spend).context("serialising the refresh journal")?;

    {
        use std::io::Write;
        let mut file = open_private(&tmp)?;
        file.write_all(&json)
            .with_context(|| format!("writing {}", tmp.display()))?;
        file.sync_all()
            .with_context(|| format!("fsync {}", tmp.display()))?;
    }
    std::fs::rename(&tmp, &path)
        .with_context(|| format!("renaming {} -> {}", tmp.display(), path.display()))?;
    sync_dir(dir);
    Ok(())
}

/// Read the pending spend, if any. A malformed journal is treated as ABSENT and
/// removed rather than surfaced as an error: it cannot be acted on, and failing
/// the refresh over an unreadable side-file would turn a recovery aid into a new
/// way for auth to break.
pub fn load(provider: &str) -> Option<PendingSpend> {
    let path = journal_path(provider).ok()?;
    let raw = std::fs::read_to_string(&path).ok()?;
    match serde_json::from_str(&raw) {
        Ok(spend) => Some(spend),
        Err(e) => {
            tracing::warn!(
                provider,
                error = %e,
                "refresh journal is unreadable - discarding it and refreshing normally"
            );
            let _ = std::fs::remove_file(&path);
            None
        }
    }
}

/// Clear the journal once the new pair is durably stored.
///
/// Best-effort by design: the new tokens are already persisted at this point, so
/// a leftover journal is harmless - the next `ensure_fresh` compares its token
/// against the stored one, sees they differ, and clears it then. Failing the
/// refresh here would discard a SUCCESSFUL exchange.
pub fn clear(provider: &str) {
    if let Ok(path) = journal_path(provider) {
        match std::fs::remove_file(&path) {
            Ok(()) => tracing::debug!(provider, "refresh journal cleared"),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => tracing::warn!(
                provider,
                error = %e,
                "could not clear the refresh journal - the next refresh will reconcile it"
            ),
        }
    }
}

/// Create/truncate a file that only the owner can read. The mode is set AT OPEN
/// on Unix rather than afterwards, so there is no window in which the file exists
/// world-readable while holding a refresh token.
fn open_private(path: &std::path::Path) -> Result<std::fs::File> {
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    opts.open(path)
        .with_context(|| format!("creating {}", path.display()))
}

/// `fsync` a directory so a rename into it is durable. Unix-only and
/// best-effort: on Windows a directory handle cannot be opened this way, and
/// there `rename` over an existing file is already ordered by the filesystem.
fn sync_dir(dir: &std::path::Path) {
    #[cfg(unix)]
    if let Ok(handle) = std::fs::File::open(dir) {
        if let Err(e) = handle.sync_all() {
            tracing::debug!(dir = %dir.display(), error = %e, "directory fsync failed");
        }
    }
    #[cfg(not(unix))]
    let _ = dir;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spend(token: &str, started_at_unix: i64) -> PendingSpend {
        PendingSpend {
            refresh_token: token.into(),
            started_at_unix,
            client_id: "cid".into(),
        }
    }

    struct ScratchHome {
        dir: std::path::PathBuf,
        prev: Option<std::ffi::OsString>,
        _guard: std::sync::MutexGuard<'static, ()>,
    }

    impl ScratchHome {
        fn new(tag: &str) -> Self {
            let guard = crate::env_test_guard();
            let dir =
                std::env::temp_dir().join(format!("meridian_journal_{tag}_{}", std::process::id()));
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

    #[test]
    fn a_recorded_spend_survives_and_round_trips() {
        let _home = ScratchHome::new("roundtrip");
        assert!(load("jira").is_none(), "no journal before recording");
        let s = spend("old-refresh", 1_000);
        record_spend("jira", &s).unwrap();
        assert_eq!(load("jira").as_ref(), Some(&s));
        clear("jira");
        assert!(
            load("jira").is_none(),
            "cleared journal must read as absent"
        );
    }

    /// The journal holds a refresh token, so it must never be world-readable -
    /// same requirement as the token file itself.
    #[cfg(unix)]
    #[test]
    fn the_journal_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let _home = ScratchHome::new("perms");
        record_spend("jira", &spend("t", 1)).unwrap();
        let path = journal_path("jira").unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "journal must be 0600, got {mode:o}");
    }

    /// A garbage journal must not fail the refresh. It is discarded, so the
    /// caller proceeds with a normal refresh rather than erroring on a side-file.
    #[test]
    fn a_corrupt_journal_reads_as_absent_and_is_removed() {
        let _home = ScratchHome::new("corrupt");
        record_spend("jira", &spend("t", 1)).unwrap();
        let path = journal_path("jira").unwrap();
        std::fs::write(&path, b"{not json").unwrap();
        assert!(
            load("jira").is_none(),
            "a corrupt journal must read as absent"
        );
        assert!(
            !path.exists(),
            "a corrupt journal must be removed, not left to be re-read forever"
        );
    }

    /// The replay window is measured on the WALL clock so it spans a suspend.
    #[test]
    fn the_replay_window_is_bounded_and_clamped() {
        let s = spend("t", 1_000);
        assert!(
            s.within_replay_window(1_000),
            "a spend just issued is in window"
        );
        assert!(
            s.within_replay_window(1_000 + REPLAY_WINDOW_SECS),
            "the boundary itself is in window"
        );
        assert!(
            !s.within_replay_window(1_000 + REPLAY_WINDOW_SECS + 1),
            "one second past the window is out"
        );
        // A backwards clock jump must not read as "issued in the future", which
        // would make the spend look permanently fresh and replay forever.
        assert_eq!(s.age_secs(500), 0, "age is clamped at zero");
        assert!(s.within_replay_window(500));
    }
}
