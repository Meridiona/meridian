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
        .output();

    let output = match tokio::time::timeout(timeout, child).await {
        Err(_) => {
            tracing::warn!(bin = %bin, timeout_s = timeout.as_secs(), "{label}: timed out");
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
