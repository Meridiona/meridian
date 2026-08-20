//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Shared "shell out to the `meridian` CLI" helpers for tray commands.
//!
//! # What this is
//! The spawn/[`crate::install::cli_cwd`]/timeout/`parse_last_line` pattern that
//! [`crate::commands::statuses`] established and [`crate::commands::worklog_generate`]
//! copied, promoted to one module (the copies had drifted zero bytes apart — this
//! keeps it that way). Tracker auth + the chosen LLM provider are read from a `.env` /
//! `settings.json` by the CLI itself, so any command doing tracker or model work
//! spawns the CLI rather than talking to a tracker or model in-process.
//!
//! **The CWD is [`crate::install::cli_cwd`], never `~/.meridian` directly**: dotenvy
//! walks UP from the process CWD, so the CWD chooses the credentials. `cli_cwd`
//! resolves to `~/.meridian` in a release build but to the **checkout** in a debug
//! one — a dev run must depend on nothing in `~/.meridian` (see `cli_cwd` and
//! [`crate::install::meridian_bin`] for the full rationale). Hardcoding `~/.meridian`
//! here would silently hand a dev tray the installed package's credentials, which is
//! exactly the bug that resolver exists to prevent.
//!
//! [`run_meridian_json`] pairs [`run_meridian`] + [`parse_last_line`] with a
//! diagnostic that names the binary on an unparseable result (a stale installed
//! `meridian` is the usual cause).
//!
//! # Who calls this
//! [`crate::commands::statuses`], [`crate::commands::worklog_generate`],
//! [`crate::commands::llm_lab`].
//!
//! # Related
//! - [`crate::commands::triage`]'s `apply_ticket_fix` — the origin of the CWD
//!   rationale (it predates this module and keeps its own bespoke variant).

use meridian_core::proc_ext::NoWindow;
use serde::Deserialize;
use std::time::Duration;

/// Run `meridian <args…>` in [`crate::install::cli_cwd`] with stdin nulled and
/// stdout/stderr piped, under `timeout`. Returns the stdout on success, or the
/// trimmed stderr (or a status message) as `Err` on non-zero exit / timeout /
/// spawn error.
pub(crate) async fn run_meridian(
    args: &[&str],
    timeout: Duration,
    label: &str,
) -> Result<String, String> {
    let home = crate::install::cli_cwd()?;
    let bin = crate::install::meridian_bin();
    // WHICH binary ran, and from where, are the most useful facts when one of these
    // misbehaves: a release tray calls the INSTALLED meridian, which can be older
    // than the tray asking it for a subcommand, and the cwd decides which `.env`
    // (so which credentials) it got. Log both before we spawn, so a failure
    // downstream is one trace lookup rather than a guess.
    tracing::debug!(bin = %bin, cwd = %home.display(), args = ?args, "{label}: spawning");
    let child = tokio::process::Command::new(&bin)
        .args(args)
        .current_dir(&home)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        // On timeout below, `tokio::time::timeout` drops the output future; without
        // this the orphaned `meridian <args>` keeps running in the background after
        // the UI reports a failure — for an LLM-backed call like `plan-task-draft`,
        // that means the retry the user clicks next spawns a SECOND CLI process
        // racing the first one for the same provider/DB, which is how a slow draft
        // compounds into a retry loop that never finishes. See `tasks.rs::sync_tasks`
        // for the sibling fix this mirrors.
        .kill_on_drop(true)
        .no_window()
        .output();

    let output = match tokio::time::timeout(timeout, child).await {
        Err(_) => {
            // `kill_on_drop` reaps the child here, taking its stderr with it, so
            // this log (bin + cwd, matching the spawn/non-zero arms below) is the
            // only record a timeout leaves.
            tracing::warn!(
                bin = %bin,
                cwd = %home.display(),
                timeout_s = timeout.as_secs(),
                "{label}: timed out"
            );
            return Err(format!("{label} timed out"));
        }
        Ok(Err(e)) => {
            tracing::warn!(bin = %bin, error = %e, "{label} spawn failed");
            return Err(format!("spawn error: {e}"));
        }
        Ok(Ok(o)) => o,
    };

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    tracing::debug!(
        bin = %bin,
        code = ?output.status.code(),
        stdout_len = stdout.len(),
        stderr_tail = %tail(&stderr, 400),
        "{label}: finished"
    );

    if !output.status.success() {
        let msg = if stderr.is_empty() {
            format!("{label} exited {:?}", output.status.code())
        } else {
            stderr
        };
        tracing::warn!(bin = %bin, code = ?output.status.code(), "{label} non-zero: {msg}");
        return Err(msg);
    }
    Ok(stdout)
}

/// Spawn `meridian <args…>` detached — no timeout, not awaited. For work that can
/// far outlive a reasonable invoke budget (an N-variant LLM experiment); the UI
/// polls the DB for progress instead of holding the call. A background task waits
/// on the child only to log its exit (and reap it — never a zombie).
pub(crate) fn spawn_meridian_detached(args: &[String], label: &'static str) -> Result<(), String> {
    let home = crate::install::cli_cwd()?;
    let bin = crate::install::meridian_bin();
    tracing::debug!(bin = %bin, cwd = %home.display(), args = ?args, "{label}: spawning detached");
    let mut child = tokio::process::Command::new(&bin)
        .args(args)
        .current_dir(&home)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .no_window()
        .spawn()
        .map_err(|e| {
            tracing::warn!(bin = %bin, error = %e, "{label} detached spawn failed");
            format!("spawn error: {e}")
        })?;
    tokio::spawn(async move {
        match child.wait().await {
            Ok(status) if status.success() => {
                tracing::info!("{label} detached run finished");
            }
            Ok(status) => {
                tracing::warn!(code = status.code(), "{label} detached run exited non-zero");
            }
            Err(e) => tracing::warn!(error = %e, "{label} detached wait failed"),
        }
    });
    Ok(())
}

/// Parse the LAST non-empty stdout line as JSON `T` (the CLI logs before the
/// result line). Returns a bounded parse-error message on failure.
pub(crate) fn parse_last_line<T: for<'de> Deserialize<'de>>(stdout: &str) -> Result<T, String> {
    let last = stdout.lines().rfind(|l| !l.trim().is_empty());
    match last.and_then(|l| serde_json::from_str::<T>(l).ok()) {
        Some(v) => Ok(v),
        None => {
            let s = stdout.trim();
            let skip = s.chars().count().saturating_sub(200);
            let tail: String = s.chars().skip(skip).collect();
            Err(format!("could not parse result: {tail}"))
        }
    }
}

/// Last `n` chars of `s` — bounded so a runaway log line can't fill a span.
fn tail(s: &str, n: usize) -> String {
    let s = s.trim();
    s.chars()
        .skip(s.chars().count().saturating_sub(n))
        .collect()
}

/// Run `meridian <args…>` and parse its last stdout line as JSON `T` — the ONE
/// way a command should shell out for a JSON result ([`run_meridian`] +
/// [`parse_last_line`] were being paired by hand at every call site, and the
/// pairing is where the diagnostics went missing).
///
/// # Why the error names the binary
/// The tray spawns the INSTALLED `meridian`, which is versioned independently of
/// the tray. Ask a stale one for a subcommand it doesn't have and (before the
/// guard in `main.rs`) it ignored the argv and booted a daemon, which the
/// single-instance guard killed with a warning on stdout — surfacing to the user
/// as "could not parse result: …daemon.sock", a message about a socket, naming
/// neither the binary nor the subcommand. So a parse failure here reports WHICH
/// binary produced the unparseable output, and logs the whole of it at `error`
/// for the trace.
pub(crate) async fn run_meridian_json<T: for<'de> Deserialize<'de>>(
    args: &[&str],
    timeout: Duration,
    label: &str,
) -> Result<T, String> {
    let stdout = run_meridian(args, timeout, label).await?;
    match parse_last_line::<T>(&stdout) {
        Ok(v) => Ok(v),
        Err(_) => {
            let bin = crate::install::meridian_bin();
            tracing::error!(
                bin = %bin,
                args = ?args,
                stdout = %tail(&stdout, 2000),
                "{label}: output was not JSON - is this binary older than the tray?"
            );
            Err(format!(
                "{label}: {bin} returned no result. It may be older than this app - reinstall or rebuild it."
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::process::Stdio;
    use std::time::Duration;

    /// Regression guard: on a timeout, `tokio::time::timeout` drops the
    /// `.output()` future, and without `kill_on_drop(true)` the spawned
    /// `meridian <args>` keeps running in the background after the caller has
    /// already reported failure to the user. For an LLM-backed call like
    /// `plan-task-draft`, that orphan then competes with the next "Try again"
    /// click's fresh process for the same provider/DB — a plausible reason a
    /// draft that missed its 150s budget once keeps missing it on retry.
    ///
    /// This can't drive `run_meridian` itself as a real spawn-and-verify test:
    /// it resolves its binary via `crate::install::meridian_bin()`, and
    /// overriding that via `MERIDIAN_BIN` would mean `std::env::set_var` on a
    /// shared test binary — exactly what `integrations.rs`'s "avoiding
    /// `std::env::set_var` on a Tokio worker thread" note warns off. So this
    /// is source-scanned, mirroring `tasks.rs::sync_tasks`, the sibling call
    /// site that already carries this fix. The MECHANISM itself — that
    /// `kill_on_drop(true)` actually terminates an orphaned child, on this
    /// platform, with tokio's real process reaping — is verified separately
    /// below, against a plain `tokio::process::Command` that needs no
    /// `MERIDIAN_BIN` override at all.
    #[test]
    fn run_meridian_kills_the_child_on_timeout() {
        let src = include_str!("cli_exec.rs");
        let prod = src.split_once("\n#[cfg(test)]").map_or(src, |(a, _)| a);
        let spawn = prod
            .find("tokio::process::Command::new(&bin)")
            .expect("run_meridian's Command builder moved or was renamed");
        let output_call = prod[spawn..]
            .find(".output();")
            .expect("run_meridian's Command builder no longer ends in .output()");
        let builder = &prod[spawn..spawn + output_call];
        assert!(
            builder.contains(".kill_on_drop(true)"),
            "run_meridian's spawned child is missing .kill_on_drop(true) — a \
             timeout will orphan it instead of killing it. Builder was: {builder}"
        );
    }

    /// A process that runs far longer than the timeout below, so the timeout
    /// always wins the race — the exact shape `run_meridian` puts its child
    /// in. No dependency on `meridian`/`MERIDIAN_BIN`: `sleep`/`ping` are
    /// present on every macOS and Windows runner this crate's tests run on
    /// (see `.github/workflows/ci.yml`'s `windows-latest` + `macos-latest`
    /// `cargo test --workspace` jobs).
    #[cfg(unix)]
    fn long_running_command() -> (&'static str, &'static [&'static str]) {
        ("sleep", &["30"])
    }
    #[cfg(windows)]
    fn long_running_command() -> (&'static str, &'static [&'static str]) {
        // `timeout.exe` refuses to run with stdin redirected (no console
        // handle) — `ping` to loopback is the standard "sleep N seconds"
        // substitute on Windows and needs no real network.
        ("ping", &["-n", "30", "127.0.0.1"])
    }

    #[cfg(unix)]
    fn process_is_alive(pid: u32) -> bool {
        std::process::Command::new("ps")
            .args(["-p", &pid.to_string()])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }
    #[cfg(windows)]
    fn process_is_alive(pid: u32) -> bool {
        // `tasklist` exits 0 either way, printing "No tasks are running..."
        // when nothing matches — the pid has to actually appear in the
        // output, not just a success status.
        std::process::Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH"])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).contains(&pid.to_string()))
            .unwrap_or(false)
    }

    /// Proves the MECHANISM `run_meridian_kills_the_child_on_timeout` pins the
    /// wiring for: that `kill_on_drop(true)` on a `tokio::process::Command`
    /// actually terminates the child once the future racing it is dropped —
    /// i.e. that this fix does what its own reasoning claims, not just that
    /// the flag is textually present.
    #[tokio::test]
    async fn kill_on_drop_actually_terminates_the_orphaned_child() {
        let (bin, args) = long_running_command();
        let child = tokio::process::Command::new(bin)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .expect("failed to spawn the long-running test process");
        let pid = child.id().expect("a just-spawned child must have a pid");
        assert!(
            process_is_alive(pid),
            "test bug: process not observed alive right after spawn"
        );

        {
            // Mirrors `run_meridian` exactly: race the child's `.output()`
            // against a timeout far shorter than the process's own runtime.
            let output_fut = child.wait_with_output();
            let result = tokio::time::timeout(Duration::from_millis(50), output_fut).await;
            assert!(
                result.is_err(),
                "test bug: the process exited before the timeout could fire"
            );
            // `output_fut` (and the `Child` it consumed) drops here — exactly
            // what happens when `tokio::time::timeout` drops `run_meridian`'s
            // `.output()` future on a real timeout.
        }

        // `kill_on_drop`'s kill is fired from `Drop`, not awaited to
        // completion — poll briefly rather than asserting instantaneously.
        let mut still_alive = process_is_alive(pid);
        for _ in 0..20 {
            if !still_alive {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
            still_alive = process_is_alive(pid);
        }
        assert!(
            !still_alive,
            "child (pid {pid}) is still running ~2s after being dropped — \
             kill_on_drop did not terminate it"
        );
    }
}
