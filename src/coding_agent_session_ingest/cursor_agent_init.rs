// meridian — normalises screenpipe activity into structured app sessions
//
// Cursor agent lazy initialization: when a Cursor Agent session needs
// summarisation, check that the cursor-agent CLI is available and
// authenticated. If missing, install it — but ONLY behind the explicit
// CURSOR_AGENT_AUTO_INSTALL=1 opt-in (the installer is unpinned remote code;
// a daemon must not run that as an automatic side effect). If
// unauthenticated, auto-login (status-probed, non-interactive). Runs
// on-demand (only when Cursor Agent sessions are summarised); non-fatal —
// the summariser falls back to MLX if any step fails.
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
/// → cached as failed → every Cursor segment falls back to MLX.
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
            // is `curl https://cursor.com/install | bash` — unpinned remote
            // code). Without the opt-in, Cursor summaries fall back to MLX.
            anyhow::bail!(
                "cursor-agent not in PATH; install it (`curl https://cursor.com/install -fsS | bash`) \
                 or set CURSOR_AGENT_AUTO_INSTALL=1 to let the daemon install it"
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

/// Locate cursor-agent in PATH.
async fn find_cursor_agent() -> anyhow::Result<PathBuf> {
    let output = run_with_timeout(
        Command::new("which").arg("cursor-agent"),
        STATUS_TIMEOUT,
        "which cursor-agent",
    )
    .await?;
    if output.status.success() {
        let path_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Ok(PathBuf::from(path_str))
    } else {
        anyhow::bail!("cursor-agent not in PATH")
    }
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
async fn try_auto_install() -> anyhow::Result<PathBuf> {
    tracing::info!("running cursor-agent installer: curl https://cursor.com/install -fsS | bash");
    let output = run_with_timeout(
        Command::new("bash")
            .arg("-c")
            .arg("curl https://cursor.com/install -fsS | bash"),
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
/// non-interactively the run fails (or times out) and the summariser falls
/// back to MLX — login is then deferred to a manual `cursor-agent login`.
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
    cmd.stdin(std::process::Stdio::null()).kill_on_drop(true);
    match tokio::time::timeout(timeout, cmd.output()).await {
        Ok(Ok(output)) => Ok(output),
        Ok(Err(e)) => anyhow::bail!("{label}: {e}"),
        Err(_) => anyhow::bail!("{label} timed out after {}s", timeout.as_secs()),
    }
}
