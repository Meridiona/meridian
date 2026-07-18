//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Unix daemon plumbing: a `~/.meridian/daemon.sock` domain socket, and the
//! `SIGINT`/`SIGTERM`/`SIGHUP` shutdown set.
//!
//! This is the original implementation, moved out of `main.rs` unchanged when
//! the Windows arm was added — including the 800 ms probe timeouts and the
//! stale-socket unlink ordering, both of which are load-bearing (see
//! [`daemon_already_running`]).
//!
//! # Related
//! - [`super`] — the shared contract and why this is a module rather than `cfg`.
//! - `super::windows` — the named-pipe counterpart.

use std::path::PathBuf;
use std::time::Duration;

/// How long to wait for a connect, and then for a greeting, when probing.
///
/// Deliberately short: this runs on the startup path, and the common failure
/// it guards against (a socket file left behind by a crash, with nothing
/// listening) resolves as connection-refused essentially instantly. The
/// timeout only bites for a socket whose peer is wedged, where waiting longer
/// would stall every start.
const PROBE_TIMEOUT: Duration = Duration::from_millis(800);

/// `~/.meridian/daemon.sock`.
fn endpoint() -> PathBuf {
    meridian_core::paths::home_dir_or_cwd()
        .join(".meridian")
        .join("daemon.sock")
}

/// The endpoint in a form suitable for a log field.
pub fn endpoint_display() -> String {
    endpoint().display().to_string()
}

/// Is a live daemon already serving this data directory?
///
/// Returns `true` only when something connects **and** sends a greeting. A
/// missing socket, or a stale one left by a previous crash with no listener
/// behind it, refuses or times out and yields `false` — so this instance is
/// free to take over. See [`super`] for why both directions matter.
pub async fn daemon_already_running() -> bool {
    use tokio::io::AsyncReadExt as _;
    use tokio::time::timeout;

    let path = endpoint();
    let connect = timeout(PROBE_TIMEOUT, tokio::net::UnixStream::connect(&path)).await;
    let Ok(Ok(mut stream)) = connect else {
        return false; // no listener (absent or stale socket) — safe to take over
    };
    // A non-empty read confirms a real daemon is there, not just a leftover
    // socket file that something else happens to have open.
    let mut buf = Vec::new();
    let _ = timeout(PROBE_TIMEOUT, stream.read_to_end(&mut buf)).await;
    !buf.is_empty()
}

/// Bind the endpoint and serve the greeting to every caller, forever.
///
/// Removes a stale socket file first — safe only because
/// [`daemon_already_running`] has already established that nothing is
/// listening on it. Call them in that order.
pub fn spawn_health_listener() -> anyhow::Result<()> {
    use tokio::io::AsyncWriteExt as _;

    let path = endpoint();
    let _ = std::fs::remove_file(&path);
    let listener = tokio::net::UnixListener::bind(&path)?;
    tokio::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((mut stream, _)) => {
                    let msg = super::greeting();
                    tokio::spawn(async move {
                        let _ = stream.write_all(msg.as_bytes()).await;
                    });
                }
                Err(e) => {
                    tracing::warn!(error = %e, "daemon.sock accept error");
                    break;
                }
            }
        }
    });
    Ok(())
}

/// Unlink the socket file on clean shutdown.
///
/// Best-effort: a leftover file is harmless — the next start's probe finds no
/// listener and unlinks it anyway.
pub fn release_endpoint() {
    let _ = std::fs::remove_file(endpoint());
}

/// Ask launchd whether the agent labelled `label` is loaded, and as what pid.
///
/// Moved here verbatim from `health::platform`. Never returns
/// [`super::ServiceStatus::Unknown`] — on Unix the query always has a real
/// answer, so a failure to run `launchctl` genuinely means not-loaded.
pub fn service_status(label: &str) -> super::ServiceStatus {
    use super::ServiceStatus;
    use std::process::Command;

    let Ok(out) = Command::new("launchctl").args(["list", label]).output() else {
        return ServiceStatus::NotRunning;
    };
    if !out.status.success() {
        return ServiceStatus::NotRunning;
    }
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        if let Some(rest) = line.trim().strip_prefix("\"PID\" = ") {
            if let Ok(pid) = rest.trim_end_matches(';').trim().parse() {
                return ServiceStatus::Running(pid);
            }
        }
    }
    ServiceStatus::NotRunning
}

/// Is `~/Library/LaunchAgents/<label>.plist` present and well-formed?
pub fn service_manifest(label: &str) -> super::ServiceManifest {
    use super::ServiceManifest;
    use std::process::Command;

    let p = meridian_core::paths::home_dir_or_cwd()
        .join("Library/LaunchAgents")
        .join(format!("{label}.plist"));
    let ok = p.is_file()
        && Command::new("plutil")
            .arg("-lint")
            .arg(&p)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
    if ok {
        ServiceManifest::Valid
    } else {
        ServiceManifest::Invalid
    }
}

/// Free space on the volume holding `path`, in GB, via `df -Pk`.
pub fn disk_free_gb(path: &std::path::Path) -> Option<f64> {
    use std::process::Command;

    let out = Command::new("df").arg("-Pk").arg(path).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout);
    let avail_kb: f64 = s.lines().nth(1)?.split_whitespace().nth(3)?.parse().ok()?;
    Some(avail_kb / 1_048_576.0)
}

/// Resolve when the OS asks the daemon to stop.
///
/// `SIGHUP` is treated as "reload config": it takes the same clean-shutdown
/// path as `SIGTERM` so launchd restarts the daemon with the new
/// `settings.json` applied.
pub async fn wait_for_shutdown() {
    use tokio::signal::unix::{signal, SignalKind};

    // Registering here rather than at startup keeps the handles local to this
    // future; a registration failure is fatal in the same way it was when
    // `main` did it eagerly, so it panics rather than silently never firing —
    // a daemon that cannot be signalled is worse than one that fails loudly.
    let mut sigint = signal(SignalKind::interrupt()).expect("register SIGINT handler");
    let mut sigterm = signal(SignalKind::terminate()).expect("register SIGTERM handler");
    let mut sighup = signal(SignalKind::hangup()).expect("register SIGHUP handler");

    tokio::select! {
        _ = sigint.recv()  => tracing::info!("SIGINT received"),
        _ = sigterm.recv() => tracing::info!("SIGTERM received"),
        _ = sighup.recv()  => tracing::info!("SIGHUP received — reloading (graceful restart)"),
    }
}
