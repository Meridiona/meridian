//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! `/api/openobserve` GET ported to Rust — OpenObserve service status probe.
//!
//! Three fast checks: plist existence, the install-status file, and an HTTP
//! reachability probe against `http://localhost:5080/healthz`. Mirrors the TS
//! route's GET exactly; the POST (enable/disable/install) stays in Next.js for now.
//!
//! # Who calls this
//! - Command: `get_openobserve_status` (registered in `lib.rs`)
//! - Frontend: `pollOpenObserveReady` in `ui/components/views/SettingsView.tsx`
//!   swapped to `load('/api/openobserve', 'get_openobserve_status')`.
//!   The `/api/openobserve` GET route is kept until export cutover.
//!
//! # Related
//! - [`crate::health`] — checks db + daemon + a11y; openobserve is optional infra

use serde::Serialize;
use std::time::Duration;

const HEALTHZ: &str = "http://localhost:5080/healthz";
const LABEL: &str = "com.meridiona.openobserve";

/// Response shape matching the TS route's `GET` return.
#[derive(Debug, Clone, Serialize)]
pub struct OpenObserveStatusResponse {
    pub installed: bool,
    pub installing: bool,
    pub reachable: bool,
    pub failed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// OpenObserve service status (the ported `/api/openobserve` GET).
#[tauri::command]
#[tracing::instrument]
pub async fn get_openobserve_status() -> Result<OpenObserveStatusResponse, String> {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let plist = plist_path(&home);

    let installed = std::path::Path::new(&plist).exists();
    let status = read_status_file(&home);
    let installing = status.as_deref() == Some("installing");
    let up = probe_healthz().await;

    let (failed, error) = match &status {
        Some(s) if s.starts_with("exit:") && s != "exit:0" && !up => {
            let code = &s["exit:".len()..];
            let install_log = format!("{}/.meridian/logs/openobserve-install.log", home);
            (
                true,
                Some(format!(
                    "OpenObserve install failed (code {code}) — see {install_log}"
                )),
            )
        }
        _ => (false, None),
    };

    let result = OpenObserveStatusResponse {
        installed,
        installing,
        reachable: up,
        failed,
        error,
    };
    tracing::info!(
        installed = result.installed,
        installing = result.installing,
        reachable = result.reachable,
        failed = result.failed,
        "openobserve_status"
    );
    Ok(result)
}

fn plist_path(home: &str) -> String {
    format!("{}/Library/LaunchAgents/{}.plist", home, LABEL)
}

fn read_status_file(home: &str) -> Option<String> {
    let path = format!("{}/.meridian/.oo-install.status", home);
    std::fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// HTTP reachability probe — 1 s timeout, mirrors the TS `reachable()`.
async fn probe_healthz() -> bool {
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(1))
        .build()
    {
        Ok(c) => c,
        Err(_) => return false,
    };
    client
        .get(HEALTHZ)
        .send()
        .await
        .map(|r| r.status().is_success())
        .unwrap_or(false)
}
