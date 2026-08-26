//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Windows daemon plumbing: a named pipe standing in for the Unix domain
//! socket, and the console-control shutdown set.
//!
//! # Why a named pipe rather than a socket file
//!
//! Windows does have `AF_UNIX` sockets now, but they carry none of the
//! properties this endpoint actually relies on: the file is not cleaned up by
//! the OS, there is no connection-refused-on-stale-file behaviour to lean on,
//! and the tray would need a filesystem path to probe. A named pipe gives the
//! semantics the single-instance guard wants for free — the name exists only
//! while a server holds it, so a crashed daemon leaves nothing behind to
//! mistake for a live one.
//!
//! That difference is why [`release_endpoint`] is a no-op here and why there
//! is no stale-endpoint unlink: both are Unix-file concerns that do not exist.
//!
//! # Per-user, not machine-wide
//!
//! The pipe name is suffixed with the user's name. The guard's job is "is a
//! daemon already serving *this* data directory", and the data directory is
//! per-user (`%USERPROFILE%\.meridian`) — so on a multi-user machine two
//! different users running Meridian are not in conflict and must not block
//! each other. A bare machine-wide name would make the second user's daemon
//! silently refuse to start.
//!
//! # Related
//! - [`super`] — the shared contract and why this is a module rather than `cfg`.
//! - `super::unix` — the domain-socket counterpart.

use std::time::Duration;

use tokio::net::windows::named_pipe::{ClientOptions, ServerOptions};

/// Matches the Unix probe budget; see that module for the reasoning.
const PROBE_TIMEOUT: Duration = Duration::from_millis(800);

/// `\\.\pipe\meridian-daemon-<user>` — see the module docs on why this is
/// per-user rather than machine-wide.
fn endpoint() -> String {
    let user = std::env::var("USERNAME").unwrap_or_else(|_| "default".to_string());
    format!(r"\\.\pipe\meridian-daemon-{user}")
}

/// The endpoint in a form suitable for a log field.
pub fn endpoint_display() -> String {
    endpoint()
}

/// Is a live daemon already serving this data directory?
///
/// A named pipe name exists only while a server holds it, so a failed open
/// already means "nobody is there" — but the greeting is still required, to
/// keep the contract identical to the Unix arm and to avoid treating some
/// unrelated process that happened to create the name as our daemon.
pub async fn daemon_already_running() -> bool {
    use tokio::io::AsyncReadExt as _;
    use tokio::time::timeout;

    let Ok(mut client) = ClientOptions::new().open(endpoint()) else {
        return false; // no server holds the name — safe to take over
    };
    let mut buf = Vec::new();
    let _ = timeout(PROBE_TIMEOUT, client.read_to_end(&mut buf)).await;
    !buf.is_empty()
}

/// Create the pipe and serve the greeting to every caller, forever.
///
/// A named pipe server instance handles exactly one client, so the loop
/// creates the next instance *before* handing the current one off — otherwise
/// there is a window where the name does not exist and a concurrent probe
/// would wrongly conclude no daemon is running.
pub fn spawn_health_listener() -> anyhow::Result<()> {
    use tokio::io::AsyncWriteExt as _;

    let name = endpoint();
    // `first_pipe_instance` makes creation fail if the name is already held,
    // which is the same "someone else got here first" signal the Unix bind
    // gives us.
    let mut server = ServerOptions::new()
        .first_pipe_instance(true)
        .create(&name)?;

    tokio::spawn(async move {
        loop {
            if let Err(e) = server.connect().await {
                tracing::warn!(error = %e, "daemon pipe connect error");
                break;
            }
            // Stand up the next instance before serving this one, so the name
            // is continuously available to probes.
            let next = match ServerOptions::new().create(&name) {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!(error = %e, "daemon pipe re-create error");
                    break;
                }
            };
            let mut connected = std::mem::replace(&mut server, next);
            let msg = super::greeting();
            tokio::spawn(async move {
                let _ = connected.write_all(msg.as_bytes()).await;
                // The client reads to EOF, so the write side must close.
                let _ = connected.shutdown().await;
            });
        }
    });
    Ok(())
}

/// No-op: a named pipe disappears with the process that holds it, so unlike a
/// socket file there is nothing to unlink.
pub fn release_endpoint() {}

/// Windows has no service integration for the daemon yet, so this reports
/// [`super::ServiceStatus::Unknown`] rather than guessing.
///
/// This is deliberate, not a stub left to rot. Nothing registers the daemon
/// with the Service Control Manager yet — that lands with the installer, since
/// querying a service is only meaningful once something creates it, and a
/// query written against a service definition that does not exist yet would be
/// unverifiable. Returning `NotRunning` in the meantime would make
/// `meridian doctor` print a confident CRITICAL about a service the product
/// has never installed, which is worse than admitting the gap.
///
/// When the SCM integration lands, this becomes a real `OpenService` +
/// `QueryServiceStatus` call and the health check below starts reporting it
/// with no change at the call site.
pub fn service_status(_label: &str) -> super::ServiceStatus {
    super::ServiceStatus::Unknown
}

/// See [`service_status`] — same reasoning: there is no manifest to inspect
/// because nothing installs one yet. A Windows service has no plist
/// equivalent in any case; its definition lives in the SCM database.
pub fn service_manifest(_label: &str) -> super::ServiceManifest {
    super::ServiceManifest::Unknown
}

/// Every running process's full command line, via PowerShell's CIM provider.
///
/// There is no `ps` on Windows, and `tasklist` reports image names without
/// arguments — useless here, since the caller has to tell an interactive
/// `claude` from the summariser's own `claude -p` subprocess, and a
/// `cursor-agent login` from a real chat session. `Win32_Process.CommandLine`
/// is the one readily available source that carries arguments.
///
/// `wmic` would be the lighter call but is deprecated and absent from current
/// Windows builds, so PowerShell it is. The ~200-500 ms startup cost is
/// irrelevant at this call site: the indexer ticks every 10 minutes.
///
/// Same contract as the Unix arm — `None` means "could not tell", which the
/// caller treats as every-CLI-running and defers to the idle backstop, so a
/// failure here can never seal a live session early.
pub async fn list_process_argvs() -> Option<Vec<String>> {
    use meridian_core::proc_ext::NoWindow;
    let mut cmd = tokio::process::Command::new("powershell");
    cmd.args([
        "-NoProfile",
        "-NonInteractive",
        "-Command",
        "Get-CimInstance Win32_Process | ForEach-Object { $_.CommandLine }",
    ])
    .kill_on_drop(true)
    .no_window();
    // Longer than the Unix budget: PowerShell start-up plus a CIM query is
    // genuinely slower than `ps`, and this runs once every ten minutes.
    let output = tokio::time::timeout(std::time::Duration::from_secs(15), cmd.output())
        .await
        .ok()?
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(str::to_string)
            .collect(),
    )
}

/// Free space is not reported on Windows yet.
///
/// `None` is already the "usage unknown" path the health check handles
/// gracefully, so this degrades to an Info rung rather than a false warning.
/// Wiring `GetDiskFreeSpaceExW` would mean taking a dependency on the
/// `windows` crate for one number; deferred until something else needs it.
pub fn disk_free_gb(_path: &std::path::Path) -> Option<f64> {
    None
}

/// Resolve when the OS asks the daemon to stop.
///
/// Windows has no `SIGHUP`, so the "reload config" path has no direct
/// counterpart — settings changes are picked up by the poll loop, and a
/// service restart goes through `ctrl_shutdown`. The other two map cleanly:
/// `ctrl_c` for an interactive stop, `ctrl_close` for the console window being
/// closed, `ctrl_shutdown` for machine shutdown or a service stop.
pub async fn wait_for_shutdown() {
    use tokio::signal::windows::{ctrl_c, ctrl_close, ctrl_shutdown};

    // Panics on registration failure, matching the Unix arm: a daemon that
    // cannot be asked to stop should fail loudly, not silently ignore it.
    let mut ctrl_c = ctrl_c().expect("register Ctrl-C handler");
    let mut close = ctrl_close().expect("register console-close handler");
    let mut shutdown = ctrl_shutdown().expect("register system-shutdown handler");

    // `pid` on every arm, for the same reason as the Unix arm — see
    // `super::unix::wait_for_shutdown`.
    let pid = std::process::id() as i64;
    tokio::select! {
        _ = ctrl_c.recv()   => tracing::info!(pid, "Ctrl-C received"),
        _ = close.recv()    => tracing::info!(pid, "console close received"),
        _ = shutdown.recv() => tracing::info!(pid, "system shutdown received"),
    }
}
