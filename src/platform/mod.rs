//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! OS-specific daemon plumbing, behind one interface.
//!
//! Two concerns live here because both are genuinely unavailable in portable
//! form and both are load-bearing at startup:
//!
//! - **The IPC endpoint** — simultaneously the tray/UI health probe and the
//!   single-instance guard. A Unix domain socket on Unix, a named pipe on
//!   Windows.
//! - **The shutdown signal set** — `SIGINT`/`SIGTERM`/`SIGHUP` on Unix,
//!   Ctrl-C / console-close / system-shutdown events on Windows.
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
//! # Who calls this
//! `src/main.rs`, at startup (single-instance check, then listener) and around
//! the main loop's shutdown select.

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
