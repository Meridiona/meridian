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

/// The status verdict: does a probe result plus the OS's process view add up
/// to "running"? Pure and cfg-free so the semantics are pinned by a unit test
/// on every platform (`a_probe_miss_with_a_live_process_still_reports_running`).
///
/// `process_alive = None` (the OS query itself failed) deliberately does NOT
/// rescue a probe miss - inventing an up daemon nothing confirmed would hide a
/// real outage. Only a positive "the process is there" overrides.
fn status_running(probe_ok: bool, process_alive: Option<bool>) -> bool {
    probe_ok || process_alive == Some(true)
}

/// [`probe`] with the watchdog's second opinion, for STATUS REPORTING - the
/// popover health panel and the dashboard's capture badge.
///
/// A daemon that is alive but slow to greet misses the probe's 800ms budget:
/// measured live on 1.83.2-staging.1, both misses landed inside stretched ETL
/// ticks (89s and 164s against the 60s cadence), when the greeting-write task
/// starves behind the run. The popover then flapped "Daemon: Not running"
/// next to a Restart button while capture ran fine - inviting exactly the
/// mid-write SIGTERM the watchdog fix (#678) closed, whose storms this same
/// alive-but-busy false negative drove. A probe miss here only reads as down
/// once `process_alive` says the process is not there.
///
/// The WATCHDOG keeps calling the bare [`probe`] on purpose: its `decide`
/// takes `probe_ok` and `process_alive` as separate inputs, and folding the
/// second opinion into the probe would silently double-apply it there.
pub async fn status() -> Probe {
    let p = probe().await;
    if p.running {
        return p;
    }
    if status_running(false, process_alive().await) {
        tracing::debug!(
            "daemon status: probe missed but the process is alive - reporting running (probe race, not an outage)"
        );
        // The greeting never arrived, so the pid is unknown from this path.
        return Probe {
            running: true,
            pid: None,
        };
    }
    p
}

// ── macOS: Unix domain socket + launchd ─────────────────────────────────────

#[cfg(target_os = "macos")]
pub async fn probe() -> Probe {
    probe_path(&sock_path()).await
}

/// [`probe`] against an explicit socket path. Split out so the mechanism
/// test can point it at its own servers -
/// `a_busy_daemon_delaying_its_greeting_reads_as_down` reproduces the
/// alive-but-slow-to-greet miss that is the entire premise of [`status`].
#[cfg(target_os = "macos")]
async fn probe_path(sock_path: &str) -> Probe {
    use std::time::Duration;
    use tokio::io::AsyncReadExt;
    use tokio::net::UnixStream;
    use tokio::time::timeout;

    let Ok(Ok(mut stream)) =
        timeout(Duration::from_millis(800), UnixStream::connect(sock_path)).await
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

/// Start the daemon if it is stopped, **without ever signalling a running
/// instance** — `launchctl kickstart` with no `-k`.
///
/// Per `man launchctl`, `-k` is precisely and only what "kill[s] the running
/// instance before restarting". Dropping it is what makes this safe to call
/// speculatively: on an already-running daemon it is a no-op instead of a
/// SIGTERM mid-WAL-write. [`restart`] keeps `-k` because its callers are asking
/// for a restart explicitly; the watchdog must not.
///
/// See [`crate::poll::watchdog`] for the corruption incident this exists to
/// prevent.
#[cfg(target_os = "macos")]
pub async fn start_if_stopped() -> Result<(), String> {
    launchctl(&["kickstart", &launchd_target()], "kickstart").await
}

/// Whether the daemon **process** exists, regardless of whether it answered its
/// endpoint. `None` means "could not determine".
///
/// Asks launchd rather than scanning the process table, so this reports on the
/// service the tray actually manages rather than any binary that happens to
/// share a name.
#[cfg(target_os = "macos")]
pub async fn process_alive() -> Option<bool> {
    let target = launchd_target();
    let out = tokio::process::Command::new("launchctl")
        .args(["print", &target])
        .output()
        .await
        .ok()?;
    if out.status.success() {
        return Some(parse_launchctl_pid(&String::from_utf8_lossy(&out.stdout)).is_some());
    }
    // Not every non-zero exit means the same thing, and collapsing them all to
    // "not alive" is how a guard against killing a live daemon quietly stops
    // guarding. `113` (EBADARCH / "Could not find service") is launchd's answer
    // for a label that is not loaded — a definite, actionable "not alive", and
    // the state `stop_daemon_for_migration` deliberately leaves behind. Any
    // other failure (launchd busy, a transient system error) tells us nothing
    // about the daemon, so report `None` = unknown rather than inventing a
    // negative.
    let code = out.status.code();
    if code == Some(LAUNCHCTL_NO_SUCH_SERVICE) {
        return Some(false);
    }
    tracing::warn!(
        target = %target,
        exit_code = code.unwrap_or(-1),
        "launchctl print failed for a reason other than 'no such service' - daemon liveness is unknown"
    );
    None
}

/// launchd's exit status for `print` against a label that is not loaded.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
const LAUNCHCTL_NO_SUCH_SERVICE: i32 = 113;

/// Windows: no launchd equivalent is wired, so liveness is unknown.
///
/// Returning `None` ("cannot tell") preserves the pre-existing Windows
/// behaviour exactly rather than silently disabling the watchdog's backstop
/// there. The corruption this guard closes is macOS-only (code 11 was 100%
/// macOS, 0% Windows across the fleet), and the other two guards — no
/// kill-before-start, and the storm cap — apply on both platforms.
#[cfg(not(target_os = "macos"))]
pub async fn process_alive() -> Option<bool> {
    None
}

/// Start the daemon if it is stopped, without ending a running instance.
///
/// The Windows counterpart of the macOS [`start_if_stopped`]: deliberately
/// omits the `/End` that [`restart`] performs first, but otherwise takes the
/// identical path — including the direct-spawn fallback for machines with no
/// scheduled task. See [`run_task_or_spawn`].
///
/// Re-running an already-running daemon is harmless: the daemon's own
/// single-instance guard (`daemon_already_running`) makes the second process
/// exit immediately.
#[cfg(target_os = "windows")]
pub async fn start_if_stopped() -> Result<(), String> {
    run_task_or_spawn().await
}

/// The `pid = N` line out of `launchctl print`'s output, if the service has one.
///
/// A loaded-but-stopped service prints a record with **no** `pid` field, which
/// is exactly the case the watchdog's backstop is for — so the absence of the
/// line is meaningful, not a parse failure.
///
/// Kept free of `cfg` so it compiles and tests on every target, including the
/// Windows CI job. `cfg`-gated helpers in this tray have twice broken that
/// build in ways invisible from macOS.
///
/// Only the macOS [`process_alive`] calls it, so a non-macOS build sees dead
/// code and `-D warnings` fails it. `cfg_attr` rather than `cfg`: the latter
/// would delete the function on Windows and take its test with it, which is the
/// whole reason it is written portably. Same pattern as
/// [`classify_run_failure`] below.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn parse_launchctl_pid(output: &str) -> Option<u32> {
    output.lines().find_map(|line| {
        let (key, value) = line.split_once('=')?;
        (key.trim() == "pid").then(|| value.trim().parse().ok())?
    })
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

/// Why a `schtasks /Run` failed, which decides how loudly [`restart`] reports it.
/// Both variants still fall back to the direct spawn — only the reporting differs.
#[derive(Debug, PartialEq, Eq)]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
enum RunFallback {
    /// No such task. On a machine where `schtasks /Create` was blocked by policy
    /// at install time this is permanent and by design — the Startup-folder
    /// launcher is the registered mechanism — so it is not a fault to report.
    NoTaskExpected,
    /// The task exists but would not run. Genuinely unexpected: the direct spawn
    /// is papering over something real, so it stays visible.
    TaskExistsUnexpected,
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
impl RunFallback {
    /// Whether this deserves WARN (and therefore egress to central telemetry)
    /// rather than DEBUG.
    fn is_noteworthy(&self) -> bool {
        matches!(self, RunFallback::TaskExistsUnexpected)
    }
}

/// Classify a failed `/Run` from whether `schtasks /Query` found the task.
///
/// Split out from [`restart`] so the decision is testable: `restart` itself is
/// `cfg(target_os = "windows")` and unreachable from any check a macOS developer
/// runs, which is how the reporting bug this fixes went unnoticed.
///
/// Keyed on `/Query`'s **exit code**, never on `/Run`'s stderr text: Windows
/// localizes those messages ("The system cannot find the file specified" is
/// English-only), so matching text would misclassify every non-English machine
/// as the unexpected case — the population most likely to be policy-locked to
/// begin with.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn classify_run_failure(task_exists: bool) -> RunFallback {
    if task_exists {
        RunFallback::TaskExistsUnexpected
    } else {
        RunFallback::NoTaskExpected
    }
}

#[cfg(target_os = "windows")]
pub async fn restart() -> Result<(), String> {
    // Task Scheduler has no atomic restart, so end-then-run. `/End` failing is
    // fine (the task may not be running); only `/Run` failing is an error.
    let _ = schtasks(&["/End", "/TN", TASK_NAME]).await;
    run_task_or_spawn().await
}

/// `schtasks /Run`, falling back to spawning the staged daemon directly.
///
/// Shared by [`restart`] and [`start_if_stopped`] so the two cannot diverge.
/// Splitting this out fixes a real regression: when the watchdog and
/// `refresh_health` switched from `restart()` to `start_if_stopped()` (to stop
/// killing healthy daemons — see [`crate::poll::watchdog`]), the Windows
/// `start_if_stopped` was a bare `schtasks /Run`. On a machine where
/// `schtasks /Create` was blocked by policy at install time there IS no task to
/// run, so **both** automatic recovery paths silently lost their only way back
/// up, and a crashed daemon would have stayed dead until the next login.
#[cfg(target_os = "windows")]
async fn run_task_or_spawn() -> Result<(), String> {
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
            //
            // Which of those two situations this is decides the log level, and
            // the difference matters more than it looks. On a policy-locked
            // machine the task will NEVER exist, so warning here reports a
            // permanent, already-handled configuration as if it were a fresh
            // transient failure — once per restart attempt, forever. WARN+ is
            // what egresses to central telemetry, and one such machine shipped
            // ~3.9k of these in three days, burying real failures in the stream
            // that exists to surface them.
            //
            // See `classify_run_failure` for why this asks `/Query` rather than
            // reading `/Run`'s stderr.
            let task_exists = schtasks(&["/Query", "/TN", TASK_NAME]).await.is_ok();
            if classify_run_failure(task_exists).is_noteworthy() {
                tracing::warn!(
                    error = %e,
                    "schtasks /Run failed though the task exists - falling back to spawning the staged daemon directly"
                );
            } else {
                tracing::debug!(
                    error = %e,
                    "no scheduled task on this machine (schtasks /Create was blocked at install) - spawning the staged daemon directly"
                );
            }
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
    // `current_dir` is load-bearing, not cosmetic: the daemon resolves its
    // `.env` relative to the working directory, so a bare spawn inherits the
    // tray's and the daemon comes up without MERIDIAN_DB_KEY — fatal once
    // meridian.db is encrypted. Matches every other daemon launch path (see
    // `crate::backend_install`'s module docs).
    std::process::Command::new(&daemon_bin)
        .current_dir(home.join(".meridian"))
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

    /// The status surfaces (popover "Daemon: Not running", the dashboard's
    /// PAUSED badge) flapped on 1.83.2-staging.1 while the daemon ran
    /// untouched: alive but slow to greet during stretched ETL ticks, it
    /// missed the 800ms budget - exactly the false negative that drove the
    /// watchdog's restart storm - and the popover pairs the false "Not
    /// running" with a Restart button, inviting the user to SIGTERM a healthy
    /// daemon mid-write by hand. Status reporting must therefore apply the
    /// watchdog's same second opinion: a probe miss only reads as down once
    /// the OS says the process is not there.
    #[test]
    fn a_probe_miss_with_a_live_process_still_reports_running() {
        // THE regression: probe timed out, launchctl says the pid exists.
        assert!(status_running(false, Some(true)));
        // A probe miss with the process confirmed gone is genuinely down.
        assert!(!status_running(false, Some(false)));
        // No second opinion available - stay with the probe's answer rather
        // than invent an up daemon nothing confirmed.
        assert!(!status_running(false, None));
        // A successful probe never needs the second opinion.
        assert!(status_running(true, None));
        assert!(status_running(true, Some(false)));
    }

    /// Executable proof of the failure mode behind the 1.83.2-staging.1
    /// status flapping: a daemon that is ALIVE but slow to greet - measured
    /// live, both probe failures landed inside stretched ETL ticks (89s and
    /// 164s against the 60s cadence), when the daemon's greeting-write task
    /// starves behind the run - reads as down to the bare probe, because the
    /// greeting genuinely does not arrive within the 800ms budget.
    ///
    /// Why it is specifically the DAEMON's delay and not the tray's: tokio's
    /// `Timeout` polls the inner future before the deadline on every wake, so
    /// a greeting that has already arrived rescues an arbitrarily-late tray.
    /// (The first version of this test starved the tray's runtime against a
    /// prompt server and the probe SURVIVED - that disproof is why this test
    /// models the delay server-side.) Only data that truly is not there yet
    /// can lose the race.
    ///
    /// The control asserts a prompt greeting probes UP - the probe itself is
    /// not broken - and the last assertion ties the miss to the fix: with the
    /// OS's "process is alive" second opinion it must read as running, never
    /// as "Not running" next to a Restart button.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_busy_daemon_delaying_its_greeting_reads_as_down() {
        use tokio::io::AsyncWriteExt as _;

        async fn serve(path: &std::path::Path, delay_ms: u64) {
            let listener = tokio::net::UnixListener::bind(path).expect("bind");
            tokio::spawn(async move {
                loop {
                    let Ok((mut s, _)) = listener.accept().await else {
                        continue;
                    };
                    tokio::spawn(async move {
                        tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
                        let _ = s.write_all(br#"{"running":true,"pid":1}"#).await;
                    });
                }
            });
        }

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        rt.block_on(async {
            let dir = tempfile::tempdir().expect("tempdir");

            // Control: a prompt greeting probes UP, so the down verdict below
            // is the delay's doing, not the probe's.
            let prompt = dir.path().join("prompt.sock");
            serve(&prompt, 0).await;
            assert!(
                probe_path(&prompt.to_string_lossy()).await.running,
                "a prompt daemon must probe up - the probe itself is broken"
            );

            // The live failure mode: connect succeeds instantly (the backlog
            // accepts), but the greeting lands 900ms later - past the read
            // budget - exactly a daemon stalled mid-ETL.
            let busy = dir.path().join("busy.sock");
            serve(&busy, 900).await;
            let p = probe_path(&busy.to_string_lossy()).await;
            assert!(
                !p.running,
                "the bare probe must miss a greeting that arrives after its \
                 budget - if this passes, the budget grew and status() may be \
                 double-guarding"
            );

            // The fix: the OS says the process exists, so the miss must not
            // reach the user as an outage.
            assert!(status_running(p.running, Some(true)));
        });
    }

    /// The two status surfaces must never regress to the bare probe - that
    /// regression IS the flapping bug, and nothing at the call sites would
    /// look wrong in review. `include_str!` over the consumers pins it.
    #[test]
    fn status_surfaces_use_the_second_opinion_not_the_bare_probe() {
        for (name, src) in [
            ("daemon.rs", include_str!("daemon.rs")),
            ("health.rs", include_str!("health.rs")),
        ] {
            assert!(
                src.contains("daemon_control::status()"),
                "{name}: status surfaces must call daemon_control::status()"
            );
            assert!(
                !src.contains("daemon_control::probe()"),
                "{name}: bare daemon_control::probe() reintroduces the false \
                 'Not running' flap (and its Restart-button invitation to \
                 SIGTERM a healthy daemon) - use status() instead"
            );
        }
    }

    /// The liveness signal the watchdog gates on. Getting this wrong in the
    /// "running" direction re-enables the restart storm that corrupted
    /// `meridian.db`, so both directions are pinned against real
    /// `launchctl print` output.
    #[test]
    fn launchctl_pid_is_read_only_from_a_real_pid_line() {
        // Shape of a real record (tab-indented `key = value`), trimmed.
        let running =
            "\tstate = running\n\tprogram = /usr/bin/x\n\tpid = 56397\n\tjob state = running\n";
        assert_eq!(parse_launchctl_pid(running), Some(56397));

        // A loaded-but-STOPPED service prints no `pid` line at all. That is the
        // case the watchdog's backstop exists for, so it must read as absent
        // rather than as a parse failure that leaves the daemon dead.
        //
        // Verified against real `launchctl print` output rather than assumed:
        // four stopped services on macOS 15 (`launchctl list` entries with a
        // `-` pid column) each printed `state = not running` and ZERO `pid =`
        // lines. If this ever changed — a stale or `pid = 0` line on a stopped
        // service — `process_alive` would answer `Some(true)` forever and the
        // watchdog would silently become a no-op, including for the one case
        // launchd's `KeepAlive` does not cover. The fallback would be to key on
        // `state` instead of the presence of `pid`.
        let stopped = "\tstate = not running\n\tprogram = /usr/bin/x\n\tjob state = stopped\n";
        assert_eq!(parse_launchctl_pid(stopped), None);

        // Keys that merely CONTAIN "pid" must not match - `ppid` in particular
        // is present on some records and would report a dead service as alive,
        // which is the direction that silently disables the backstop.
        let ppid_only = "\tppid = 1\n\tstate = not running\n";
        assert_eq!(parse_launchctl_pid(ppid_only), None);

        // Junk must not panic or invent a pid.
        assert_eq!(parse_launchctl_pid(""), None);
        assert_eq!(parse_launchctl_pid("pid = not-a-number\n"), None);
    }

    #[test]
    fn greeting_parses_pid_and_rejects_junk() {
        let p = parse_greeting(br#"{"running":true,"pid":4242}"#);
        assert!(p.running);
        assert_eq!(p.pid, Some(4242));

        assert!(!parse_greeting(b"").running);
        assert!(!parse_greeting(b"not json").running);
    }

    /// On a machine where `schtasks /Create` was blocked by policy at install
    /// time, no "Meridian Daemon" task will EVER exist — the Startup-folder
    /// launcher is the registered mechanism. `/Run` therefore fails on every
    /// restart attempt, forever, and warning about it reports a permanent,
    /// already-handled configuration as a fresh transient fault.
    ///
    /// `WARN`+ is what egresses to central telemetry: one machine shipped ~3.9k
    /// of these in three days, out of ~6.6k fleet-wide, burying real failures in
    /// the stream that exists to surface them.
    ///
    /// Both cases must still fall back to the direct spawn — only the reporting
    /// differs. The classifier is deliberately platform-independent so this can
    /// be verified on macOS; the `restart()` that consumes it is
    /// `cfg(target_os = "windows")` and otherwise unreachable from any local
    /// check a macOS developer runs.
    #[test]
    fn a_missing_scheduled_task_is_expected_and_must_not_warn() {
        assert_eq!(
            classify_run_failure(false),
            RunFallback::NoTaskExpected,
            "no task exists (schtasks /Create was blocked at install) - this is \
             the designed fallback path and must not reach WARN"
        );
        assert!(!classify_run_failure(false).is_noteworthy());

        // The inverse keeps the classifier honest: a task that EXISTS but will
        // not run is genuinely unexpected, and the direct spawn is papering over
        // something real. That one must stay visible.
        assert_eq!(
            classify_run_failure(true),
            RunFallback::TaskExistsUnexpected,
            "the task exists but would not run - that is a real fault"
        );
        assert!(classify_run_failure(true).is_noteworthy());
    }
}
