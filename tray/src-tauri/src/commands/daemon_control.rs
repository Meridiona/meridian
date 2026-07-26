//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! The OS-specific mechanics behind the daemon lifecycle commands.
//!
//! [`super::daemon`] owns the Tauri commands and their response shapes, which
//! are platform-agnostic; everything that actually talks to the OS — probing
//! the daemon's IPC endpoint, and asking the service manager to start / stop /
//! restart it — lives here, behind one interface per concern.
//!
//! The split mirrors the daemon's own `meridian::platform`: macOS drives
//! launchd and a Unix domain socket, Windows drives the Task Scheduler and a
//! named pipe. The two are different enough (a socket file with a stale-file
//! problem vs. a pipe name that only exists while served; `launchctl`
//! subcommands vs. `schtasks` verbs) that inline `cfg` in every command would
//! be noisier than a module.
//!
//! # Contract
//! - [`probe`] returns `(running, pid)` only when a live daemon answered with
//!   its greeting — never on a stale endpoint.
//! - [`restart`] / [`set_running`] return `Ok` only when the service manager
//!   accepted the request.

/// Whether the daemon is up, and its pid if it announced one.
pub struct Probe {
    pub running: bool,
    pub pid: Option<u32>,
}

impl Probe {
    fn down() -> Self {
        Self {
            running: false,
            pid: None,
        }
    }
}

/// Parse the daemon's greeting (`{"running":true,"pid":…}`) into a [`Probe`].
/// Shared by both platforms so the two cannot disagree on the wire format.
fn parse_greeting(buf: &[u8]) -> Probe {
    if buf.is_empty() {
        return Probe::down();
    }
    match serde_json::from_slice::<serde_json::Value>(buf) {
        Ok(v) => {
            let pid = v.get("pid").and_then(|p| p.as_u64()).map(|p| p as u32);
            Probe { running: true, pid }
        }
        Err(_) => Probe::down(),
    }
}

// ── macOS: Unix domain socket + launchd ─────────────────────────────────────

#[cfg(target_os = "macos")]
pub async fn probe() -> Probe {
    use std::time::Duration;
    use tokio::io::AsyncReadExt;
    use tokio::net::UnixStream;
    use tokio::time::timeout;

    let sock_path = sock_path();
    let Ok(Ok(mut stream)) =
        timeout(Duration::from_millis(800), UnixStream::connect(&sock_path)).await
    else {
        return Probe::down();
    };
    let mut buf = Vec::new();
    let _ = timeout(Duration::from_millis(800), stream.read_to_end(&mut buf)).await;
    parse_greeting(&buf)
}

#[cfg(target_os = "macos")]
fn sock_path() -> String {
    let home = meridian_core::paths::home_dir_or_cwd();
    home.join(".meridian/daemon.sock")
        .to_string_lossy()
        .into_owned()
}

/// launchd service target, `gui/<uid>/com.meridiona.daemon`.
#[cfg(target_os = "macos")]
fn launchd_target() -> String {
    format!("gui/{}/com.meridiona.daemon", crate::sys::uid_str())
}

#[cfg(target_os = "macos")]
pub async fn restart() -> Result<(), String> {
    launchctl(&["kickstart", "-k", &launchd_target()], "kickstart").await
}

#[cfg(target_os = "macos")]
pub async fn set_running(running: bool) -> Result<(), String> {
    let verb = if running { "start" } else { "stop" };
    launchctl(&[verb, &launchd_target()], verb).await
}

/// SIGHUP the daemon so it exits cleanly and launchd relaunches it, picking up
/// startup-only settings (OTLP config, credentials). Returns the pid signalled.
#[cfg(target_os = "macos")]
pub async fn reload() -> Result<u32, String> {
    let probe = probe().await;
    let Some(pid) = probe.pid.filter(|_| probe.running) else {
        return Err("daemon not running".to_string());
    };
    let status = std::process::Command::new("kill")
        .args(["-HUP", &pid.to_string()])
        .status()
        .map_err(|e| format!("kill failed: {e}"))?;
    if !status.success() {
        return Err(format!("kill -HUP {pid} returned non-zero"));
    }
    Ok(pid)
}

#[cfg(target_os = "macos")]
async fn launchctl(args: &[&str], what: &str) -> Result<(), String> {
    let status = std::process::Command::new("launchctl")
        .args(args)
        .status()
        .map_err(|e| format!("launchctl failed: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("launchctl {what} returned non-zero"))
    }
}

// ── Windows: named pipe + Task Scheduler ────────────────────────────────────

// Suppresses the console-window flash on the schtasks / direct-daemon spawns
// below; no-op on other platforms, so kept behind the same cfg as its callers.
#[cfg(target_os = "windows")]
use meridian_core::proc_ext::NoWindow;

#[cfg(target_os = "windows")]
pub async fn probe() -> Probe {
    use std::time::Duration;
    use tokio::io::AsyncReadExt;
    use tokio::net::windows::named_pipe::ClientOptions;
    use tokio::time::timeout;

    // The pipe name must match the daemon's own (meridian::platform's Windows
    // arm) exactly, per-user suffix included, or the probe never finds it.
    let user = std::env::var("USERNAME").unwrap_or_else(|_| "default".to_string());
    let name = format!(r"\\.\pipe\meridian-daemon-{user}");

    let Ok(mut client) = ClientOptions::new().open(&name) else {
        return Probe::down();
    };
    let mut buf = Vec::new();
    let _ = timeout(Duration::from_millis(800), client.read_to_end(&mut buf)).await;
    parse_greeting(&buf)
}

/// Task Scheduler name for the daemon's per-user login task. Must match the one
/// `backend_install::register_service` creates.
#[cfg(target_os = "windows")]
const TASK_NAME: &str = "Meridian Daemon";

#[cfg(target_os = "windows")]
pub async fn restart() -> Result<(), String> {
    // Task Scheduler has no atomic restart, so end-then-run. `/End` failing is
    // fine (the task may not be running); only `/Run` failing is an error.
    let _ = schtasks(&["/End", "/TN", TASK_NAME]).await;
    match schtasks(&["/Run", "/TN", TASK_NAME]).await {
        Ok(()) => Ok(()),
        Err(e) => {
            // `/Run` fails outright — not "task exists but isn't running" — on
            // any machine where `schtasks /Create` was blocked by policy at
            // install time (see `backend_install::register_service`): there is
            // no "Meridian Daemon" task to run, only the Startup-folder VBScript
            // fallback, which fires once at login and never again. Without this,
            // a daemon that later crashes on one of those machines stays dead
            // until the next login — restart() would report success on nothing.
            // Spawning the staged binary directly is the same recovery
            // `register_service` already does for a fresh install; the daemon's
            // own single-instance guard (`daemon_already_running`) makes this
            // safe to attempt even if the task turns out to still be alive.
            tracing::warn!(
                error = %e,
                "schtasks /Run failed - falling back to spawning the staged daemon directly"
            );
            spawn_staged_daemon().map_err(|spawn_err| {
                format!("schtasks /Run failed ({e}); direct spawn also failed: {spawn_err}")
            })
        }
    }
}

/// Launch `~/.meridian/bin/meridian.exe` directly, bypassing the Task
/// Scheduler entirely. Used only as [`restart`]'s fallback when no scheduled
/// task exists to run.
#[cfg(target_os = "windows")]
fn spawn_staged_daemon() -> Result<(), String> {
    let home = meridian_core::paths::home_dir()
        .ok_or_else(|| "home directory could not be resolved".to_string())?;
    let daemon_bin = home.join(".meridian").join("bin").join("meridian.exe");
    if !daemon_bin.exists() {
        return Err(format!("no staged daemon at {}", daemon_bin.display()));
    }
    std::process::Command::new(&daemon_bin)
        .no_window()
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("spawn {} failed: {e}", daemon_bin.display()))
}

#[cfg(target_os = "windows")]
pub async fn set_running(running: bool) -> Result<(), String> {
    if running {
        schtasks(&["/Run", "/TN", TASK_NAME]).await
    } else {
        schtasks(&["/End", "/TN", TASK_NAME]).await
    }
}

/// The Windows analogue of the macOS SIGHUP reload.
///
/// Most settings are re-read by the daemon on every poll tick, so a live
/// change applies within one interval with no action here. For the
/// startup-only settings that macOS handles by making launchd relaunch the
/// process, the equivalent is to restart the scheduled task — which is exactly
/// [`restart`]. Returns the pid the daemon reported before the restart, purely
/// so the command's response shape matches macOS.
#[cfg(target_os = "windows")]
pub async fn reload() -> Result<u32, String> {
    let probe = probe().await;
    let Some(pid) = probe.pid.filter(|_| probe.running) else {
        return Err("daemon not running".to_string());
    };
    restart().await?;
    Ok(pid)
}

#[cfg(target_os = "windows")]
async fn schtasks(args: &[&str]) -> Result<(), String> {
    let out = tokio::process::Command::new("schtasks")
        .args(args)
        .no_window()
        .output()
        .await
        .map_err(|e| format!("schtasks failed: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "schtasks {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn greeting_parses_pid_and_rejects_junk() {
        let p = parse_greeting(br#"{"running":true,"pid":4242}"#);
        assert!(p.running);
        assert_eq!(p.pid, Some(4242));

        assert!(!parse_greeting(b"").running);
        assert!(!parse_greeting(b"not json").running);
    }
}
