//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! `/api/openobserve` GET + POST ported to Rust — OpenObserve service control.
//!
//! [`get_openobserve_status`] does three fast checks (plist existence, the
//! install-status file, an HTTP reachability probe). [`set_openobserve`] is the
//! enable/disable toggle: on a fresh machine it kicks off the background
//! installer; otherwise it syncs credentials into the plist (first boot only) and
//! drives the launchd service up (`enable`→`bootstrap`→`kickstart`), confirming
//! it actually serves :5080 and that the stored credentials authenticate. The
//! toggle gates the SERVICE, not just the daemon's exporters. Faithful port of
//! `ui/app/api/openobserve/route.ts`.
//!
//! # Who calls this
//! - Commands: `get_openobserve_status` + `set_openobserve` (registered in `lib.rs`)
//! - Frontend: `SettingsView.tsx` — `pollOpenObserveReady` (status) +
//!   `applyObservability` (the toggle), via `ui/lib/bridge.ts`.
//!
//! # Related
//! - [`crate::commands::health`] — checks db + daemon + a11y; openobserve is optional infra
//! - [`crate::commands::settings`] — saves the OO credentials this reads at first boot.
//! - [`meridian_core::settings`] — where `oo_email` / `oo_password` / `otlp_endpoint` live.

use crate::sys;
use serde::Serialize;
use serde_json::{json, Value};
use std::time::Duration;

const HEALTHZ: &str = "http://localhost:5080/healthz";
const LABEL: &str = "com.meridiona.openobserve";
/// Default OTLP traces URL (the authenticated-probe target) when no endpoint is set.
const DEFAULT_TRACES_URL: &str = "http://localhost:5080/api/default/v1/traces";

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

// ── POST: enable / disable the service ────────────────────────────────────────

/// Enable or disable the OpenObserve launchd service (the ported `/api/openobserve`
/// POST). `enabled=true`: install in the background on a fresh machine (returns
/// `{ ok, installing: true }`), else sync first-boot creds + drive the service up
/// and confirm it serves + authenticates (`{ ok, running: true }`). `enabled=false`:
/// stop + disable so it doesn't relaunch at login (`{ ok, running: false }`).
/// Errors (mapped from the route's 500/409) surface to the UI as the toast text.
#[tauri::command]
#[tracing::instrument]
pub async fn set_openobserve(enabled: bool) -> Result<Value, String> {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let uid = sys::uid_str();
    let domain = format!("gui/{uid}");
    let target = format!("{domain}/{LABEL}");
    let plist = plist_path(&home);

    if !enabled {
        // Stop, and disable so RunAtLoad doesn't restart it at next login.
        launchctl(&["bootout", &target]); // "not loaded" is fine
        launchctl(&["disable", &target]);
        tracing::info!("openobserve disabled");
        return Ok(json!({ "ok": true, "running": false }));
    }

    // Fresh machine: no launchd agent yet → bootstrap from zero in the background
    // (downloads the binary, writes the plist, reads creds from settings.json),
    // and let the client poll the GET for readiness.
    if !std::path::Path::new(&plist).exists() {
        let Some(script) = resolve_installer(&home) else {
            return Err(
                "OpenObserve installer not found (scripts/install-openobserve-daemon.sh)"
                    .to_string(),
            );
        };
        start_background_install(&home, &script)?;
        tracing::info!("openobserve install started in background");
        return Ok(json!({ "ok": true, "installing": true }));
    }

    let settings = meridian_core::settings::load_runtime_settings();
    let oo_email = settings.oo_email.as_deref().filter(|s| !s.is_empty());
    let oo_password = settings.oo_password.as_deref().filter(|s| !s.is_empty());

    // OpenObserve creates its root account from ZO_ROOT_USER_* on its FIRST boot
    // only; once the data dir is populated they're ignored. So patch the plist
    // (and bounce the entry) only when not yet initialised.
    let data_dir = format!("{home}/.openobserve/data");
    let initialised = std::fs::read_dir(&data_dir)
        .map(|mut d| d.next().is_some())
        .unwrap_or(false);

    if !initialised {
        if let (Some(email), Some(password)) = (oo_email, oo_password) {
            launchctl(&["bootout", &target]); // re-read the fresh plist on next bootstrap
                                              // launchd tears down asynchronously; bootstrap fails with EIO while it
                                              // lingers, so poll until the entry is gone (max ~5 s).
            for _ in 0..10 {
                if !launchctl(&["print", &target]) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
            patch_plist_env(&plist, "ZO_ROOT_USER_EMAIL", email);
            patch_plist_env(&plist, "ZO_ROOT_USER_PASSWORD", password);
        }
    }

    // enable first — bootstrap fails with EIO on a disabled service.
    launchctl(&["enable", &target]);
    launchctl(&["bootstrap", &domain, &plist]); // "already bootstrapped" is fine
    launchctl(&["kickstart", &target]); // start now if not running

    // Confirm OO is actually SERVING, not merely loaded (launchctl reports a job
    // as present the moment it's bootstrapped, before it binds :5080).
    let mut up = false;
    for _ in 0..20 {
        if probe_healthz().await {
            up = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    if !up {
        return Err(
            "OpenObserve did not become reachable — see ~/.meridian/logs/openobserve-error.log"
                .to_string(),
        );
    }

    // Already-initialised instance: verify the stored creds actually log in. If
    // not, the user likely changed email/password after OO's first boot (which OO
    // ignores) — fail loudly rather than report success while export 401s.
    if initialised {
        if let (Some(email), Some(password)) = (oo_email, oo_password) {
            if !auth_ok(&settings.otlp_endpoint, email, password).await {
                return Err(
                    "OpenObserve is already initialised with a different login. \
                     Enter the existing OpenObserve credentials, or reset \
                     ~/.openobserve/data to start over with new ones."
                        .to_string(),
                );
            }
        }
    }
    tracing::info!("openobserve enabled and serving");
    Ok(json!({ "ok": true, "running": true }))
}

/// Run `launchctl <args>`; `true` on exit 0. Most callers ignore the result —
/// "already bootstrapped" / "not loaded" are expected non-fatal states.
fn launchctl(args: &[&str]) -> bool {
    std::process::Command::new("launchctl")
        .args(args)
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Resolve `install-openobserve-daemon.sh` across install types: bundle first
/// (`~/.meridian/app`), then walk up from cwd for a source checkout. Mirrors the
/// route's `resolveInstaller`.
fn resolve_installer(home: &str) -> Option<String> {
    let mut candidates = vec![format!(
        "{home}/.meridian/app/scripts/install-openobserve-daemon.sh"
    )];
    let mut dir = std::env::current_dir().ok();
    for _ in 0..6 {
        let Some(d) = dir.as_ref() else { break };
        candidates.push(
            d.join("scripts/install-openobserve-daemon.sh")
                .to_string_lossy()
                .into_owned(),
        );
        dir = d.parent().map(|p| p.to_path_buf());
    }
    candidates
        .into_iter()
        .find(|p| std::path::Path::new(p).exists())
}

/// Kick off the installer detached (it downloads a binary — slow), teeing output
/// to the install log and recording its exit code in the status file the GET
/// reads. Mirrors the route's `startBackgroundInstall`.
fn start_background_install(home: &str, script: &str) -> Result<(), String> {
    let log = format!("{home}/.meridian/logs/openobserve-install.log");
    let status_file = format!("{home}/.meridian/.oo-install.status");
    if let Some(dir) = std::path::Path::new(&log).parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("create log dir: {e}"))?;
    }
    std::fs::write(&status_file, "installing").map_err(|e| format!("write status: {e}"))?;

    // Wrapper runs the installer, tees to the log, then records `exit:<code>`.
    let cmd = format!(
        "{s} >> {l} 2>&1; printf 'exit:%s' \"$?\" > {f}",
        s = sh_quote(script),
        l = sh_quote(&log),
        f = sh_quote(&status_file),
    );
    // Spawn detached and drop the handle so it outlives this command (no .wait()).
    std::process::Command::new("bash")
        .args(["-c", &cmd])
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("failed to start installer: {e}"))
}

/// POSIX single-quote escape for safe interpolation into a `bash -c` string.
fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Patch one `EnvironmentVariables.<key>` entry in the plist via `plutil`. Best
/// effort — a plist without an `EnvironmentVariables` dict is left as installed.
fn patch_plist_env(plist: &str, key: &str, value: &str) {
    let _ = std::process::Command::new("plutil")
        .args([
            "-replace",
            &format!("EnvironmentVariables.{key}"),
            "-string",
            value,
            plist,
        ])
        .status();
}

/// The configured OTLP traces URL (settings override → local default).
fn traces_url(otlp_endpoint: &Option<String>) -> String {
    match otlp_endpoint {
        Some(ep) if !ep.trim().is_empty() => ep.clone(),
        _ => DEFAULT_TRACES_URL.to_string(),
    }
}

/// Do the given credentials authenticate against the running OpenObserve? Good
/// auth → 200, wrong auth → 401/403. `reqwest`'s `basic_auth` sets the header
/// (no base64 dep). Mirrors the route's `authOk`.
async fn auth_ok(otlp_endpoint: &Option<String>, email: &str, password: &str) -> bool {
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
    {
        Ok(c) => c,
        Err(_) => return false,
    };
    match client
        .post(traces_url(otlp_endpoint))
        .basic_auth(email, Some(password))
        .header("Content-Type", "application/x-protobuf")
        .body(Vec::<u8>::new())
        .send()
        .await
    {
        Ok(r) => {
            let s = r.status().as_u16();
            s != 401 && s != 403
        }
        Err(_) => false,
    }
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
