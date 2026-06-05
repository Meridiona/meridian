// meridian — normalises screenpipe activity into structured app sessions
//
// Cursor agent startup check: detects cursor-agent CLI availability and attempts
// auto-login if installed. Runs once at daemon startup; non-fatal (falls back to
// MLX if cursor-agent is unavailable or auth fails).

use std::process::Command;

/// Check if cursor-agent is installed; attempt auto-login if found.
/// Logs suggestions if not installed or login fails.
pub async fn check_and_init() {
    match find_cursor_agent() {
        Ok(path) => {
            tracing::info!(cursor_agent_path = %path.display(), "cursor-agent found");
            match try_auto_login(&path).await {
                Ok(()) => {
                    tracing::info!("cursor-agent auto-login succeeded");
                }
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        "cursor-agent auto-login failed; run 'cursor-agent login' manually, \
                         or summariser will fall back to MLX"
                    );
                }
            }
        }
        Err(_) => {
            tracing::info!(
                "cursor-agent not found in PATH. \
                 To use your Cursor subscription for summarisation, install with: \
                 curl https://cursor.com/install -fsS | bash"
            );
        }
    }
}

/// Locate cursor-agent in PATH.
fn find_cursor_agent() -> std::io::Result<std::path::PathBuf> {
    // Try to find cursor-agent in PATH by running `which cursor-agent`.
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

/// Attempt auto-login to cursor-agent. This runs cursor-agent with no stdin,
/// hoping it auto-detects Cursor's auth. If Cursor is authenticated, this
/// should succeed; if not, the user needs to run `cursor-agent login` manually.
async fn try_auto_login(cursor_agent_path: &std::path::Path) -> anyhow::Result<()> {
    let output = tokio::task::spawn_blocking({
        let path = cursor_agent_path.to_path_buf();
        move || Command::new(&path).arg("login").output()
    })
    .await??;

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("cursor-agent login exited non-zero: {}", stderr.trim())
    }
}
