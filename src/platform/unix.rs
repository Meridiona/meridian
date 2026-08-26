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
                    // Log and keep serving. `break` here killed the health
                    // endpoint for the rest of the process's life on ONE
                    // transient error (EMFILE, ECONNABORTED): every later
                    // probe failed, so the popover said "Not running" next to
                    // a Restart button while capture ran fine, and only a
                    // daemon restart healed it. Accept errors are per-attempt,
                    // not per-listener - the next accept is independent. The
                    // pause keeps a *persistent* error (fd exhaustion) from
                    // spinning this loop hot while it lasts.
                    tracing::warn!(error = %e, "daemon.sock accept error");
                    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                    continue;
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

/// Every running process's argv line, via `ps -axo args=` (argv **only**).
///
/// `pgrep -f` is useless for the caller's purpose: on macOS it matches the
/// environment block too, and `PATH` / `CODEX_*` vars put agent names into half
/// the processes on the box.
///
/// Moved here verbatim, including the hard timeout and `kill_on_drop`: reading
/// proc info can wedge in the kernel on a stuck process, and an un-timed await
/// parks the whole indexer loop (observed live 2026-06-06). `None` means "could
/// not tell", which the caller treats as every-CLI-running — deferring to the
/// idle backstop rather than sealing something prematurely.
pub async fn list_process_argvs() -> Option<Vec<String>> {
    let mut cmd = tokio::process::Command::new("ps");
    cmd.args(["-axo", "args="]).kill_on_drop(true);
    let output = tokio::time::timeout(std::time::Duration::from_secs(5), cmd.output())
        .await
        .ok()?
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(str::to_string)
            .collect(),
    )
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

    // `pid` on every arm: these lines are the record of WHICH daemon generation
    // was asked to stop, and during an update or a quit-then-relaunch there are
    // several within seconds of each other. Without it the signal cannot be
    // matched to the "meridian daemon starting" line it belongs to, and the
    // shutdown sequence — the window where the tray and the daemon can overlap
    // on meridian.db — is unreconstructable after the fact.
    //
    // `as i64` deliberately; see the same cast at the startup log in `main.rs`
    // for why a `u32` would ship as a string, or not at all.
    let pid = std::process::id() as i64;
    tokio::select! {
        _ = sigint.recv()  => tracing::info!(pid, "SIGINT received"),
        _ = sigterm.recv() => tracing::info!(pid, "SIGTERM received"),
        _ = sighup.recv()  => tracing::info!(pid, "SIGHUP received — reloading (graceful restart)"),
    }
}

#[cfg(test)]
mod tests {
    /// One transient `accept()` error must not kill the health endpoint for
    /// the rest of the daemon's life. When this loop `break`s, the listener is
    /// dropped, every later probe fails, and each UI surface tells the user
    /// the daemon is down - next to a Restart button - while capture runs
    /// fine. Only a daemon restart heals it, and nothing ever says why.
    ///
    /// A deterministic accept error cannot be provoked through a real
    /// `UnixListener` from a test, so this pins the policy at the source
    /// level: the error arm of the accept loop must `continue`, never `break`
    /// (same convention as the UI's source-scanning guards, per docs/testing.md's
    /// "test what units cannot reach" rule).
    #[test]
    fn an_accept_error_must_not_end_the_health_listener() {
        let src = include_str!("unix.rs");
        // Brace-matched rather than a fixed byte window: a window can either
        // truncate before `continue` (a false failure on an untouched arm) or
        // run past this arm's closing brace into whatever follows (a `break`
        // anywhere later in the file would then fail this test for the wrong
        // reason). Capturing the exact `Err(e) => { ... }` body is immune to
        // both as the arm grows or shrinks.
        let arm_start = src
            .find("Err(e) => {")
            .expect("the accept-error match arm exists")
            + "Err(e) => {".len();
        let mut depth = 1i32;
        let mut arm_end = None;
        for (i, c) in src[arm_start..].char_indices() {
            match c {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        arm_end = Some(arm_start + i);
                        break;
                    }
                }
                _ => {}
            }
        }
        let arm = &src[arm_start..arm_end.expect("the accept-error arm's closing brace")];
        // The policy is what runs AFTER the warn log, not the arm's leading
        // explanatory comment - which itself narrates the old `break` bug in
        // prose and would trip a naive `contains("break")` over the whole arm.
        let policy = arm
            .split("daemon.sock accept error")
            .nth(1)
            .expect("the warn log is inside the matched arm");
        assert!(
            !policy.contains("break"),
            "the accept-error arm breaks the loop - one transient error would \
             permanently kill the health endpoint: {policy}"
        );
        assert!(
            policy.contains("continue"),
            "the accept-error arm must continue serving: {policy}"
        );
    }
}
