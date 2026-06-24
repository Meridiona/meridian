//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Dashboard health — the dashboard is now a static export embedded in the
// Tauri binary (no separate Node server or launchd `com.meridiona.ui` agent).
// The Node-era HTTP probes (`serve_health`, `serve_mode_check`, asset checks)
// are gone; `doctor` reports an informational line instead of a false CRITICAL.

use crate::config::Config;
use crate::health::Check;

/// Returns a single Info check confirming the dashboard is embedded in the
/// Tauri binary. The Node-era HTTP probe (was `GET http://localhost:3939/`) is
/// removed — it always fails on healthy post-fold installs and misattributes
/// the "fault" to the dashboard when the real topology has no separate service.
pub async fn checks(_cfg: &Config) -> Vec<Check> {
    vec![Check::info(
        "ui serving",
        "ui",
        "dashboard embedded in the Tauri binary (no separate service)",
    )]
}

/// Returns `None` — the `/api/health` endpoint was retired with the Node
/// server. Post-fold the dashboard runs embedded in the Tauri webview; there
/// is no HTTP port to probe.
pub async fn api_health_check(_port: u16) -> Option<Check> {
    None
}
