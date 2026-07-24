//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Cursor agent lazy initialization: when a Cursor Agent session needs
// summarisation, check that the cursor-agent CLI is available and
// authenticated. If missing, install it — but ONLY behind the explicit
// CURSOR_AGENT_AUTO_INSTALL=1 opt-in (the installer is unpinned remote code;
// a daemon must not run that as an automatic side effect). If
// unauthenticated, auto-login (status-probed, non-interactive). Runs
// on-demand (only when Cursor Agent sessions are summarised); non-fatal —
// if any step fails the Cursor row is left pending for a later drain.
//
// Field-tested 2026-06-06: `cursor-agent login` returned in ~16s when it
// could adopt the IDE's auth, but a SECOND login while already authenticated
// hung indefinitely on a browser round-trip — hence the `status` probe first
// (skip login when already authed), NO_OPEN_BROWSER on the login itself, and
// tokio's kill_on_drop so a timed-out child is reaped, not leaked (a leaked
// login child kept the one-shot CLI process alive forever).

use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::process::Command;
use tokio::sync::OnceCell;

/// One-shot init result, cached for the daemon's lifetime. `OnceCell` (not a
/// raw atomic) so concurrent first callers serialize on a single
/// `try_install_and_login` run instead of racing into duplicate installs —
/// the drain is sequential today, but the cache must not rely on that.
/// Err is stored as String because anyhow::Error is not Clone.
static INIT_RESULT: OnceCell<Result<(), String>> = OnceCell::const_new();

/// Hard ceilings — the daemon runs unattended; neither the installer (network
/// fetch) nor auth probes may hang the summariser. On timeout the init fails
/// → cached as failed → every Cursor segment is left pending for a later drain.
const INSTALL_TIMEOUT: Duration = Duration::from_secs(600);
const STATUS_TIMEOUT: Duration = Duration::from_secs(30);
const LOGIN_TIMEOUT: Duration = Duration::from_secs(120);

/// Ensure cursor-agent is installed and logged in. Call this before attempting
/// to use cursor-agent for summarisation. The first caller runs install +
/// auth; concurrent and subsequent callers get the same cached result.
pub async fn ensure_ready() -> anyhow::Result<()> {
    INIT_RESULT
        .get_or_init(|| async { try_install_and_login().await.map_err(|e| format!("{e:#}")) })
        .await
        .clone()
        .map_err(|e| anyhow::anyhow!("cursor-agent init failed: {e}"))
}

/// Main flow: find (or install) cursor-agent, then make sure it's authed —
/// `status` first (cheap, never interactive), `login` only when status says
/// unauthenticated.
async fn try_install_and_login() -> anyhow::Result<()> {
    let path = match find_cursor_agent().await {
        Ok(p) => {
            tracing::info!(cursor_agent_path = %p.display(), "cursor-agent found");
            p
        }
        Err(_) if auto_install_enabled() => {
            tracing::info!("cursor-agent not in PATH; auto-install opted in — installing");
            try_auto_install().await?
        }
        Err(_) => {
            // Running a remote install script must be an explicit user
            // decision, never an automatic daemon side effect (the installer
            // is unpinned remote code). Without the opt-in, Cursor summaries
            // stay pending. The hint is platform-specific — see
            // meridian_core::CURSOR_INSTALL_HINT's doc: a bare `curl | bash`
            // hint on Windows would point the user at a command that cannot
            // possibly work there (no bash/curl, and cursor.com/install is a
            // bash-only script even when one is on PATH via WSL/Git-Bash).
            anyhow::bail!(
                "cursor-agent not in PATH; install it (`{}`) or set \
                 CURSOR_AGENT_AUTO_INSTALL=1 to let the daemon install it",
                meridian_core::CURSOR_INSTALL_HINT,
            )
        }
    };

    if is_authenticated(&path).await {
        tracing::info!("cursor-agent already authenticated");
        return Ok(());
    }

    tracing::info!("attempting cursor-agent auto-login");
    try_auto_login(&path).await?;
    tracing::info!("cursor-agent ready for summarisation");
    Ok(())
}

/// Locate cursor-agent, the same way every other provider CLI is located.
///
/// NOT a bare `which`/`where` shell-out: `which` doesn't exist on Windows at all (the
/// process fails to even spawn, `ErrorKind::NotFound`), so this used to hard-fail on
/// every Windows machine regardless of whether cursor-agent was actually installed —
/// every Cursor Agent session was left pending forever, install or no install.
/// [`crate::llm::detect::resolve_cli`] is the shared, platform-correct probe (PATHEXT-
/// aware on Windows, login-shell + candidate dirs on Unix, and memoised) that every other
/// CLI lookup in this codebase already goes through.
async fn find_cursor_agent() -> anyhow::Result<PathBuf> {
    crate::llm::detect::resolve_cli("cursor-agent")
        .await
        .ok_or_else(|| anyhow::anyhow!("cursor-agent not in PATH"))
}

/// The auto-install opt-in: CURSOR_AGENT_AUTO_INSTALL=1|true|yes. Default OFF
/// — the daemon must not execute unverified remote code without the user
/// having explicitly turned that on.
fn auto_install_enabled() -> bool {
    std::env::var("CURSOR_AGENT_AUTO_INSTALL")
        .map(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes"))
        .unwrap_or(false)
}

/// Install cursor-agent via the official installer script (opt-in only — see
/// `auto_install_enabled`). Runs once per daemon lifetime (cached by
/// ensure_ready).
///
/// Uses the PINNED installer ([`meridian_core::CURSOR_INSTALL_CMD`]) through
/// [`crate::llm::detect::installer_command`] — the SAME platform dispatch the tray's
/// "Install" button runs (login shell on Unix, `powershell.exe` directly on Windows; see
/// that function's doc for why `bash -c`, which this used to hardcode, never worked on
/// Windows at all: no `bash` on a stock install, and even with WSL/Git-Bash's `bash.exe`
/// on PATH, `CURSOR_INSTALL_CMD`'s Windows form is PowerShell script text (`irm`/`iex`),
/// not something `bash` could run either) — so an unattended daemon install can never
/// pull a newer cursor-agent than the build this code was verified against, on any
/// platform.
async fn try_auto_install() -> anyhow::Result<PathBuf> {
    let cmd = meridian_core::CURSOR_INSTALL_CMD;
    tracing::info!(
        version = meridian_core::CURSOR_CLI_VERSION,
        "running pinned cursor-agent installer"
    );
    let output = run_with_timeout(
        &mut crate::llm::detect::installer_command(cmd),
        INSTALL_TIMEOUT,
        "cursor-agent install",
    )
    .await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("cursor-agent install failed: {}", stderr.trim());
    }

    find_cursor_agent()
        .await
        .map_err(|e| anyhow::anyhow!("cursor-agent installed but not in PATH: {}", e))
}

/// `cursor-agent status` — exit 0 + no "not logged in" marker means authed.
/// Never interactive, so a hang here is a real fault and the timeout is just
/// a backstop.
async fn is_authenticated(cursor_agent_path: &Path) -> bool {
    let output = match run_with_timeout(
        Command::new(cursor_agent_path).arg("status"),
        STATUS_TIMEOUT,
        "cursor-agent status",
    )
    .await
    {
        Ok(o) => o,
        Err(e) => {
            tracing::warn!(error = %e, "cursor-agent status probe failed");
            return false;
        }
    };
    if !output.status.success() {
        return false;
    }
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
    .to_lowercase();
    !text.contains("not logged in") && !text.contains("unauthenticated")
}

/// Attempt auto-login. NO_OPEN_BROWSER stops the CLI from popping a browser
/// tab on the user's desktop; if the IDE's auth can't be adopted
/// non-interactively the run fails (or times out) and the Cursor row stays
/// pending — login is then deferred to a manual `cursor-agent login`.
async fn try_auto_login(cursor_agent_path: &Path) -> anyhow::Result<()> {
    let output = run_with_timeout(
        Command::new(cursor_agent_path)
            .arg("login")
            .env("NO_OPEN_BROWSER", "1"),
        LOGIN_TIMEOUT,
        "cursor-agent login",
    )
    .await?;

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("cursor-agent login failed: {}", stderr.trim())
    }
}

/// Run a command with a hard timeout. `kill_on_drop` guarantees the child is
/// reaped when the timeout abandons it — a leaked child would otherwise pin
/// the process (observed: a hung `login` kept the one-shot CLI alive).
async fn run_with_timeout(
    cmd: &mut Command,
    timeout: Duration,
    label: &str,
) -> anyhow::Result<std::process::Output> {
    use meridian_core::proc_ext::NoWindow;
    cmd.stdin(std::process::Stdio::null())
        .kill_on_drop(true)
        .no_window();
    match tokio::time::timeout(timeout, cmd.output()).await {
        Ok(Ok(output)) => Ok(output),
        Ok(Err(e)) => anyhow::bail!("{label}: {e}"),
        Err(_) => anyhow::bail!("{label} timed out after {}s", timeout.as_secs()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `CURSOR_AGENT_AUTO_INSTALL` is a process-global env var and cargo runs tests in
    /// parallel threads — every test that mutates it must hold this lock, same pattern as
    /// `meridian_core::paths`'s `env_lock`.
    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
        LOCK.get_or_init(|| std::sync::Mutex::new(()))
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    fn with_auto_install<R>(value: Option<&str>, f: impl FnOnce() -> R) -> R {
        let _guard = env_lock();
        let prev = std::env::var_os("CURSOR_AGENT_AUTO_INSTALL");
        match value {
            Some(v) => std::env::set_var("CURSOR_AGENT_AUTO_INSTALL", v),
            None => std::env::remove_var("CURSOR_AGENT_AUTO_INSTALL"),
        }
        let out = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
        match prev {
            Some(v) => std::env::set_var("CURSOR_AGENT_AUTO_INSTALL", v),
            None => std::env::remove_var("CURSOR_AGENT_AUTO_INSTALL"),
        }
        match out {
            Ok(r) => r,
            Err(p) => std::panic::resume_unwind(p),
        }
    }

    /// Default OFF: an unset var must never opt a daemon into running remote installer
    /// code unattended.
    #[test]
    fn auto_install_defaults_to_disabled() {
        with_auto_install(None, || assert!(!auto_install_enabled()));
    }

    #[test]
    fn auto_install_accepts_the_documented_truthy_values() {
        for v in ["1", "true", "yes", "TRUE", "Yes", "  true  "] {
            with_auto_install(Some(v), || {
                assert!(auto_install_enabled(), "{v:?} should enable auto-install");
            });
        }
    }

    #[test]
    fn auto_install_rejects_everything_else() {
        // Common near-misses a user might reasonably type, none of which are the
        // documented opt-in spelling — must fail closed, not open.
        for v in ["0", "false", "no", "on", "", "  ", "yesplease", "TRUEE"] {
            with_auto_install(Some(v), || {
                assert!(
                    !auto_install_enabled(),
                    "{v:?} should not enable auto-install"
                );
            });
        }
    }

    /// `"1 "` (trailing space) IS accepted — `auto_install_enabled` trims before matching
    /// — documented separately from the rejection list above so that behaviour is asserted
    /// rather than silently tolerated by the `|| v == "1 "` escape hatch there.
    #[test]
    fn auto_install_trims_surrounding_whitespace() {
        with_auto_install(Some(" 1 "), || assert!(auto_install_enabled()));
        with_auto_install(Some("\tyes\n"), || assert!(auto_install_enabled()));
    }

    /// The regression this exists for: `find_cursor_agent` used to shell out to `which`,
    /// which does not exist as a binary on Windows at all — `Command::new("which")` fails
    /// to even spawn there (`ErrorKind::NotFound`), so cursor-agent was reported absent on
    /// every Windows machine regardless of whether it was actually installed. Routing
    /// through `crate::llm::detect::resolve_cli` (PATHEXT-aware, candidate-dir fallback,
    /// memoised) fixes that; this pins the observable contract without assuming whether
    /// cursor-agent happens to be installed on the machine running the test.
    #[tokio::test]
    async fn find_cursor_agent_resolves_to_a_real_path_or_a_clear_not_found_error() {
        match find_cursor_agent().await {
            Ok(p) => assert!(
                p.is_absolute() && p.exists(),
                "resolved path must be spawnable directly: {p:?}"
            ),
            Err(e) => assert_eq!(e.to_string(), "cursor-agent not in PATH"),
        }
    }
}
