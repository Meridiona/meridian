//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! `/api/integrations` (+ tracker connect/disconnect) ported to Rust.
//!
//! The "connect a tracker" surface — all file/env/process/external-HTTP, so it
//! lives tray-side, not in `meridian-core` (which stays DB-only):
//! - [`get_integrations`] — the GET: which trackers are connected.
//! - [`disconnect_integration`] — the DELETE: forget a tracker's credentials.
//! - [`discover_azure_devops`] — the `azure-devops/discover` POST: probe the
//!   Azure DevOps REST API for a PAT's orgs/projects (external HTTP).
//! - [`start_oauth`] — the `auth/oauth/start` POST: browser OAuth. jira/trello
//!   run IN-PROCESS via the shared `meridian-oauth` crate (no subprocess);
//!   github shells `meridian oauth-login github` (gh-CLI). The flow writes the
//!   token store the GET reads.
//! - [`save_integration_token`] — the `auth/token` POST: write a token-based
//!   tracker's creds to `.env` + reload the daemon (the in-app replacement for
//!   "run `meridian config edit`"). Covers jira(token)/linear/github(PAT)/azure.
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
//! write — and [`save_integration_token`] writes through the SAME resolver.
//!
//! # Who calls this
//! Registered in `lib.rs`'s `invoke_handler!`; consumed by the shared
//! `ui/components/IntegrationConnect.tsx` (`<ConnectTrackers>`, used by BOTH the
//! dashboard `TasksView` and the first-run wizard `app/setup`) via
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

/// Providers connected via the `gh` CLI — `meridian oauth-login github`
/// writes `GITHUB_TOKEN` to `~/.meridian/.env` instead of a `.json` store.
const GH_CLI_PROVIDERS: [&str; 1] = ["github"];

/// Providers connected via `.env` keys. Disconnecting strips every listed key
/// from the active `.env`. Mirrors the route's `TOKEN_KEYS`.
const TOKEN_KEYS: &[(&str, &[&str])] = &[
    // Jira connects via OAuth AND via API token (base URL + email + token), so a
    // disconnect must strip these env keys in addition to removing the OAuth json
    // — otherwise a token-connected Jira can never be disconnected.
    ("jira", &["JIRA_BASE_URL", "JIRA_EMAIL", "JIRA_API_TOKEN"]),
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

/// Token-based connect map: `provider → [(ui_field, env_key)]`. This is the
/// write side of [`get_integrations`] — pasting a token/PAT in the UI writes
/// these `.env` keys (and reloads the daemon) so a tracker connects WITHOUT a
/// terminal step. Mirrors the deleted `/api/auth/token` route's `FIELD_MAP`,
/// plus an `azure_devops` entry (the route predated Azure). Jira here is the
/// API-token / self-hosted path (base URL + email + token); Jira Cloud OAuth
/// goes through [`start_oauth`] instead.
const TOKEN_FIELD_MAP: &[(&str, &[(&str, &str)])] = &[
    (
        "jira",
        &[
            ("base_url", "JIRA_BASE_URL"),
            ("email", "JIRA_EMAIL"),
            ("api_token", "JIRA_API_TOKEN"),
        ],
    ),
    (
        "linear",
        &[
            ("api_key", "LINEAR_API_KEY"),
            ("team_ids", "LINEAR_TEAM_IDS"),
        ],
    ),
    (
        "github",
        &[
            ("token", "GITHUB_TOKEN"),
            ("project_ids", "GITHUB_PROJECT_IDS"),
        ],
    ),
    (
        "azure_devops",
        &[("url", "AZURE_DEVOPS_URL"), ("pat", "AZURE_DEVOPS_PAT")],
    ),
];

/// Env keys that MUST be present for a provider to count as connected. Optional
/// keys (team/project IDs) are absent here. Mirrors the route's `required`.
const TOKEN_REQUIRED: &[(&str, &[&str])] = &[
    ("jira", &["JIRA_BASE_URL", "JIRA_EMAIL", "JIRA_API_TOKEN"]),
    ("linear", &["LINEAR_API_KEY"]),
    ("github", &["GITHUB_TOKEN"]),
    ("azure_devops", &["AZURE_DEVOPS_URL", "AZURE_DEVOPS_PAT"]),
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

fn oauth_error_path(provider: &str) -> Option<PathBuf> {
    home().map(|h| h.join(".meridian/oauth").join(format!("{provider}.error")))
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

/// Insert-or-replace `KEY=value` lines for `updates` in `path`, preserving every
/// other line and comment. A key already present is rewritten in place; a new key
/// is appended (deterministic order — `BTreeMap`). Creates the file (and parent
/// dir) if missing. Mirrors the deleted route's `upsertEnv` (replace-then-append)
/// so the daemon reads exactly the same shape.
fn upsert_env(path: &std::path::Path, updates: &BTreeMap<String, String>) -> std::io::Result<()> {
    let existing = std::fs::read_to_string(path).unwrap_or_default();
    let mut remaining = updates.clone();
    let mut lines: Vec<String> = existing
        .lines()
        .map(|line| {
            let key = line.split('=').next().unwrap_or("").trim();
            match remaining.remove(key) {
                Some(val) => format!("{key}={val}"),
                None => line.to_string(),
            }
        })
        .collect();
    for (key, val) in remaining {
        lines.push(format!("{key}={val}"));
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, lines.join("\n"))
}

/// Disconnect a tracker (the ported /api/integrations DELETE). Removes the
/// OAuth token store (`~/.meridian/oauth/<p>.json`) AND strips the provider's
/// `.env` keys — Jira can be connected either way, so both run. (trello = json
/// only; linear/github/azure = env keys only; jira = both.) After credentials
/// are removed, clears the provider's tasks from the DB (best-effort — warns on
/// failure but does not block the disconnect). Returns `{ ok: true }`; an unknown
/// provider is the route's 400.
#[tauri::command]
#[tracing::instrument(skip(body, pool), fields(provider = %body.provider))]
pub async fn disconnect_integration(
    body: DisconnectBody,
    pool: State<'_, Option<meridian_core::SqlitePool>>,
) -> Result<serde_json::Value, String> {
    let provider = body.provider.as_str();
    let token_keys = TOKEN_KEYS.iter().find(|(p, _)| *p == provider);
    if !OAUTH_PROVIDERS.contains(&provider)
        && !GH_CLI_PROVIDERS.contains(&provider)
        && token_keys.is_none()
    {
        return Err("Invalid provider".to_string());
    }

    // A provider can have BOTH an OAuth token store and env-key credentials
    // (Jira: OAuth json + JIRA_* keys), so run both cleanups independently rather
    // than as an either/or — otherwise a token-connected Jira survives disconnect.
    if OAUTH_PROVIDERS.contains(&provider) {
        if let Some(home) = home() {
            let token = home
                .join(".meridian/oauth")
                .join(format!("{provider}.json"));
            // Not-present is a no-op (route swallows the unlink error).
            let _ = std::fs::remove_file(&token);
        }
        tracing::info!("removed OAuth token store");
    }
    if let Some((_, keys)) = token_keys {
        match crate::install::detect_install_mode().env_path() {
            Some(env_path) => strip_env_keys(env_path, keys).map_err(|e| {
                tracing::warn!(error = %e, "could not rewrite .env");
                format!("could not rewrite .env: {e}")
            })?,
            None => tracing::warn!("no .env detected — nothing to strip"),
        }
        tracing::info!("stripped .env credential keys");
    }
    // Clear the error sentinel either way so a future connect starts clean.
    if let Some(sentinel) = oauth_error_path(provider) {
        let _ = std::fs::remove_file(&sentinel);
    }

    // Best-effort: remove the provider's tasks so they don't linger in the UI.
    // A missing DB or uninitialised tables are logged but never block disconnect.
    if let Some(p) = pool.inner() {
        if let Err(e) = meridian_core::integrations::clear_provider_tasks(p, provider).await {
            tracing::warn!(error = %e, provider, "could not clear provider tasks from DB");
        }
    }

    Ok(serde_json::json!({ "ok": true }))
}

/// POST body for [`save_integration_token`] (`{ provider, fields }`).
#[derive(Debug, Deserialize)]
pub struct SaveTokenBody {
    pub provider: String,
    /// UI field name → value, e.g. `{"api_key": "lin_…", "team_ids": "T1,T2"}`.
    #[serde(default)]
    pub fields: HashMap<String, String>,
}

/// Write a token-based tracker's credentials to the active `.env` and reload the
/// daemon — the in-app replacement for "run `meridian config edit`" (ports the
/// deleted `/api/auth/token` route). Covers jira (API-token / self-hosted),
/// linear, github (PAT), and azure_devops. Browser-OAuth providers (Jira Cloud
/// OAuth, Trello) connect via [`start_oauth`] instead.
///
/// Validation mirrors the route: required keys must be non-empty; CR/LF are
/// stripped from each value (an env file is line-oriented). For jira, any stored
/// OAuth token is removed so the freshly-set API token wins (matching
/// `resolve()`'s "API token beats stored OAuth"). Writes go through the SAME
/// `detect_install_mode().env_path()` resolver [`get_integrations`] reads, so a
/// connect the GET reflects is one the daemon sees on its next reload.
#[tauri::command]
#[tracing::instrument(skip(body), fields(provider = %body.provider))]
pub async fn save_integration_token(body: SaveTokenBody) -> Result<serde_json::Value, String> {
    let provider = body.provider.as_str();
    let field_map = TOKEN_FIELD_MAP
        .iter()
        .find(|(p, _)| *p == provider)
        .map(|(_, m)| *m)
        .ok_or_else(|| format!("Unknown provider: {provider}"))?;

    // Build env updates from the submitted fields (trimmed, newline-free).
    let mut updates: BTreeMap<String, String> = BTreeMap::new();
    for (field, env_key) in field_map {
        if let Some(raw) = body.fields.get(*field) {
            let val = raw.replace(['\r', '\n'], "").trim().to_string();
            if !val.is_empty() {
                updates.insert((*env_key).to_string(), val);
            }
        }
    }
    if updates.is_empty() {
        return Err("No fields provided".to_string());
    }

    // Required-field check (the route's 400 on a partial submit).
    let required = TOKEN_REQUIRED
        .iter()
        .find(|(p, _)| *p == provider)
        .map(|(_, r)| *r)
        .unwrap_or(&[]);
    let missing: Vec<&str> = required
        .iter()
        .copied()
        .filter(|k| !updates.contains_key(*k))
        .collect();
    if !missing.is_empty() {
        return Err(format!("Missing: {}", missing.join(", ")));
    }

    // Jira API token must win over a stale OAuth session: resolve() already
    // prefers the token, but removing the store keeps the UI/get_integrations
    // unambiguous about which auth is live.
    if provider == "jira" {
        if let Some(home) = home() {
            let _ = std::fs::remove_file(home.join(".meridian/oauth/jira.json"));
        }
    }

    // Resolve + write inside a scope so the &Path borrow of `mode` is released
    // before the daemon-reload await below.
    {
        let mode = crate::install::detect_install_mode();
        let env_path = mode.env_path().ok_or("could not resolve .env path")?;
        upsert_env(env_path, &updates).map_err(|e| {
            tracing::warn!(error = %e, "could not write .env");
            format!("could not write .env: {e}")
        })?;
    }
    tracing::info!(
        provider,
        keys = updates.len(),
        "integration token saved to .env"
    );

    // Best-effort daemon reload so credentials take effect now (not next restart).
    // A down daemon is fine — it reads .env on its next start.
    if let Err(e) = crate::commands::daemon::reload_daemon().await {
        tracing::debug!(error = %e, "daemon reload after token save (non-fatal)");
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

/// Start a browser-OAuth connect (the ported /api/auth/oauth/start POST).
/// `started=true` means the flow was launched; the UI then polls
/// [`get_oauth_status`] until the token store appears (success) or a `.error`
/// sentinel is written (failure).
///
/// **jira/trello run IN-PROCESS** via the shared `meridian-oauth` crate — the
/// tray opens the browser, serves the loopback callback in its OWN runtime, and
/// writes `~/.meridian/oauth/<provider>.json` directly. No `meridian oauth-login`
/// subprocess (which depended on resolving the daemon binary on launchd's PATH
/// and on log-tailing to surface errors). **github stays a subprocess**: its
/// `gh`-CLI flow lives in the daemon, so the tray shells out to
/// `meridian oauth-login github` exactly as before.
#[tauri::command]
#[tracing::instrument(fields(provider = %body.provider))]
pub async fn start_oauth(body: StartOAuthBody) -> Result<StartOAuthResponse, String> {
    match body.provider.as_str() {
        "jira" | "trello" => start_oauth_in_process(body.provider),
        "github" => start_oauth_github_subprocess(body.provider),
        other => Err(format!("Unknown provider: {other}")),
    }
}

/// Forward connect-time OAuth creds from the active `.env` into the tray's
/// process env so the in-process `meridian_oauth` resolvers (`client_id`,
/// `client_secret`, `redirect_port`, `app_key` — they read `std::env`) pick them
/// up. Packaged builds bake the Jira secret at compile time and inject
/// `TRELLO_APP_KEY` into the bundle `.env`; this is the dev/source path and the
/// bundle-`.env` path, mirroring the env the old subprocess flow forwarded. Only
/// sets a key absent from the process env, so a real env var always wins.
fn forward_oauth_env() {
    let mode = crate::install::detect_install_mode();
    let dot_env = mode.env_path().map(parse_env).unwrap_or_default();
    for key in [
        "JIRA_OAUTH_CLIENT_SECRET",
        "JIRA_OAUTH_CLIENT_ID",
        "JIRA_OAUTH_REDIRECT_PORT",
        "TRELLO_APP_KEY",
        "TRELLO_OAUTH_REDIRECT_PORT",
    ] {
        if std::env::var_os(key).is_none() {
            if let Some(val) = dot_env.get(key) {
                std::env::set_var(key, val);
            }
        }
    }
}

/// Run the jira/trello browser login in-process on the tray's runtime. Returns
/// immediately (`started=true`); a spawned task drives the flow and writes the
/// token store on success or the `.error` sentinel (with the REAL error string —
/// no log-tail guessing) on failure, which [`get_oauth_status`] surfaces.
fn start_oauth_in_process(provider: String) -> Result<StartOAuthResponse, String> {
    forward_oauth_env();
    // Clear any previous error sentinel before launching a fresh flow.
    if let Some(sentinel) = oauth_error_path(&provider) {
        let _ = std::fs::remove_file(&sentinel);
    }

    let task_provider = provider.clone();
    tauri::async_runtime::spawn(async move {
        let result: anyhow::Result<()> = match task_provider.as_str() {
            "jira" => meridian_oauth::jira::login(
                &meridian_oauth::jira::client_id(),
                meridian_oauth::jira::redirect_port(),
            )
            .await
            .map(|_site_url| ()),
            "trello" => {
                meridian_oauth::trello::login(
                    &meridian_oauth::trello::app_key(),
                    meridian_oauth::trello::redirect_port(),
                )
                .await
            }
            _ => Ok(()),
        };
        match result {
            Ok(()) => tracing::info!(provider = %task_provider, "in-process OAuth login succeeded"),
            Err(e) => {
                let msg = format!("{e:#}");
                tracing::warn!(provider = %task_provider, error = %msg, "in-process OAuth login failed");
                if let Some(sentinel) = oauth_error_path(&task_provider) {
                    let _ = std::fs::write(&sentinel, &msg);
                }
            }
        }
    });

    tracing::info!(provider = %provider, "in-process OAuth login launched");
    Ok(StartOAuthResponse {
        started: true,
        provider,
    })
}

/// Launch `meridian oauth-login github` as a detached subprocess (the `gh`-CLI
/// flow lives in the daemon, not the shared crate). Output goes to
/// `~/.meridian/logs/oauth-github.log`; a non-zero exit writes the `.error`
/// sentinel from the log's last line so the UI can surface it.
fn start_oauth_github_subprocess(provider: String) -> Result<StartOAuthResponse, String> {
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

    // Clear any previous error sentinel before launching a fresh flow.
    if let Some(sentinel) = oauth_error_path(&provider) {
        let _ = std::fs::remove_file(&sentinel);
    }

    let bin = crate::install::meridian_bin();
    let mut cmd = tokio::process::Command::new(&bin);
    cmd.args(["oauth-login", &provider])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::from(log))
        .stderr(std::process::Stdio::from(err_log))
        .kill_on_drop(false);

    let mut child = cmd.spawn().map_err(|e| {
        tracing::warn!(bin = %bin, error = %e, "oauth-login spawn failed");
        format!("Could not start OAuth flow: {e}")
    })?;

    // Watch the child; write a .error sentinel if it exits non-zero so the UI
    // can surface the reason without waiting for the poll timeout.
    let log_path_bg = log_path.clone();
    let provider_bg = provider.clone();
    tauri::async_runtime::spawn(async move {
        match child.wait().await {
            Ok(status) if status.success() => {
                tracing::info!(provider = %provider_bg, "oauth-login succeeded");
            }
            Ok(status) => {
                tracing::warn!(
                    provider = %provider_bg,
                    code = ?status.code(),
                    "oauth-login failed"
                );
                let msg = std::fs::read_to_string(&log_path_bg)
                    .ok()
                    .and_then(|s| {
                        s.lines()
                            .rfind(|l| !l.trim().is_empty())
                            .map(str::to_string)
                    })
                    .unwrap_or_else(|| {
                        format!("OAuth login failed (exit {})", status.code().unwrap_or(-1))
                    });
                if let Some(sentinel) = oauth_error_path(&provider_bg) {
                    let _ = std::fs::write(&sentinel, &msg);
                }
            }
            Err(e) => {
                tracing::warn!(provider = %provider_bg, error = %e, "oauth-login wait error");
            }
        }
    });

    tracing::info!(log = %log_path.display(), "oauth-login launched");
    Ok(StartOAuthResponse {
        started: true,
        provider,
    })
}

/// Status returned by [`get_oauth_status`].
#[derive(Debug, Serialize)]
pub struct OAuthStatus {
    pub connected: bool,
    pub error: Option<String>,
}

/// Poll the completion status of an OAuth login for `provider`.
///
/// Returns `connected=true` once `~/.meridian/oauth/<provider>.json` exists,
/// or `error` with the last non-empty line from the OAuth log if the child
/// process exited with a non-zero status. The UI polls this every 2 s after
/// [`start_oauth`] returns so failures surface immediately instead of waiting
/// for the 3-minute timeout.
///
/// # Who calls this
/// `ui/components/views/TasksView.tsx` `OAuthSetup`.
///
/// # Related
/// - [`start_oauth`] — launches the background `meridian oauth-login` process.
/// - [`get_integrations`] — broader connected-status check (used for success).
#[tauri::command]
#[tracing::instrument]
pub async fn get_oauth_status(provider: String) -> Result<OAuthStatus, String> {
    if !OAUTH_PROVIDERS.contains(&provider.as_str())
        && !GH_CLI_PROVIDERS.contains(&provider.as_str())
    {
        return Err(format!("Unknown provider: {provider}"));
    }
    // gh-CLI providers write a token to .env rather than a .json store.
    let connected = if GH_CLI_PROVIDERS.contains(&provider.as_str()) {
        let mode = crate::install::detect_install_mode();
        let env = mode.env_path().map(parse_env).unwrap_or_default();
        is_set(&env, "GITHUB_TOKEN")
    } else {
        oauth_file_exists(&provider)
    };
    let error = if connected {
        None
    } else {
        oauth_error_path(&provider)
            .and_then(|p| std::fs::read_to_string(p).ok())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    };
    tracing::debug!(provider = %provider, connected, error = ?error, "get_oauth_status");
    Ok(OAuthStatus { connected, error })
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

    fn tmp_env(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("meridian-int-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    #[test]
    fn upsert_env_replaces_existing_and_appends_new() {
        let path = tmp_env("upsert-replace.env");
        std::fs::write(&path, "KEEP=1\nLINEAR_API_KEY=old\n").unwrap();
        let mut updates = BTreeMap::new();
        updates.insert("LINEAR_API_KEY".to_string(), "new".to_string());
        updates.insert("LINEAR_TEAM_IDS".to_string(), "T1,T2".to_string());
        upsert_env(&path, &updates).unwrap();

        let out = std::fs::read_to_string(&path).unwrap();
        assert!(out.contains("KEEP=1"), "untouched line preserved");
        assert!(
            out.contains("LINEAR_API_KEY=new"),
            "existing key replaced in place"
        );
        assert!(!out.contains("LINEAR_API_KEY=old"), "old value gone");
        assert!(out.contains("LINEAR_TEAM_IDS=T1,T2"), "new key appended");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn upsert_env_preserves_comments_and_creates_missing() {
        // Missing file → created with just the new key.
        let path = tmp_env("upsert-create.env");
        std::fs::remove_file(&path).ok();
        let mut updates = BTreeMap::new();
        updates.insert("GITHUB_TOKEN".to_string(), "ghp_x".to_string());
        upsert_env(&path, &updates).unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap().trim(),
            "GITHUB_TOKEN=ghp_x"
        );

        // A comment line is preserved verbatim across an upsert.
        std::fs::write(&path, "# my creds\nGITHUB_TOKEN=ghp_old\n").unwrap();
        upsert_env(&path, &updates).unwrap();
        let out = std::fs::read_to_string(&path).unwrap();
        assert!(out.contains("# my creds"), "comment preserved");
        assert!(out.contains("GITHUB_TOKEN=ghp_x"));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn jira_token_keys_present_so_disconnect_strips_them() {
        // Regression guard: a Jira connected via API token must be disconnectable.
        // The disconnect path strips TOKEN_KEYS for jira IN ADDITION to removing
        // the OAuth json, so jira MUST appear in TOKEN_KEYS with its three keys.
        let jira = TOKEN_KEYS
            .iter()
            .find(|(p, _)| *p == "jira")
            .expect("jira must be in TOKEN_KEYS so token-connected Jira can disconnect");
        assert!(jira.1.contains(&"JIRA_BASE_URL"));
        assert!(jira.1.contains(&"JIRA_EMAIL"));
        assert!(jira.1.contains(&"JIRA_API_TOKEN"));

        // Round-trip: connect (upsert) then disconnect (strip) leaves no Jira creds.
        let path = tmp_env("jira-roundtrip.env");
        let mut updates = BTreeMap::new();
        updates.insert(
            "JIRA_BASE_URL".to_string(),
            "https://acme.atlassian.net".to_string(),
        );
        updates.insert("JIRA_EMAIL".to_string(), "a@b.com".to_string());
        updates.insert("JIRA_API_TOKEN".to_string(), "ATATT".to_string());
        updates.insert("KEEP".to_string(), "1".to_string());
        upsert_env(&path, &updates).unwrap();
        strip_env_keys(&path, jira.1).unwrap();
        let out = std::fs::read_to_string(&path).unwrap();
        assert!(out.contains("KEEP=1"), "unrelated key kept");
        assert!(!out.contains("JIRA_BASE_URL"));
        assert!(!out.contains("JIRA_EMAIL"));
        assert!(!out.contains("JIRA_API_TOKEN"));
        std::fs::remove_file(&path).ok();
    }
}
