//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! OS-specific daemon plumbing, behind one interface.
//!
//! Three concerns live here because each is genuinely unavailable in portable
//! form and each is load-bearing at startup:
//!
//! - **The IPC endpoint** — the tray/UI health probe, and a first, cheap look
//!   at whether another daemon is up. A Unix domain socket on Unix, a named
//!   pipe on Windows.
//! - **The single-instance lock** — the *authoritative* guard, and the one
//!   thing here that is atomic. [`acquire_single_instance_lock`], `flock` on
//!   Unix and `LockFileEx` on Windows.
//! - **The shutdown signal set** — `SIGINT`/`SIGTERM`/`SIGHUP` on Unix,
//!   Ctrl-C / console-close / system-shutdown events on Windows.
//!
//! The first two are easy to mistake for one thing. They are not: see
//! [`acquire_single_instance_lock`] for why the probe cannot replace the lock,
//! and why the lock does not make the probe redundant.
//!
//! # Why a module rather than inline `cfg`
//!
//! The repo's convention is that leaf-level differences (a field, a single
//! call) stay as inline `#[cfg]` where they occur, and anything larger is
//! promoted to a sibling module re-exported through one name. Both concerns
//! here are well past "a few lines" — the Unix socket path needs a listener
//! task and a file to unlink, the named pipe needs neither and has entirely
//! different reconnect semantics — so they get real per-OS files rather than
//! `cfg` blocks threaded through `main`.
//!
//! `#[path]` rewrites are deliberately avoided: they work, but they hide which
//! file is active from rust-analyzer and from anyone reading the tree.
//!
//! # The contract callers depend on
//!
//! [`daemon_already_running`] must return `true` only when a **live** daemon
//! answers — never merely because a stale endpoint is left over from a crash.
//! Both implementations satisfy this by requiring a greeting to come back, not
//! just a successful connect. Getting this wrong in either direction is
//! costly: a false `true` means the daemon refuses to start after a crash, and
//! a false `false` means two daemons write one `meridian.db`, double every ETL
//! pass, and fire the worklog trigger twice.
//!
//! What it does NOT promise, and what the lock is for: it is a **probe**, so
//! two daemons starting together can both get `false` and both proceed. The
//! answer is only true of the instant it was asked.
//!
//! # Who calls this
//! `src/main.rs`, at startup (probe, then lock, then pool, then listener) and
//! around the main loop's shutdown select.

#[cfg(unix)]
mod unix;
#[cfg(unix)]
pub use unix::{
    daemon_already_running, disk_free_gb, endpoint_display, list_process_argvs, release_endpoint,
    service_manifest, service_status, spawn_health_listener, wait_for_shutdown,
};

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::{
    daemon_already_running, disk_free_gb, endpoint_display, list_process_argvs, release_endpoint,
    service_manifest, service_status, spawn_health_listener, wait_for_shutdown,
};

// ── Single-instance lock ────────────────────────────────────────────────────

/// Why a lock attempt did not succeed.
///
/// The distinction is the whole point of this type, and collapsing it is how
/// this change would turn from a fix into an outage — see [`LockOutcome`].
pub(crate) enum LockError {
    /// Another live process holds the lock. Authoritative: the OS says so.
    Held,
    /// The lock could not be attempted or answered at all — the directory
    /// could not be created, the filesystem is read-only, permissions deny it,
    /// or the syscall failed for a reason that is not contention.
    Other(String),
}

/// What [`acquire_single_instance_lock`] concluded.
///
/// Three states, not two, deliberately:
///
/// - [`Acquired`](Self::Acquired) — this process owns the data dir. Proceed.
/// - [`HeldByAnother`](Self::HeldByAnother) — a live daemon owns it. Stand
///   down cleanly, exactly as the endpoint probe already does.
/// - [`Unavailable`](Self::Unavailable) — we could not find out. **Proceed
///   unlocked**, with a warning.
///
/// That last one is not defensive padding, it is the safety property. Before
/// this lock existed there was no lock at all, so running unlocked is precisely
/// the status quo and costs nothing that was previously guaranteed. Refusing to
/// start, by contrast, would be a BRAND NEW way for the daemon to be
/// permanently dead on a machine where nothing was actually wrong — a
/// read-only home directory or an odd `errno` would brick an install that
/// worked fine yesterday. A guard against a rare race must never be able to
/// cause a common outage.
pub enum LockOutcome {
    /// The lock is held by this process for as long as the guard lives.
    Acquired(DaemonLock),
    /// Another process holds it. Do not touch `meridian.db`.
    HeldByAnother,
    /// Indeterminate; the string is the underlying error for the log.
    Unavailable(String),
}

/// Ownership of the single-instance lock, released when this is dropped.
///
/// Holds the open file and nothing else: on both platforms the lock is tied to
/// the open file description, so the OS drops it when the handle closes —
/// including on `SIGKILL`, a panic, or a power loss. That is what makes this
/// safe where a marker FILE would not be: there is no stale state to clean up
/// and therefore no "is this leftover lock real?" question to get wrong.
///
/// The file itself is never deleted. Unlinking on shutdown would reintroduce
/// exactly the race being closed (process A unlinks while B holds a lock on the
/// same inode; C then creates a fresh file and locks that instead, and A and C
/// both believe they are alone).
pub struct DaemonLock {
    _file: std::fs::File,
}

/// `~/.meridian/daemon.lock` — the file whose lock means "this process owns
/// this data directory".
///
/// Sits beside `daemon.sock` and scopes to the same data dir the endpoint does,
/// so the two guards answer about the same thing.
fn lock_path() -> std::path::PathBuf {
    meridian_core::paths::home_dir_or_cwd()
        .join(".meridian")
        .join("daemon.lock")
}

/// Take the single-instance lock for this data directory.
///
/// # Why this exists alongside [`daemon_already_running`]
///
/// They are not redundant, and deleting either one reopens a real hazard:
///
/// - The **endpoint probe** is the cheap, informative check. It produces the
///   good log line, and it is the only thing that sees a daemon from a build
///   that predates this lock — which matters for the whole rollout window
///   during which an old daemon takes no lock at all.
/// - **This lock** is the authoritative one. The probe is check-then-act: two
///   daemons starting together both find nothing listening (the winner has not
///   bound its listener yet — the bind is deliberately deferred until after
///   the database is open) and both proceed into `setup_db`, running
///   migrations on one file. `flock`/`LockFileEx` is an atomic acquire, so
///   exactly one caller wins no matter how the two are interleaved.
///
/// # Related
/// - `src/main.rs` — the sole caller, immediately before `setup_db`.
/// - [`LockOutcome`] — why a failure to lock does not stop the daemon.
pub fn acquire_single_instance_lock() -> LockOutcome {
    acquire_lock_at(&lock_path())
}

/// [`acquire_single_instance_lock`] against an explicit path, so the behaviour
/// can be tested without touching the real `~/.meridian`.
fn acquire_lock_at(path: &std::path::Path) -> LockOutcome {
    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            return LockOutcome::Unavailable(format!("could not create {}: {e}", parent.display()));
        }
    }
    // Opened read+write and SHARED (no exclusive share mode on Windows): the
    // lock is taken as a separate, explicit step below. Opening exclusively
    // would conflict with anything else that merely has the file open — an
    // antivirus scanner, a backup agent — and that is indistinguishable from a
    // real second daemon, which is the one mistake that must not be made here.
    let file = match std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
    {
        Ok(f) => f,
        Err(e) => {
            return LockOutcome::Unavailable(format!("could not open {}: {e}", path.display()))
        }
    };
    match lock_file_exclusive(&file) {
        Ok(()) => LockOutcome::Acquired(DaemonLock { _file: file }),
        Err(LockError::Held) => LockOutcome::HeldByAnother,
        Err(LockError::Other(e)) => LockOutcome::Unavailable(e),
    }
}

#[cfg(unix)]
use unix::lock_file_exclusive;
#[cfg(windows)]
use windows::lock_file_exclusive;

#[cfg(test)]
mod tests {
    use super::*;

    fn describe(outcome: &LockOutcome) -> String {
        match outcome {
            LockOutcome::Acquired(_) => "Acquired".into(),
            LockOutcome::HeldByAnother => "HeldByAnother".into(),
            LockOutcome::Unavailable(e) => format!("Unavailable({e})"),
        }
    }

    /// The property the whole change rests on: once one caller holds the lock,
    /// a second acquire of the same path loses — atomically, with no window in
    /// which both believe they won.
    ///
    /// This works in-process because both `flock` and `LockFileEx` attach the
    /// lock to the OPEN FILE DESCRIPTION, and `acquire_lock_at` opens the file
    /// fresh each call. Two `open`s therefore contend exactly as two processes
    /// would — which is what makes the guarantee testable at all without
    /// spawning real daemons.
    #[test]
    fn a_second_acquire_of_the_same_path_loses() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("sub").join("daemon.lock");

        let first = acquire_lock_at(&path);
        assert!(
            matches!(first, LockOutcome::Acquired(_)),
            "the first acquire must win on a fresh path, got {}",
            describe(&first)
        );

        let second = acquire_lock_at(&path);
        assert!(
            matches!(second, LockOutcome::HeldByAnother),
            "a second acquire while the first guard is alive must report \
             HeldByAnother - if this is Acquired, two daemons can run \
             migrations on one meridian.db, got {}",
            describe(&second)
        );
        drop(first);
    }

    /// The lock must be released by dropping the guard alone. Nothing unlinks
    /// the file, so if release depended on deletion this would fail — and the
    /// next daemon start would stand down forever against a lock nobody holds.
    #[test]
    fn dropping_the_guard_releases_the_lock() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("daemon.lock");

        let first = acquire_lock_at(&path);
        assert!(matches!(first, LockOutcome::Acquired(_)));
        drop(first);

        let again = acquire_lock_at(&path);
        assert!(
            matches!(again, LockOutcome::Acquired(_)),
            "after the holder drops, the next acquire must win - the lock file \
             is deliberately never deleted, so release comes from closing the \
             handle and nothing else, got {}",
            describe(&again)
        );
        assert!(
            path.exists(),
            "the lock file must survive release: unlinking it would let a \
             later start create a NEW inode and lock that instead, so two \
             processes could each hold 'the' lock"
        );
    }

    /// The asymmetry that keeps this guard from becoming an outage: a lock we
    /// could not even attempt is [`LockOutcome::Unavailable`], never
    /// [`LockOutcome::HeldByAnother`].
    ///
    /// `main` stands down on `HeldByAnother` and proceeds on `Unavailable`, so
    /// misclassifying here means a daemon that refuses to start on a machine
    /// where nothing is wrong.
    ///
    /// Both ways `acquire_lock_at` can fail before it ever reaches the lock
    /// syscall are covered, because they are separate branches and an early
    /// version of this test exercised only the first — it passed unchanged
    /// when the second was deliberately broken.
    #[test]
    fn an_unusable_path_is_unavailable_not_held() {
        let dir = tempfile::tempdir().expect("tempdir");

        // 1. The parent directory cannot be created: a regular file already
        //    occupies that name, so `create_dir_all` fails for real.
        let blocker = dir.path().join("not-a-directory");
        std::fs::write(&blocker, b"").expect("write blocker file");
        let parent_fails = acquire_lock_at(&blocker.join("daemon.lock"));
        assert!(
            matches!(parent_fails, LockOutcome::Unavailable(_)),
            "a lock path whose parent cannot be created must be Unavailable so \
             the daemon proceeds unlocked (the pre-existing behaviour); \
             reporting it as HeldByAnother would make it stand down forever, \
             got {}",
            describe(&parent_fails)
        );

        // 2. The parent is fine but the lock path itself cannot be opened as a
        //    file - here because it is a directory (EISDIR).
        let as_dir = dir.path().join("daemon.lock");
        std::fs::create_dir(&as_dir).expect("create dir at the lock path");
        let open_fails = acquire_lock_at(&as_dir);
        assert!(
            matches!(open_fails, LockOutcome::Unavailable(_)),
            "a lock path that cannot be opened must be Unavailable for the same \
             reason, got {}",
            describe(&open_fails)
        );
    }
}

/// What the health report can say about the service's on-disk definition.
///
/// Same reasoning as [`ServiceStatus`]: `Missing` is a finding, `Unknown` is
/// an admission, and reporting the second as the first is how a health check
/// loses the user's trust.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceManifest {
    /// Present and the OS accepts it (on Unix: `plutil -lint` passes).
    Valid,
    /// Queried, and absent or malformed.
    Invalid,
    /// No service integration for this platform yet.
    Unknown,
}

/// What the OS's service manager says about the daemon's own service.
///
/// The third variant is the reason this is an enum rather than
/// `Option<pid>`: "I could not determine this" and "it is definitely not
/// running" are different answers, and collapsing them makes `meridian doctor`
/// report a confident CRITICAL on a platform where it simply has not looked.
/// A health report that cries wolf is worse than one that admits ignorance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceStatus {
    /// The service manager reports it running, with this pid.
    Running(i64),
    /// The service manager was queried and the daemon is not loaded.
    NotRunning,
    /// No service integration exists for this platform yet — say so rather
    /// than guessing.
    Unknown,
}

/// The greeting a live daemon writes to every accepted connection.
///
/// Shared by both implementations so the probe and the listener cannot drift
/// apart, and so the tray's own probe has one shape to expect. The trailing
/// newline is part of the contract — the tray reads a line.
pub(crate) fn greeting() -> String {
    format!("{{\"running\":true,\"pid\":{}}}\n", std::process::id())
}
