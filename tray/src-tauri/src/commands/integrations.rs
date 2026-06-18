//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! `/api/integrations` (+ tracker connect/disconnect) ported to Rust.
//!
//! The "connect a tracker" surface — all file/env/process/external-HTTP, so it
//! lives tray-side, not in `meridian-core` (which stays DB-only):
//! - [`get_integrations`] — the GET: which trackers are connected.
//! - [`disconnect_integration`] — the DELETE: forget a tracker's credentials.
//! - [`discover_azure_devops`] — the `azure-devops/discover` POST: probe the
//!   Azure DevOps REST API for a PAT's orgs/projects (external HTTP).
//! - [`start_oauth`] — the `auth/oauth/start` POST: launch `meridian oauth-login`
//!   in the background; the browser flow writes the token store the GET reads.
//!
//! Env-path note (load-bearing): both the GET *and* the DELETE resolve the
//! credential `.env` through the SAME [`crate::install::detect_install_mode`] —
//! canonical `~/.meridian/.env` (all install types) or a repo `.env` (dev). The
//! daemon reads that same file (dotenvy walks up from `~/.meridian/app`, finding
//! `~/.meridian/.env`), so a disconnect the GET reflects is also one the daemon
//! sees on its next SIGHUP restart. Read-target and write-target MUST stay one
//! resolver — never write creds to one file and read status from another.
//!
//! Deliberate divergence from the route: the Next route's prod `activeEnvPath()`
//! still points at the legacy `~/.meridian/app/.env`. The installer migrated
//! creds to `~/.meridian/.env` (`meridian-npm-setup.sh`), so the route path is
//! stale; this port uses the post-migration canonical location for both read and
//! write. (The `/api/auth/token` write route has no UI consumer — the token-setup
//! UI tells users to run `meridian config edit` — so it is intentionally NOT
//! ported; porting it would add dead Rust.)
//!
//! # Who calls this
//! Registered in `lib.rs`'s `invoke_handler!`; consumed by
//! `ui/components/views/TasksView.tsx` (the Connect-trackers panel) via
//! `ui/lib/bridge.ts` (`load` for the GET, `mutate` for the writes).

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::time::Duration;
use tauri::State;
use tracing::Instrument;

/// Providers connected via browser OAuth — their token store under
/// `~/.meridian/oauth/<p>.json` is the connect/disconnect surface.
const OAUTH_PROVIDERS: [&str; 2] = ["jira", "trello"];

/// Providers connected via `.env` keys. Disconnecting strips every listed key
/// from the active `.env`. Mirrors the route's `TOKEN_KEYS`.
const TOKEN_KEYS: &[(&str, &[&str])] = &[
    ("github", &["GITHUB_TOKEN", "GITHUB_PROJECT_IDS"]),
    ("linear", &["LINEAR_API_KEY", "LINEAR_TEAM_IDS"]),
    (
        "azure_devops",
        &[
            "AZURE_DEVOPS_PAT",
            "AZURE_DEVOPS_URL",
            "AZURE_DEVOPS_ORG",
            "AZURE_DEVOPS_PROJECT",
            "AZURE_DEVOPS_ORG_URL",
        ],
    ),
];

#[derive(Debug, Clone, Serialize)]
pub struct IntegrationsResponse {
    pub jira: bool,
    pub linear: bool,
    pub github: bool,
    pub trello: bool,
    pub azure_devops: bool,
    pub sync_errors: BTreeMap<String, String>,
}

fn home() -> Option<PathBuf> {
    std::env::var("HOME").ok().map(PathBuf::from)
}

fn parse_env(path: &std::path::Path) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let Ok(contents) = std::fs::read_to_string(path) else {
        return out;
    };
    for line in contents.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some(eq) = trimmed.find('=') {
            if eq < 1 {
                continue;
            }
            let key = trimmed[..eq].trim();
            // Strip surrounding quotes so KEY="" and KEY='' register as empty.
            let raw = trimmed[eq + 1..].trim();
            let val = raw.trim_matches('"').trim_matches('\'').trim();
            if !key.is_empty() && !val.is_empty() {
                out.insert(key.to_string(), val.to_string());
            }
        }
    }
    out
}

/// A value counts as "set" only if present and not a leftover `.env.example`
/// placeholder (`your-`, `_your_`, `-here`). Mirrors the route's `isSet`.
fn is_set(env: &HashMap<String, String>, key: &str) -> bool {
    match env.get(key) {
        None => false,
        Some(v) => {
            let lower = v.to_lowercase();
            !lower.contains("your-") && !lower.contains("_your_") && !lower.contains("-here")
        }
    }
}

fn oauth_file_exists(provider: &str) -> bool {
    home()
        .map(|h| h.join(".meridian/oauth").join(format!("{provider}.json")))
        .map(|p| p.exists())
        .unwrap_or(false)
}

/// Which trackers are connected (the ported /api/integrations GET).
#[tauri::command]
#[tracing::instrument(skip(pool))]
pub async fn get_integrations(
    pool: State<'_, Option<meridian_core::SqlitePool>>,
) -> Result<IntegrationsResponse, String> {
    let mode = crate::install::detect_install_mode();
    let env = mode.env_path().map(parse_env).unwrap_or_default();

    let jira_basic = is_set(&env, "JIRA_BASE_URL")
        && is_set(&env, "JIRA_EMAIL")
        && is_set(&env, "JIRA_API_TOKEN");

    // Sync errors are best-effort: a missing/uninitialised DB just omits them
    // (matches the route's silent catch).
    let sync_errors = match pool.inner() {
        Some(pool) => meridian_core::integrations::sync_errors(pool)
            .await
            .unwrap_or_default(),
        None => BTreeMap::new(),
    };

    Ok(IntegrationsResponse {
        jira: oauth_file_exists("jira") || jira_basic,
        linear: is_set(&env, "LINEAR_API_KEY"),
        github: is_set(&env, "GITHUB_TOKEN"),
        trello: oauth_file_exists("trello"),
        azure_devops: is_set(&env, "AZURE_DEVOPS_PAT")
            && (is_set(&env, "AZURE_DEVOPS_URL")
                || is_set(&env, "AZURE_DEVOPS_ORG")
                || is_set(&env, "AZURE_DEVOPS_ORG_URL")),
        sync_errors,
    })
}

/// POST body for [`disconnect_integration`] (`{ provider }`).
#[derive(Debug, Deserialize)]
pub struct DisconnectBody {
    pub provider: String,
}

/// Strip every `key=…` line for `keys` from `path`, in place. Mirrors the
/// route's `lines.filter(l => !keys.some(k => l.trimStart().startsWith(k + '=')))`
/// — only an EXISTING file is edited (a missing file is a no-op, never created).
fn strip_env_keys(path: &std::path::Path, keys: &[&str]) -> std::io::Result<()> {
    if !path.exists() {
        return Ok(());
    }
    let contents = std::fs::read_to_string(path)?;
    let kept: Vec<&str> = contents
        .lines()
        .filter(|line| {
            let t = line.trim_start();
            !keys.iter().any(|k| t.starts_with(&format!("{k}=")))
        })
        .collect();
    std::fs::write(path, kept.join("\n"))
}

/// Disconnect a tracker (the ported /api/integrations DELETE). For an OAuth
/// provider this deletes its `~/.meridian/oauth/<p>.json` token store; for a
/// token provider it strips that provider's keys from the active `.env` (the
/// same file [`get_integrations`] reads). Returns `{ ok: true }`; an unknown
/// provider is the route's 400.
#[tauri::command]
#[tracing::instrument(skip(body), fields(provider = %body.provider))]
pub async fn disconnect_integration(body: DisconnectBody) -> Result<serde_json::Value, String> {
    let provider = body.provider.as_str();
    let token_keys = TOKEN_KEYS.iter().find(|(p, _)| *p == provider);
    if !OAUTH_PROVIDERS.contains(&provider) && token_keys.is_none() {
        return Err("Invalid provider".to_string());
    }

    if OAUTH_PROVIDERS.contains(&provider) {
        if let Some(home) = home() {
            let token = home
                .join(".meridian/oauth")
                .join(format!("{provider}.json"));
            // Not-present is a no-op (route swallows the unlink error).
            let _ = std::fs::remove_file(&token);
        }
        tracing::info!("disconnected OAuth provider (token store removed)");
    } else if let Some((_, keys)) = token_keys {
        match crate::install::detect_install_mode().env_path() {
            Some(env_path) => strip_env_keys(env_path, keys).map_err(|e| {
                tracing::warn!(error = %e, "could not rewrite .env");
                format!("could not rewrite .env: {e}")
            })?,
            None => tracing::warn!("no .env detected — nothing to strip"),
        }
        tracing::info!("disconnected token provider (.env keys stripped)");
    }

    Ok(serde_json::json!({ "ok": true }))
}

/// POST body for [`discover_azure_devops`] (`{ pat, org? }`).
#[derive(Debug, Deserialize)]
pub struct AzureDiscoverBody {
    pub pat: String,
    /// Present → list that org's projects; absent → list the PAT owner's orgs.
    #[serde(default)]
    pub org: Option<String>,
}

/// `{ orgs }` (step 1) or `{ projects }` (step 2) — mirrors the route's two
/// response shapes; only the populated field is serialised.
#[derive(Debug, Serialize)]
pub struct AzureDiscoverResponse {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub orgs: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub projects: Option<Vec<String>>,
}

/// GET `url` with Azure DevOps Basic auth (`:PAT`, i.e. empty user / PAT
/// password — `reqwest::basic_auth("", Some(pat))` base64s exactly `:pat`,
/// matching the route's `Buffer.from(":" + pat)`). Returns the parsed JSON, or
/// `(status, error)` so the caller can map the route's per-step messages.
async fn azure_get(url: reqwest::Url, pat: &str) -> Result<serde_json::Value, (u16, String)> {
    let resp = reqwest::Client::new()
        .get(url)
        .basic_auth("", Some(pat))
        .header(reqwest::header::ACCEPT, "application/json")
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| (0, e.to_string()))?;
    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err((status.as_u16(), body));
    }
    resp.json().await.map_err(|e| (0, e.to_string()))
}

/// Pull `value[].<field>` as a sorted string list. NOTE: the route sorts with
/// `localeCompare` (locale-aware); this uses codepoint order — they differ only
/// for non-ASCII / mixed-case names in a dropdown, which is cosmetic.
fn sorted_names(body: &serde_json::Value, field: &str) -> Vec<String> {
    let mut names: Vec<String> = body
        .get("value")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|item| item.get(field).and_then(|n| n.as_str()))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    names.sort_unstable();
    names
}

/// Probe the Azure DevOps REST API for a PAT's orgs or projects (the ported
/// /api/integrations/azure-devops/discover POST). With no `org`: look up the
/// PAT owner's member id, then their organisations. With an `org`: list that
/// org's projects. Errors carry the route's exact per-step, per-status messages
/// so the connect dialog shows the same guidance.
#[tauri::command]
#[tracing::instrument(skip(body), fields(org = ?body.org))]
pub async fn discover_azure_devops(
    body: AzureDiscoverBody,
) -> Result<AzureDiscoverResponse, String> {
    if body.pat.is_empty() {
        return Err("pat is required".to_string());
    }

    if let Some(org) = body.org.as_deref() {
        // Step 2: projects for the chosen org. `push` percent-encodes the segment.
        let mut url = reqwest::Url::parse("https://dev.azure.com").unwrap();
        url.path_segments_mut()
            .expect("https base has a path")
            .push(org)
            .push("_apis")
            .push("projects");
        url.query_pairs_mut().append_pair("api-version", "7.1");

        let body_json = azure_get(url, &body.pat)
            .instrument(tracing::debug_span!("integrations.azure.projects"))
            .await
            .map_err(|(status, _detail)| {
                if status == 401 || status == 403 {
                    "PAT is invalid or lacks Work Items → Read & write scope".to_string()
                } else {
                    format!("Azure DevOps returned HTTP {status}")
                }
            })?;
        let projects = sorted_names(&body_json, "name");
        tracing::info!(count = projects.len(), "azure projects discovered");
        return Ok(AzureDiscoverResponse {
            orgs: None,
            projects: Some(projects),
        });
    }

    // Step 1a: the PAT owner's member id from the profile API.
    let profile_url = reqwest::Url::parse(
        "https://app.vssps.visualstudio.com/_apis/profile/profiles/me?api-version=6.0",
    )
    .unwrap();
    let profile = azure_get(profile_url, &body.pat)
        .instrument(tracing::debug_span!("integrations.azure.profile"))
        .await
        .map_err(|(status, _detail)| {
            if status == 401 || status == 403 {
                "PAT is invalid or expired — check it and try again".to_string()
            } else {
                format!("Azure DevOps profile API returned HTTP {status}")
            }
        })?;
    let member_id = profile
        .get("id")
        .and_then(|i| i.as_str())
        .ok_or("Azure DevOps profile API returned no member id")?;

    // Step 1b: the orgs that member belongs to.
    let mut accounts_url =
        reqwest::Url::parse("https://app.vssps.visualstudio.com/_apis/accounts").unwrap();
    accounts_url
        .query_pairs_mut()
        .append_pair("memberId", member_id)
        .append_pair("api-version", "6.0");
    let accounts = azure_get(accounts_url, &body.pat)
        .instrument(tracing::debug_span!("integrations.azure.accounts"))
        .await
        .map_err(|(status, _detail)| format!("Could not list organizations (HTTP {status})"))?;
    let orgs = sorted_names(&accounts, "accountName");
    tracing::info!(count = orgs.len(), "azure orgs discovered");
    Ok(AzureDiscoverResponse {
        orgs: Some(orgs),
        projects: None,
    })
}

/// POST body for [`start_oauth`] (`{ provider }`).
#[derive(Debug, Deserialize)]
pub struct StartOAuthBody {
    pub provider: String,
}

/// `{ started, provider }` — mirrors the route. `started=true` means the
/// background login was launched (not that it finished — the UI then polls
/// [`get_integrations`] until the token store appears).
#[derive(Debug, Serialize)]
pub struct StartOAuthResponse {
    pub started: bool,
    pub provider: String,
}

/// Launch `meridian oauth-login <provider>` in the background (the ported
/// /api/auth/oauth/start POST). The CLI opens the browser, serves the OAuth
/// callback on a local port, and writes `~/.meridian/oauth/<provider>.json`.
/// Detached so it outlives this command; output is appended to
/// `~/.meridian/logs/oauth-<provider>.log` for debugging. Only jira/trello
/// connect via OAuth (the route's 400 for anything else).
#[tauri::command]
#[tracing::instrument(fields(provider = %body.provider))]
pub async fn start_oauth(body: StartOAuthBody) -> Result<StartOAuthResponse, String> {
    let provider = body.provider.as_str();
    if !OAUTH_PROVIDERS.contains(&provider) {
        return Err(format!("Unknown provider: {provider}"));
    }

    let log_dir = std::env::var("MERIDIAN_LOG_DIR")
        .ok()
        .map(PathBuf::from)
        .or_else(|| home().map(|h| h.join(".meridian/logs")))
        .ok_or("could not resolve log dir")?;
    let _ = std::fs::create_dir_all(&log_dir);
    let log_path = log_dir.join(format!("oauth-{provider}.log"));
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .map_err(|e| format!("Could not start OAuth flow: {e}"))?;
    let err_log = log.try_clone().map_err(|e| e.to_string())?;

    let bin = crate::install::meridian_bin();
    // Detached: stdin null, stdout/stderr to the log; we drop the handle and
    // never wait, so the login survives this command returning.
    std::process::Command::new(&bin)
        .args(["oauth-login", provider])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::from(log))
        .stderr(std::process::Stdio::from(err_log))
        .spawn()
        .map_err(|e| {
            tracing::warn!(bin = %bin, error = %e, "oauth-login spawn failed");
            format!("Could not start OAuth flow: {e}")
        })?;

    tracing::info!(log = %log_path.display(), "oauth-login launched");
    Ok(StartOAuthResponse {
        started: true,
        provider: body.provider,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_env_keys_removes_only_matching_lines() {
        let dir = std::env::temp_dir().join("meridian-int-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("strip.env");
        std::fs::write(
            &path,
            "JIRA_BASE_URL=x\nLINEAR_API_KEY=secret\nLINEAR_TEAM_IDS=a,b\nKEEP=1\n",
        )
        .unwrap();

        strip_env_keys(&path, &["LINEAR_API_KEY", "LINEAR_TEAM_IDS"]).unwrap();

        let out = std::fs::read_to_string(&path).unwrap();
        assert!(out.contains("JIRA_BASE_URL=x"));
        assert!(out.contains("KEEP=1"));
        assert!(!out.contains("LINEAR_API_KEY"));
        assert!(!out.contains("LINEAR_TEAM_IDS"));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn strip_env_keys_missing_file_is_noop() {
        let path = std::env::temp_dir().join("meridian-int-test/does-not-exist.env");
        assert!(strip_env_keys(&path, &["X"]).is_ok());
    }

    #[test]
    fn sorted_names_extracts_and_sorts() {
        let body = serde_json::json!({
            "value": [{ "name": "Zebra" }, { "name": "Apple" }, { "other": "skip" }]
        });
        assert_eq!(sorted_names(&body, "name"), vec!["Apple", "Zebra"]);
    }
}
