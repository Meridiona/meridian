// meridian — normalises screenpipe activity into structured app sessions
//
// Cursor agent lazy initialization: when a Cursor Agent session needs summarisation,
// check if cursor-agent CLI is available. If not, auto-install. Then auto-login.
// Runs on-demand (only when Cursor Agent sessions are summarised); non-fatal
// (falls back to MLX if install/login fails).

use std::process::Command;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::Duration;

// Cache state: 0 = unchecked, 1 = ready, 2 = failed
static INIT_STATE: AtomicU8 = AtomicU8::new(0);

/// Hard ceilings — the daemon runs unattended; neither the installer (network
/// fetch) nor `cursor-agent login` (may wait on a browser round-trip that no
/// one is present to complete) may hang the summariser. On timeout the init
/// fails → cached as failed → every Cursor segment falls back to MLX.
const INSTALL_TIMEOUT: Duration = Duration::from_secs(600);
const LOGIN_TIMEOUT: Duration = Duration::from_secs(120);

/// Ensure cursor-agent is installed and logged in. Call this before attempting
/// to use cursor-agent for summarisation. On first call, attempts install + login
/// if needed; subsequent calls return the cached result. Returns Ok(()) if
/// cursor-agent is ready, Err if install/login failed or timed out.
pub async fn ensure_ready() -> anyhow::Result<()> {
    match INIT_STATE.load(Ordering::Relaxed) {
        1 => return Ok(()), // Already ready
        2 => return Err(anyhow::anyhow!("cursor-agent init failed on prior attempt")),
        _ => {}
    }

    // Try install + login
    let result = try_install_and_login().await;

    // Cache the result for future calls
    let state = if result.is_ok() { 1 } else { 2 };
    INIT_STATE.store(state, Ordering::Relaxed);

    result
}

/// Main install + login flow: try to find cursor-agent in PATH; if not found,
/// auto-install; then auto-login.
async fn try_install_and_login() -> anyhow::Result<()> {
    let path = match find_cursor_agent() {
        Ok(p) => {
            tracing::info!(cursor_agent_path = %p.display(), "cursor-agent found");
            p
        }
        Err(_) => {
            tracing::info!("cursor-agent not in PATH; attempting auto-install");
            try_auto_install().await?
        }
    };

    tracing::info!("attempting cursor-agent auto-login");
    try_auto_login(&path).await?;
    tracing::info!("cursor-agent ready for summarisation");
    Ok(())
}

/// Locate cursor-agent in PATH.
fn find_cursor_agent() -> std::io::Result<std::path::PathBuf> {
    let output = Command::new("which").arg("cursor-agent").output()?;
    if output.status.success() {
        let path_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Ok(std::path::PathBuf::from(path_str))
    } else {
        Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "cursor-agent not in PATH",
        ))
    }
}

/// Auto-install cursor-agent via the official installer script. Blocks briefly;
/// should only run once per daemon lifetime (cached by ensure_ready).
async fn try_auto_install() -> anyhow::Result<std::path::PathBuf> {
    tracing::info!("running cursor-agent installer: curl https://cursor.com/install -fsS | bash");
    let output = tokio::time::timeout(
        INSTALL_TIMEOUT,
        tokio::task::spawn_blocking(|| {
            Command::new("bash")
                .arg("-c")
                .arg("curl https://cursor.com/install -fsS | bash")
                .output()
        }),
    )
    .await
    .map_err(|_| {
        anyhow::anyhow!(
            "cursor-agent install timed out after {}s",
            INSTALL_TIMEOUT.as_secs()
        )
    })???;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("cursor-agent install failed: {}", stderr.trim());
    }

    // After install, cursor-agent should be in PATH. Try to find it.
    // Allow a moment for PATH to refresh (unlikely to be needed, but safe).
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    find_cursor_agent()
        .map_err(|e| anyhow::anyhow!("cursor-agent installed but not in PATH: {}", e))
}

/// Attempt auto-login to cursor-agent. This runs cursor-agent with no stdin
/// (`Command::output()` gives the child a closed stdin), hoping it auto-detects
/// Cursor's auth from the IDE. A browser-based login flow that nobody is
/// present to complete hits LOGIN_TIMEOUT and fails the init → MLX fallback;
/// login is then deferred to a manual `cursor-agent login`.
async fn try_auto_login(cursor_agent_path: &std::path::Path) -> anyhow::Result<()> {
    let output = tokio::time::timeout(
        LOGIN_TIMEOUT,
        tokio::task::spawn_blocking({
            let path = cursor_agent_path.to_path_buf();
            move || Command::new(&path).arg("login").output()
        }),
    )
    .await
    .map_err(|_| {
        anyhow::anyhow!(
            "cursor-agent login timed out after {}s (browser flow unattended?)",
            LOGIN_TIMEOUT.as_secs()
        )
    })???;

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("cursor-agent login failed: {}", stderr.trim())
    }
}
