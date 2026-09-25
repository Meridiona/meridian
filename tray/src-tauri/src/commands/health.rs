//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! `/api/health` GET ported to Rust — fast local health check for the status banner.
//!
//! Three checks run in parallel (FS + log scan + socket probe), matching the TS route.
//! The launchctl fallback for a11y trust also mirrors the route.
//!
//! No in-module cache — the tray's poll loop controls cadence (`do_health` every 60 s),
//! and the Tauri command is on-demand. The TS route's 15 s stale-while-revalidate was
//! only needed because multiple SSE clients hit the same Next.js server concurrently.
//!
//! # Who calls this
//! - Command: `get_health` (registered in `lib.rs`)
//! - Internal: [`crate::poll::refresh_health`] calls [`check_health`] directly,
//!   bypassing the HTTP round-trip.
//! - Frontend: `ui/components/HealthBanner.tsx` uses `/api/health/stream` (SSE) —
//!   that stream will be replaced with a Tauri event in the SSE migration phase.
//!
//! # Related
//! - [`crate::commands::daemon`] — deeper socket probe (reads daemon PID)
//! - [`crate::poll`] — schedules the tray's periodic health refresh

use serde::Serialize;

/// Response shape matching the TS route's `HealthStatus`.
#[derive(Debug, Clone, Serialize)]
pub struct HealthResponse {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub database_ready: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub daemon_running: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub a11y_helper_trusted: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Whether the user's CURRENTLY-CHOSEN LLM provider looks usable (installed + last test not
    /// failed). `Some(false)` drives the dashboard's "provider unavailable" banner — summaries
    /// are paused/degraded until it's fixed. See [`meridian::llm::detect::in_use_provider_health`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub llm_provider_ok: Option<bool>,
    /// `Some(true)` when the in-use provider is usable but RATE-LIMITED — drives a softer
    /// "catching up" notice instead of the "unavailable" alarm (it clears on its own).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub llm_provider_rate_limited: Option<bool>,
    /// Human name of the in-use provider for the banner copy (e.g. "Codex").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub llm_provider_name: Option<String>,
    /// The banner reason — the failure/"not installed" text when unavailable, or the rate-limit
    /// message when rate-limited.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub llm_provider_detail: Option<String>,
}

/// Whether a [`HealthResponse`] counts as a healthy tray: the DB is open and
/// the daemon process is running (the two signals the popover's
/// online/offline banner is actually gated on — LLM-provider availability is
/// a separate, softer banner and does not factor in here).
///
/// Shared between the poll loop's notice-owning
/// [`crate::poll::refresh_health`] and the startup fast-poll
/// ([`crate::poll::startup_health`]) so the two can never disagree about what
/// "healthy" means — this is the one place that decides it.
///
/// `database_ready` defaults to unhealthy when unknown; `daemon_running`
/// defaults to healthy (older schema compat).
pub fn is_healthy(hr: &HealthResponse) -> bool {
    hr.database_ready.unwrap_or(false) && hr.daemon_running.unwrap_or(true)
}

/// Run all three health checks in parallel and return the combined result.
/// Called by both `get_health` (Tauri command) and `poll::refresh_health` (internal).
pub async fn check_health() -> HealthResponse {
    // When the `capture` feature is enabled, a11y runs in-process inside the
    // tray (screenpipe-screen crate). No separate a11y-helper binary is needed
    // or expected — skip the trust check so the banner never fires falsely.
    #[cfg(feature = "capture")]
    let (db, daemon) = tokio::join!(check_database(), check_daemon_running());
    #[cfg(feature = "capture")]
    let trusted: Option<bool> = None;

    #[cfg(not(feature = "capture"))]
    let (db, a11y, daemon) = {
        let home = meridian_core::paths::home_dir_or_cwd();
        let home = home.to_string_lossy();
        tokio::join!(
            check_database(),
            check_a11y_trusted(&home),
            check_daemon_running(),
        )
    };
    #[cfg(not(feature = "capture"))]
    let trusted = match a11y {
        Some(v) => Some(v),
        None => launchctl_a11y_trusted().await,
    };

    let error = if !db.0 { db.1 } else { None };

    // The in-use LLM provider's availability - a cheap install probe + cached last-test read,
    // no metered call (see `in_use_provider_health`), so it's fine on the 60 s health cadence.
    let settings = meridian_core::settings::load_runtime_settings();
    let provider = meridian::llm::detect::in_use_provider_health(&settings).await;

    HealthResponse {
        database_ready: Some(db.0),
        error,
        a11y_helper_trusted: trusted,
        daemon_running: daemon,
        llm_provider_ok: Some(provider.ok),
        llm_provider_rate_limited: Some(provider.rate_limited),
        llm_provider_name: Some(provider.name),
        llm_provider_detail: provider.detail,
    }
}

/// Check whether the meridian DB is readable.
///
/// Resolves the path through [`crate::install::meridian_db_path`] — the same
/// `MERIDIAN_DB` / `~/.meridian/.env` / default chain the daemon and the tray's
/// own DB pool use — rather than re-deriving it inline. (The old inline lookup
/// read a non-existent `MERIDIAN_DB_PATH` var and the hardcoded default, so it
/// reported "not found" on any installed system with a custom `MERIDIAN_DB`.)
async fn check_database() -> (bool, Option<String>) {
    let db = crate::install::meridian_db_path();
    match tokio::fs::metadata(&db).await {
        Ok(_) => (true, None),
        Err(_) => (
            false,
            Some(format!(
                "meridian.db not found - start the daemon: {}",
                start_daemon_hint()
            )),
        ),
    }
}

/// Platform-specific instructions for manually (re-)starting the daemon —
/// used only in the health banner's "database not found" message, so a user
/// stuck on a broken autostart has something concrete to try. Must stay in
/// sync with how each platform's `backend_install::register_service` starts
/// the daemon at login.
#[cfg(target_os = "macos")]
fn start_daemon_hint() -> String {
    "launchctl load ~/Library/LaunchAgents/com.meridiona.daemon.plist".to_string()
}

// Windows shares the generic "restart Meridian" hint below rather than
// naming the "Meridian Daemon" Task Scheduler entry: `register_service`
// falls back to a Startup-folder launcher (no Task Scheduler entry at all)
// on machines where policy blocks `schtasks /Create`, so pointing at Task
// Scheduler wouldn't always apply.

#[cfg(not(target_os = "macos"))]
fn start_daemon_hint() -> String {
    "restart Meridian".to_string()
}

/// Walk the last 200 lines of `~/.meridian/logs/a11y-helper.log` for a trust
/// entry. Returns `None` when the log is absent or has no trust line yet.
#[cfg(not(feature = "capture"))]
async fn check_a11y_trusted(home: &str) -> Option<bool> {
    let log_path = format!("{}/.meridian/logs/a11y-helper.log", home);
    let content = tokio::fs::read_to_string(&log_path).await.ok()?;
    let lines: Vec<&str> = content.trim_end().split('\n').collect();
    let start = lines.len().saturating_sub(200);
    for line in lines[start..].iter().rev() {
        if line.contains("trusted: true") || line.contains("[trusted]") {
            return Some(true);
        }
        if line.contains("trusted: false") || line.contains("[untrusted]") {
            return Some(false);
        }
    }
    None
}

/// Whether a live daemon is answering its IPC endpoint.
///
/// Routes through the platform-aware probe in [`super::daemon_control`] (a Unix
/// socket on macOS, a named pipe on Windows) rather than opening a
/// `UnixStream` directly — the direct form did not compile off Unix and
/// duplicated the greeting handshake. Returns `Some(running)`; kept as an
/// `Option` to match this module's other checks, which use `None` for
/// "couldn't tell", though the probe collapses any failure to `running=false`.
///
/// [`daemon_control::status`] (probe + process-alive second opinion), NOT the
/// bare probe: this feeds the popover's "Daemon: Not running" line and its
/// Restart button, which flapped against a healthy daemon whenever the tray's
/// own load starved the 800ms probe — see `status`'s doc.
async fn check_daemon_running() -> Option<bool> {
    Some(super::daemon_control::status().await.running)
}

/// Fallback: ask `launchctl print` for the a11y-helper trust state.
/// Only called when the log scan is inconclusive (returns `None`).
#[cfg(not(feature = "capture"))]
async fn launchctl_a11y_trusted() -> Option<bool> {
    let uid = crate::sys::uid_str();
    let out = tokio::process::Command::new("launchctl")
        .args(["print", &format!("gui/{}/com.meridiona.a11y-helper", uid)])
        .output()
        .await
        .ok()?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    if stdout.is_empty() {
        return None;
    }
    if stdout.contains("a11y_trusted = 1") || stdout.contains("trusted") {
        Some(true)
    } else if stdout.contains("a11y_trusted = 0") {
        Some(false)
    } else {
        None
    }
}

/// Re-run the health check and push it to the webview as `health-update`, out of band
/// with the poll loop.
///
/// The banner is push-only: `HealthBanner` primes once on mount and then updates solely
/// from this event, which [`crate::poll::refresh_health`] emits every **60 s** (every 2nd
/// 30 s tick). So anything that changes provider health *right now* - connecting,
/// disconnecting, swapping a key, switching provider - would otherwise leave the banner
/// telling the user the opposite of what the screen they just clicked says, for up to a
/// minute. Every command that moves `llm_provider_ok` calls this so the two agree
/// immediately.
///
/// Fire-and-forget: a failed emit is a stale banner for one tick, never a failed command.
/// It is still LOGGED, though - "the banner is briefly stale" is a claim about the failure,
/// and discarding the error outright is what would make it unfalsifiable. A repeated emit
/// failure means the banner has stopped updating entirely, which is indistinguishable from
/// "health never changed" on screen and impossible to diagnose from a bug report without
/// this line.
#[tracing::instrument(skip(app))]
pub async fn push_health_update(app: &tauri::AppHandle) {
    use tauri::Emitter;
    let health = check_health().await;
    let llm_provider_ok = health.llm_provider_ok;
    match app.emit("health-update", &health) {
        Ok(()) => tracing::debug!(llm_provider_ok = ?llm_provider_ok, "health update pushed"),
        Err(e) => tracing::warn!(
            error = %e,
            "health update emit failed - banner stays stale until the next poll tick"
        ),
    }
}

/// The health check command (the ported `/api/health` GET).
///
/// Runs all three checks in parallel and returns the combined result.
/// Errors resolve to an empty response (matches the route's silent-resolve contract).
#[tauri::command]
#[tracing::instrument]
pub async fn get_health() -> Result<HealthResponse, String> {
    let result = check_health().await;
    tracing::info!(
        db = ?result.database_ready,
        daemon = ?result.daemon_running,
        a11y = ?result.a11y_helper_trusted,
        llm_provider = ?result.llm_provider_name,
        llm_provider_ok = ?result.llm_provider_ok,
        llm_provider_rate_limited = ?result.llm_provider_rate_limited,
        llm_provider_detail = ?result.llm_provider_detail,
        "health checked"
    );
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hr(database_ready: Option<bool>, daemon_running: Option<bool>) -> HealthResponse {
        HealthResponse {
            database_ready,
            daemon_running,
            a11y_helper_trusted: None,
            error: None,
            llm_provider_ok: None,
            llm_provider_rate_limited: None,
            llm_provider_name: None,
            llm_provider_detail: None,
        }
    }

    #[test]
    fn healthy_when_both_signals_are_true() {
        assert!(is_healthy(&hr(Some(true), Some(true))));
    }

    #[test]
    fn unhealthy_when_the_db_is_not_ready() {
        assert!(!is_healthy(&hr(Some(false), Some(true))));
    }

    #[test]
    fn unhealthy_when_the_daemon_is_not_running() {
        assert!(!is_healthy(&hr(Some(true), Some(false))));
    }

    /// `database_ready: None` must read as unhealthy - an unknown DB state is
    /// never treated as "fine". Older-schema compat only applies to
    /// `daemon_running` (below), not this field.
    #[test]
    fn an_unknown_db_state_is_treated_as_unhealthy() {
        assert!(!is_healthy(&hr(None, Some(true))));
    }

    /// `daemon_running: None` defaults to healthy (older schema compat) - the
    /// mirror image of the DB default, and the one place the two signals
    /// disagree on how to treat "unknown".
    #[test]
    fn an_unknown_daemon_state_defaults_to_healthy() {
        assert!(is_healthy(&hr(Some(true), None)));
    }
}
