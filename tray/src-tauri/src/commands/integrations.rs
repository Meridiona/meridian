//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! `/api/integrations` (+ tracker connect/disconnect) ported to Rust.
//!
//! The "connect a tracker" surface — all file/env/process/external-HTTP, so it
//! lives tray-side, not in `meridian-core` (which stays DB-only):
//! - [`get_integrations`] — the GET: which trackers are connected.
//! - [`disconnect_integration`] — the DELETE: forget a tracker's credentials.
//! - [`discover_azure_devops`] — the `azure-devops/discover` POST: probe the
//!   Azure DevOps REST API for a PAT's orgs/projects (external HTTP).
//! - [`start_oauth`] — the `auth/oauth/start` POST: browser OAuth, all providers
//!   IN-PROCESS via the shared `meridian-oauth` crate (no subprocess).
//!   jira/trello use the loopback-redirect flow (writes the `<p>.json` store);
//!   github uses the OAuth device flow (writes `GITHUB_TOKEN` to `.env`). The
//!   flow writes the credential the GET reads.
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
//! still points at the legacy `~/.meridian/app/.env`. The install now writes
//! creds to the canonical `~/.meridian/.env`, so the route path is
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
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;
use tauri::State;
use tracing::Instrument;

// Per-provider in-flight guard: prevents two concurrent OAuth flows from racing
// to bind the same loopback port or write the same token file. Checked before
// spawning; cleared (success or failure) inside the spawned task.
static JIRA_OAUTH_IN_FLIGHT: AtomicBool = AtomicBool::new(false);
static TRELLO_OAUTH_IN_FLIGHT: AtomicBool = AtomicBool::new(false);
static GITHUB_OAUTH_IN_FLIGHT: AtomicBool = AtomicBool::new(false);

// Handle to the currently in-flight jira/trello task, so [`cancel_oauth`] can
// abort it — this is what actually lets a user retry after closing the
// browser tab without waiting out the 5-minute `CONSENT_TIMEOUT`: aborting
// drops the task's `TcpListener` on the spot, freeing the loopback port, and
// `cancel_oauth` clears the in-flight flag immediately rather than waiting
// for the (now-aborted) task to reach its own cleanup code.
static JIRA_OAUTH_HANDLE: Mutex<Option<tokio::task::JoinHandle<()>>> = Mutex::new(None);
static TRELLO_OAUTH_HANDLE: Mutex<Option<tokio::task::JoinHandle<()>>> = Mutex::new(None);

/// Providers connected via browser OAuth — their token store under
/// `~/.meridian/oauth/<p>.json` is the connect/disconnect surface.
const OAUTH_PROVIDERS: [&str; 2] = ["jira", "trello"];

/// Providers whose OAuth writes a token to `~/.meridian/.env` rather than a
/// `~/.meridian/oauth/<p>.json` store — github uses the device flow and writes
/// `GITHUB_TOKEN`. [`get_oauth_status`] checks the env key, not a json file, for
/// these.
const ENV_OAUTH_PROVIDERS: [&str; 1] = ["github"];

/// Providers connected via `.env` keys. Disconnecting strips every listed key
/// from the active `.env`. Mirrors the route's `TOKEN_KEYS`.
const TOKEN_KEYS: &[(&str, &[&str])] = &[
    // Jira connects via OAuth AND via API token (base URL + email + token), so a
    // disconnect must strip these env keys in addition to removing the OAuth json
    // — otherwise a token-connected Jira can never be disconnected. JIRA_PROJECT_KEYS
    // is the project picker's selection (see discover_jira_projects) and must go too,
    // or a reconnect would silently inherit the previous account's project scope.
    (
        "jira",
        &[
            "JIRA_BASE_URL",
            "JIRA_EMAIL",
            "JIRA_API_TOKEN",
            "JIRA_PROJECT_KEYS",
        ],
    ),
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
    // Trello uses OAuth for its primary auth but also stores a user-supplied app
    // key in .env (prod has it baked in; dev users paste their own). Disconnect
    // must strip it so a rotated key isn't silently reused on the next connect.
    ("trello", &["TRELLO_APP_KEY"]),
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
            // Written by the project picker (discover_jira_projects → save here) on
            // top of EITHER auth mode — API token or a browser-OAuth session. See
            // missing_required's oauth_connected branch for why this can be saved
            // with none of the three fields above present in the payload.
            ("project_keys", "JIRA_PROJECT_KEYS"),
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
    // Trello's app key is baked in for production builds; dev users supply their
    // own from https://trello.com/app-key via the UI, which saves it here before
    // the browser OAuth flow starts.
    ("trello", &[("api_key", "TRELLO_APP_KEY")]),
];

/// Env keys that MUST be present for a provider to count as connected. Optional
/// keys (team/project IDs) are absent here. Mirrors the route's `required`.
const TOKEN_REQUIRED: &[(&str, &[&str])] = &[
    ("jira", &["JIRA_BASE_URL", "JIRA_EMAIL", "JIRA_API_TOKEN"]),
    ("linear", &["LINEAR_API_KEY"]),
    ("github", &["GITHUB_TOKEN"]),
    ("azure_devops", &["AZURE_DEVOPS_URL", "AZURE_DEVOPS_PAT"]),
];

/// Which of `provider`'s [`TOKEN_REQUIRED`] keys would still be unset after
/// applying `updates` on top of the `existing` `.env`.
///
/// The distinction matters because a required key is required to *end up set*,
/// not to appear in every payload. GitHub is the case that forces it: its OAuth
/// device flow writes `GITHUB_TOKEN` to `.env` itself (see [`ENV_OAUTH_PROVIDERS`]),
/// and the project picker that runs afterwards submits only `project_ids`. Judging
/// that payload in isolation reported "Missing: GITHUB_TOKEN" for a token that was
/// already on disk. The user-visible symptom was a dead-end rather than a mere
/// error string: after connecting GitHub, the picker would list the account's
/// boards (it reads the very token the check claimed was missing) and then refuse
/// every save, so `GITHUB_PROJECT_IDS` could never be set and GitHub never synced.
///
/// Checking `existing` rather than dropping the requirement keeps the guard that
/// matters: a `project_ids`-only save with no GitHub connection at all still
/// reports the token missing. Both halves test the *value*, not bare presence, so
/// a leftover `.env.example` placeholder fails the check whether it arrives in the
/// payload or is already on disk — that agrees with the connected-state test in
/// [`get_integrations`], which would otherwise accept the save and then report the
/// provider disconnected.
///
/// `oauth_connected` covers the Jira analogue of the GitHub gap above, with one
/// twist: GitHub's OAuth (device flow) writes `GITHUB_TOKEN` straight into
/// `.env`, so `existing` alone already proves it happened. Jira's browser-OAuth
/// writes to `~/.meridian/oauth/jira.json` instead — a file `existing` (a parsed
/// `.env`) can never see — so a jira project-keys-only save (from
/// [`discover_jira_projects`]'s picker, run after an OAuth connect with no API
/// token ever set) would otherwise report the three basic-auth fields missing
/// even though Jira is fully connected. Meaningful only for `provider == "jira"`;
/// every other provider ignores it.
///
/// # Who calls this
/// [`save_integration_token`], which turns a non-empty result into its error.
fn missing_required(
    provider: &str,
    updates: &BTreeMap<String, String>,
    existing: &HashMap<String, String>,
    oauth_connected: bool,
) -> Vec<&'static str> {
    let required = TOKEN_REQUIRED
        .iter()
        .find(|(p, _)| *p == provider)
        .map(|(_, r)| *r)
        .unwrap_or(&[]);

    // A projects-only submit (none of the basic-auth fields in this payload) on
    // top of an already-live OAuth session needs nothing further from this check.
    // A submit that DOES include a basic-auth field is a real (re)connect attempt
    // and must still be validated normally, even if OAuth also happens to exist.
    if provider == "jira" && oauth_connected && !required.iter().any(|k| updates.contains_key(*k)) {
        return Vec::new();
    }

    required
        .iter()
        .copied()
        .filter(|k| match updates.get(*k) {
            // Submitted: judge the payload's value, and ONLY that. Falling back
            // to `.env` here is what let a submitted placeholder pass — it was
            // not "set", but the valid token already on disk satisfied the
            // check, and `upsert_env` then wrote the placeholder over it,
            // destroying working credentials. A submitted placeholder must fail
            // the required-field check so the write never happens.
            Some(v) => !value_is_set(v),
            // Not submitted: the write leaves whatever `.env` holds, so that is
            // what the check judges. This is the case the GitHub project picker
            // depends on — it submits only `project_ids` on top of a token its
            // OAuth device flow already wrote.
            None => !is_set(existing, k),
        })
        .collect()
}

#[derive(Debug, Clone, Serialize)]
pub struct IntegrationsResponse {
    pub jira: bool,
    pub linear: bool,
    pub github: bool,
    pub trello: bool,
    pub azure_devops: bool,
    /// `true` once `GITHUB_PROJECT_IDS` is set — `github` alone only means the
    /// OAuth token exists; sync additionally needs at least one selected
    /// project (see [`discover_github_projects`]). Lets the UI tell "connected,
    /// token only" apart from "connected and actually syncing."
    pub github_projects_selected: bool,
    /// `true` once `JIRA_PROJECT_KEYS` is set — mirrors
    /// [`Self::github_projects_selected`] for Jira (see
    /// [`discover_jira_projects`]). `jira` alone means EITHER auth mode is live;
    /// this additionally means at least one project was picked to sync.
    pub jira_projects_selected: bool,
    pub sync_errors: BTreeMap<String, String>,
}

fn home() -> Option<PathBuf> {
    meridian_core::paths::home_dir()
}

/// Parse a `.env` file using dotenvy so edge cases (export prefix, quoted
/// values, backslash continuation) are handled the same way the daemon handles
/// them via `dotenvy::dotenv_override()`. The old hand-rolled parser diverged
/// on these cases, which could cause the tray and daemon to read different
/// values for the same key.
fn parse_env(path: &std::path::Path) -> HashMap<String, String> {
    dotenvy::from_path_iter(path)
        .map(|iter| iter.filter_map(|item| item.ok()).collect())
        .unwrap_or_default()
}

/// Whether a raw value counts as "set" — i.e. not a leftover `.env.example`
/// placeholder (`your-`, `_your_`, `-here`).
///
/// Split out from [`is_set`] so the same rule can be applied to a submitted
/// payload, which is a `BTreeMap` rather than the parsed-`.env` `HashMap`.
/// One predicate for both is what stops a submitted placeholder from passing
/// the required-field check and then being reported disconnected by
/// [`get_integrations`], which tests the written value with this same rule.
fn value_is_set(v: &str) -> bool {
    let lower = v.to_lowercase();
    !lower.contains("your-") && !lower.contains("_your_") && !lower.contains("-here")
}

/// A value counts as "set" only if present and not a leftover `.env.example`
/// placeholder (`your-`, `_your_`, `-here`). Mirrors the route's `isSet`.
fn is_set(env: &HashMap<String, String>, key: &str) -> bool {
    env.get(key).is_some_and(|v| value_is_set(v))
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

/// Which trackers have usable credentials, in the canonical provider order the UI
/// also uses (`ui/components/timeline/useTimelineData.ts`'s `PROVIDER_IDS`).
///
/// The credential-probing half of [`get_integrations`], split out so a command can
/// ask "is this provider actually connected?" without going through the response
/// struct field by field.
///
/// `has_oauth` is injected rather than calling [`oauth_file_exists`] directly: the
/// two OAuth-backed trackers are otherwise decided by files under the real `$HOME`,
/// which would make this untestable (and its result quietly dependent on whichever
/// machine it runs on).
fn connected_from_env(
    env: &HashMap<String, String>,
    has_oauth: impl Fn(&str) -> bool,
) -> Vec<&'static str> {
    let jira_basic =
        is_set(env, "JIRA_BASE_URL") && is_set(env, "JIRA_EMAIL") && is_set(env, "JIRA_API_TOKEN");
    let azure = is_set(env, "AZURE_DEVOPS_PAT")
        && (is_set(env, "AZURE_DEVOPS_URL")
            || is_set(env, "AZURE_DEVOPS_ORG")
            || is_set(env, "AZURE_DEVOPS_ORG_URL"));

    let mut on = Vec::new();
    if has_oauth("jira") || jira_basic {
        on.push("jira");
    }
    if is_set(env, "LINEAR_API_KEY") {
        on.push("linear");
    }
    if is_set(env, "GITHUB_TOKEN") {
        on.push("github");
    }
    if has_oauth("trello") {
        on.push("trello");
    }
    if azure {
        on.push("azure_devops");
    }
    on
}

/// Every connected tracker id, resolved through the same install-mode `.env` the
/// GET reads.
///
/// Exists so other commands can VALIDATE a provider id the frontend sent instead of
/// trusting it - see [`crate::commands::set_worklog_provider`].
pub fn connected_providers() -> Vec<&'static str> {
    let mode = crate::install::detect_install_mode();
    let env = mode.env_path().map(parse_env).unwrap_or_default();
    connected_from_env(&env, oauth_file_exists)
}

/// Which trackers are connected (the ported /api/integrations GET).
#[tauri::command]
#[tracing::instrument(skip(pool))]
pub async fn get_integrations(
    pool: State<'_, Option<meridian_core::SqlitePool>>,
) -> Result<IntegrationsResponse, String> {
    let mode = crate::install::detect_install_mode();
    let env = mode.env_path().map(parse_env).unwrap_or_default();
    let on = connected_from_env(&env, oauth_file_exists);

    // Sync errors are best-effort: a missing/uninitialised DB just omits them
    // (matches the route's silent catch).
    let sync_errors = match pool.inner() {
        Some(pool) => meridian_core::integrations::sync_errors(pool)
            .await
            .unwrap_or_default(),
        None => BTreeMap::new(),
    };

    Ok(IntegrationsResponse {
        jira: on.contains(&"jira"),
        linear: on.contains(&"linear"),
        github: on.contains(&"github"),
        trello: on.contains(&"trello"),
        azure_devops: on.contains(&"azure_devops"),
        github_projects_selected: is_set(&env, "GITHUB_PROJECT_IDS"),
        jira_projects_selected: is_set(&env, "JIRA_PROJECT_KEYS"),
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
/// so the daemon reads exactly the same shape. Always writes with a trailing
/// newline so subsequent appends don't concatenate on the same line.
///
/// `pub(crate)`: also used by [`crate::db_key`] to mirror the SQLCipher
/// encryption key into the same `.env` the daemon reads `MERIDIAN_DB` from.
pub(crate) fn upsert_env(
    path: &std::path::Path,
    updates: &BTreeMap<String, String>,
) -> std::io::Result<()> {
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
    let content = format!("{}\n", lines.join("\n"));
    std::fs::write(path, content)
}

/// Disconnect a tracker (the ported /api/integrations DELETE). Removes the
/// OAuth token store (`~/.meridian/oauth/<p>.json`) AND strips the provider's
/// `.env` keys — Jira can be connected either way, so both run. (trello = json
/// + env key; linear/github/azure = env keys only; jira = both.) After credentials
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
        && !ENV_OAUTH_PROVIDERS.contains(&provider)
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
/// linear, github (PAT), azure_devops, and trello (API key prerequisite for the
/// browser OAuth flow; saving `TRELLO_APP_KEY` here lets [`start_oauth`] find the
/// key and proceed — the OAuth step still runs to write `~/.meridian/oauth/trello.json`).
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

    // Required-field check (the route's 400 on a partial submit), judged against
    // the state this write LEAVES BEHIND — payload plus what `.env` already
    // holds — rather than the payload alone. See [`missing_required`]; the
    // GitHub project picker submits only `project_ids` on top of a token its
    // OAuth device flow already wrote.
    //
    // Read through `detect_install_mode().env_path()`, the same resolver
    // `discover_github_projects` reads the token with, so the check and the call
    // that proves the token exists can never disagree about which file is live.
    // Resolved ONCE and reused for the write below: `detect_install_mode` does
    // synchronous filesystem probing, and this is an async command, so calling
    // it twice stalls the reactor twice for the same answer.
    let mode = crate::install::detect_install_mode();
    let existing = mode.env_path().map(parse_env).unwrap_or_default();
    let oauth_connected = provider == "jira" && oauth_file_exists("jira");
    let missing = missing_required(provider, &updates, &existing, oauth_connected);
    if !missing.is_empty() {
        return Err(format!("Missing: {}", missing.join(", ")));
    }

    // Jira API token must win over a stale OAuth session: resolve() already
    // prefers the token, but removing the store keeps the UI/get_integrations
    // unambiguous about which auth is live. Gated on an ACTUAL basic-auth field
    // being submitted — a projects-only save (see `oauth_connected` above) must
    // not delete the very OAuth session it's riding on.
    let is_basic_auth_submit = updates.contains_key("JIRA_BASE_URL")
        || updates.contains_key("JIRA_EMAIL")
        || updates.contains_key("JIRA_API_TOKEN");
    if provider == "jira" && is_basic_auth_submit {
        if let Some(home) = home() {
            let _ = std::fs::remove_file(home.join(".meridian/oauth/jira.json"));
        }
    }
    // Clear any OAuth error sentinel — a previous failed OAuth attempt writes a
    // sentinel that get_oauth_status surfaces as an error. Token connect succeeds
    // independently of OAuth, so the sentinel must be cleared or the dashboard
    // shows a broken OAuth state even though the API token is working.
    if let Some(sentinel) = oauth_error_path(provider) {
        let _ = std::fs::remove_file(&sentinel);
    }

    // Resolve + write. `upsert_env` is synchronous std::fs I/O; run it on the
    // blocking thread pool so we don't stall the Tokio reactor.
    let key_count = updates.len();
    {
        // On a fresh `.app` install no `.env` exists yet, so `detect_install_mode`
        // returns `Bare` (no path). Credentials must be saveable before any `.env`
        // exists — default to the canonical `~/.meridian/.env`, which `upsert_env`
        // creates (parent dir + file). Dev/Canonical modes keep their resolved path.
        let env_path = match mode.env_path() {
            Some(p) => p.to_owned(),
            None => crate::install::canonical_env_path()
                .ok_or("could not resolve home directory for .env")?,
        };
        tokio::task::spawn_blocking(move || upsert_env(&env_path, &updates))
            .await
            .map_err(|e| format!("spawn_blocking panicked: {e}"))?
            .map_err(|e| {
                tracing::warn!(error = %e, "could not write .env");
                format!("could not write .env: {e}")
            })?;
    }
    tracing::info!(
        provider,
        keys = key_count,
        "integration token saved to .env"
    );

    // Best-effort daemon reload so credentials take effect now (not next restart).
    // A down daemon is fine — it reads .env on its next start. `reloaded` is
    // returned to the UI so it can warn the user when the daemon isn't running
    // (common in dev where the daemon isn't launchd-supervised).
    let reloaded = crate::commands::daemon::reload_daemon().await.is_ok();
    if !reloaded {
        tracing::debug!("daemon reload after token save (non-fatal — will pick up on next start)");
    }

    Ok(serde_json::json!({ "ok": true, "reloaded": reloaded }))
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
            .map_err(|(status, detail)| {
                if status == 401 || status == 403 {
                    "PAT is invalid or lacks Work Items → Read & write scope".to_string()
                } else if status == 0 {
                    format!("Could not reach Azure DevOps: {detail}")
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
        .map_err(|(status, detail)| {
            if status == 401 || status == 403 {
                "PAT is invalid or org-scoped — enter your org name manually below, or use an 'All accessible organizations' PAT".to_string()
            } else if status == 0 {
                format!("Could not reach Azure DevOps: {detail}")
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
        .map_err(|(status, detail)| {
            if status == 0 {
                format!("Could not list organizations: {detail}")
            } else {
                format!("Could not list organizations (HTTP {status})")
            }
        })?;
    let orgs = sorted_names(&accounts, "accountName");
    tracing::info!(count = orgs.len(), "azure orgs discovered");
    Ok(AzureDiscoverResponse {
        orgs: Some(orgs),
        projects: None,
    })
}

/// One GitHub Projects v2 board, returned by [`discover_github_projects`].
#[derive(Debug, Clone, Serialize)]
pub struct GithubProjectOption {
    /// Projects v2 node id (`PVT_xxx`) — what `GITHUB_PROJECT_IDS` stores.
    pub id: String,
    pub title: String,
    /// The viewer's own login, or the owning org's login.
    pub owner: String,
}

/// `{ projects }` — mirrors [`AzureDiscoverResponse`]'s single-populated-field
/// shape, minus the two-step org/project split (GitHub returns everything the
/// token can see in one GraphQL round trip).
#[derive(Debug, Serialize)]
pub struct GithubDiscoverResponse {
    pub projects: Vec<GithubProjectOption>,
}

/// Flatten the GraphQL response body into a sorted, owner-tagged project list.
/// Pure function so it's unit-testable against a hand-built fixture — mirrors
/// the shape `viewer.projectsV2` + `viewer.organizations.nodes[].projectsV2`.
fn flatten_github_projects(body: &serde_json::Value) -> Vec<GithubProjectOption> {
    let viewer = body.pointer("/data/viewer");
    let nodes_of = |v: &serde_json::Value| -> Vec<serde_json::Value> {
        v.pointer("/projectsV2/nodes")
            .and_then(|n| n.as_array())
            .cloned()
            .unwrap_or_default()
    };

    let mut projects = Vec::new();
    if let Some(viewer) = viewer {
        let owner = viewer
            .get("login")
            .and_then(|l| l.as_str())
            .unwrap_or("you")
            .to_string();
        for node in nodes_of(viewer) {
            if let (Some(id), Some(title)) = (
                node.get("id").and_then(|v| v.as_str()),
                node.get("title").and_then(|v| v.as_str()),
            ) {
                projects.push(GithubProjectOption {
                    id: id.to_string(),
                    title: title.to_string(),
                    owner: owner.clone(),
                });
            }
        }
        let orgs = viewer
            .pointer("/organizations/nodes")
            .and_then(|n| n.as_array())
            .cloned()
            .unwrap_or_default();
        for org in orgs {
            let owner = org
                .get("login")
                .and_then(|l| l.as_str())
                .unwrap_or("org")
                .to_string();
            for node in nodes_of(&org) {
                if let (Some(id), Some(title)) = (
                    node.get("id").and_then(|v| v.as_str()),
                    node.get("title").and_then(|v| v.as_str()),
                ) {
                    projects.push(GithubProjectOption {
                        id: id.to_string(),
                        title: title.to_string(),
                        owner: owner.clone(),
                    });
                }
            }
        }
    }
    projects.sort_unstable_by(|a, b| {
        (a.owner.as_str(), a.title.as_str()).cmp(&(b.owner.as_str(), b.title.as_str()))
    });
    projects
}

/// List the GitHub Projects v2 boards the connected account can see (the
/// browser-OAuth-connect follow-up: `discover_azure_devops`'s PAT→org→project
/// discovery, but GitHub returns everything in one GraphQL call). Reads
/// `GITHUB_TOKEN` from the resolved `.env` — no args, since discovery only
/// makes sense once [`start_oauth`]/[`save_integration_token`] already wrote a
/// token there (unlike Azure DevOps, whose PAT is pasted in *before* it's saved).
///
/// # Who calls this
/// `ui/components/IntegrationConnect.tsx`'s `GitHubProjectPicker`, both right
/// after a GitHub OAuth connect succeeds and from the "connected but no
/// projects selected" prompt in `ConnectedPanel`.
#[tauri::command]
#[tracing::instrument]
pub async fn discover_github_projects() -> Result<GithubDiscoverResponse, String> {
    let mode = crate::install::detect_install_mode();
    let env = mode.env_path().map(parse_env).unwrap_or_default();
    let token = env
        .get("GITHUB_TOKEN")
        .filter(|t| !t.trim().is_empty())
        .ok_or("GitHub is not connected yet")?;

    const QUERY: &str = r#"{ viewer { login
        projectsV2(first: 50) { nodes { id title } }
        organizations(first: 25) { nodes { login projectsV2(first: 50) { nodes { id title } } } }
    } }"#;

    let resp = reqwest::Client::new()
        .post("https://api.github.com/graphql")
        .bearer_auth(token)
        .header(reqwest::header::USER_AGENT, "meridian-tray")
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .json(&serde_json::json!({ "query": QUERY }))
        .timeout(Duration::from_secs(15))
        .send()
        .instrument(tracing::debug_span!("integrations.github.projects"))
        .await
        .map_err(|e| format!("Could not reach GitHub: {e}"))?;

    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        return Err(
            "GitHub token invalid or missing the read:project scope — reconnect GitHub".to_string(),
        );
    }
    if !status.is_success() {
        return Err(format!("GitHub returned HTTP {status}"));
    }

    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("Could not parse GitHub response: {e}"))?;

    // GraphQL returns HTTP 200 with BOTH `data` and `errors` on a partial
    // failure — e.g. a user in an org with SAML SSO enforcement (their token
    // isn't SSO-authorised for that org) or an org that restricts Projects v2
    // visibility gets that org as an `errors` entry while their own/other-org
    // projects still come back in `data`. Only hard-fail if `data.viewer` is
    // absent entirely (e.g. bad credentials); otherwise parse what's there.
    if let Some(errors) = body.get("errors").and_then(|e| e.as_array()) {
        if !errors.is_empty() {
            let first_msg = errors
                .first()
                .and_then(|e| e.get("message"))
                .and_then(|m| m.as_str())
                .unwrap_or("unknown");
            if body.pointer("/data/viewer").is_none() {
                return Err(format!("GitHub API error: {first_msg}"));
            }
            tracing::warn!(
                errors = errors.len(),
                first = first_msg,
                "GitHub GraphQL partial errors — some orgs/projects may be missing from the list"
            );
        }
    }

    let projects = flatten_github_projects(&body);
    tracing::info!(count = projects.len(), "github projects discovered");
    Ok(GithubDiscoverResponse { projects })
}

/// One Jira project, returned by [`discover_jira_projects`].
#[derive(Debug, Clone, Serialize)]
pub struct JiraProjectOption {
    /// Numeric Jira project id — informational only; `JIRA_PROJECT_KEYS` stores
    /// [`Self::key`], the identifier the JQL in
    /// `src/intelligence/providers/jira/fetch.rs` and every other Jira read path
    /// key off.
    pub id: String,
    pub key: String,
    pub name: String,
}

/// `{ projects }` — mirrors [`GithubDiscoverResponse`].
#[derive(Debug, Serialize)]
pub struct JiraDiscoverResponse {
    pub projects: Vec<JiraProjectOption>,
}

fn default_true() -> bool {
    true
}

#[derive(Deserialize)]
struct JiraProjectSearchPage {
    values: Vec<JiraProjectSearchItem>,
    /// Defensively defaults to `true` (stop paginating) rather than `false` — a
    /// response shape Jira changed out from under this parser should fail safe
    /// by returning what it got, not loop forever re-requesting the same page.
    #[serde(rename = "isLast", default = "default_true")]
    is_last: bool,
}

#[derive(Deserialize)]
struct JiraProjectSearchItem {
    id: String,
    key: String,
    name: String,
}

/// Parse one `/rest/api/3/project/search` page into `(projects, is_last)`. Pure
/// function so pagination termination is unit-testable against hand-built
/// fixtures without a live Jira site. An unparseable body (e.g. an error page
/// serialised as JSON) reports zero projects and `is_last = true` so the caller
/// stops rather than looping.
fn parse_jira_projects_page(body: &serde_json::Value) -> (Vec<JiraProjectOption>, bool) {
    let page: JiraProjectSearchPage = match serde_json::from_value(body.clone()) {
        Ok(p) => p,
        Err(_) => return (Vec::new(), true),
    };
    let projects = page
        .values
        .into_iter()
        .map(|v| JiraProjectOption {
            id: v.id,
            key: v.key,
            name: v.name,
        })
        .collect();
    (projects, page.is_last)
}

/// Resolve a [`meridian_oauth::jira::JiraReqCtx`] for Jira discovery, tray-side.
/// Mirrors the daemon's `src/intelligence/oauth/jira::resolve` (API token beats
/// a stored OAuth session — a set `JIRA_API_TOKEN` always wins) but reads
/// `.env` directly rather than depending on the daemon's `JiraConfig`, the same
/// crate-boundary reason [`discover_github_projects`] reads `GITHUB_TOKEN`
/// directly instead of calling into daemon code.
async fn resolve_jira_ctx() -> Result<meridian_oauth::jira::JiraReqCtx, String> {
    let mode = crate::install::detect_install_mode();
    let env = mode.env_path().map(parse_env).unwrap_or_default();
    let has_basic = is_set(&env, "JIRA_BASE_URL")
        && is_set(&env, "JIRA_EMAIL")
        && is_set(&env, "JIRA_API_TOKEN");
    if has_basic {
        return Ok(meridian_oauth::jira::JiraReqCtx::Basic {
            base_url: env.get("JIRA_BASE_URL").cloned().unwrap_or_default(),
            email: env.get("JIRA_EMAIL").cloned().unwrap_or_default(),
            api_token: env.get("JIRA_API_TOKEN").cloned().unwrap_or_default(),
        });
    }
    if oauth_file_exists("jira") {
        let tokens = meridian_oauth::jira::ensure_fresh()
            .await
            .map_err(|e| format!("Could not refresh Jira session: {e}"))?;
        return Ok(meridian_oauth::jira::JiraReqCtx::OAuth {
            token: tokens.access_token,
            cloud_id: tokens.cloud_id,
            site_url: tokens.site_url,
        });
    }
    Err("Jira is not connected yet".to_string())
}

/// List the Jira projects the connected account/site can see (the Jira
/// analogue of [`discover_github_projects`]) — works under EITHER auth mode,
/// API token or browser OAuth, via [`resolve_jira_ctx`]. Paginates
/// `/rest/api/3/project/search` (50/page) until Jira reports `isLast`, so a
/// site with more than one page of projects isn't silently truncated the way a
/// single-call discovery would be.
///
/// # Who calls this
/// `ui/components/IntegrationConnect.tsx`'s `JiraProjectPicker`, both right
/// after a Jira connect (OAuth or token) succeeds and from the "connected but
/// no projects selected" prompt in `ConnectedPanel` — mirrors
/// [`discover_github_projects`]'s two entry points.
#[tauri::command]
#[tracing::instrument]
pub async fn discover_jira_projects() -> Result<JiraDiscoverResponse, String> {
    let ctx = resolve_jira_ctx().await?;
    let client = reqwest::Client::new();

    const PAGE_SIZE: u64 = 50;
    let mut projects = Vec::new();
    let mut start_at: u64 = 0;
    loop {
        let url = ctx.api_url("/rest/api/3/project/search");
        let resp = ctx
            .apply(client.get(&url))
            .query(&[
                ("startAt", start_at.to_string()),
                ("maxResults", PAGE_SIZE.to_string()),
            ])
            .timeout(Duration::from_secs(15))
            .send()
            .instrument(tracing::debug_span!("integrations.jira.projects", start_at))
            .await
            .map_err(|e| format!("Could not reach Jira: {e}"))?;

        let status = resp.status();
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(
                "Jira credentials invalid or missing project access — reconnect Jira".to_string(),
            );
        }
        if !status.is_success() {
            return Err(format!("Jira returned HTTP {status}"));
        }

        let body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| format!("Could not parse Jira response: {e}"))?;
        let (mut page, is_last) = parse_jira_projects_page(&body);
        let page_len = page.len() as u64;
        projects.append(&mut page);
        if is_last || page_len == 0 {
            break;
        }
        start_at += page_len;
    }

    projects.sort_unstable_by(|a, b| a.name.cmp(&b.name));
    tracing::info!(count = projects.len(), "jira projects discovered");
    Ok(JiraDiscoverResponse { projects })
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
    /// GitHub device flow only: the one-time code the user must enter at
    /// [`Self::verification_uri`]. `None` for the loopback-redirect providers
    /// (jira/trello), which need no user-visible code.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_code: Option<String>,
    /// GitHub device flow only: where the user enters [`Self::user_code`]
    /// (`https://github.com/login/device`). `None` for jira/trello.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verification_uri: Option<String>,
}

/// Start a browser-OAuth connect (the ported /api/auth/oauth/start POST).
/// `started=true` means the flow was launched; the UI then polls
/// [`get_oauth_status`] until the token store appears (success) or a `.error`
/// sentinel is written (failure).
///
/// **All providers run IN-PROCESS** via the shared `meridian-oauth` crate — no
/// `meridian oauth-login` subprocess (which depended on resolving the daemon
/// binary on launchd's PATH and on log-tailing to surface errors).
/// - **jira/trello**: loopback-redirect flow — the tray opens the browser, serves
///   the callback in its OWN runtime, and writes `~/.meridian/oauth/<provider>.json`.
/// - **github**: OAuth **device flow** — the tray requests a one-time code,
///   returns it (`user_code` + `verification_uri`) for the UI to display, opens
///   the browser, and polls for the token in the background, writing
///   `GITHUB_TOKEN` to `~/.meridian/.env` (the daemon reads it there). This
///   replaced the old `gh`-CLI subprocess, which ran the device flow headless so
///   the user never saw the code — and always timed out.
#[tauri::command]
#[tracing::instrument(fields(provider = %body.provider))]
pub async fn start_oauth(body: StartOAuthBody) -> Result<StartOAuthResponse, String> {
    match body.provider.as_str() {
        "jira" | "trello" => start_oauth_in_process(body.provider),
        "github" => start_oauth_github_device(body.provider).await,
        other => Err(format!("Unknown provider: {other}")),
    }
}

/// POST body for [`cancel_oauth`] (`{ provider }`).
#[derive(Debug, Deserialize)]
pub struct CancelOAuthBody {
    pub provider: String,
}

/// Cancel an in-flight jira/trello browser-OAuth attempt (the "Try again"
/// button's real action — see `IntegrationConnect.tsx`'s `OAuthSetup`).
///
/// Closing the browser tab mid-flow tells the tray nothing: the spawned task
/// stays parked on `TcpListener::accept()` for up to `CONSENT_TIMEOUT` (5 min,
/// `meridian-oauth/src/flow.rs`), holding both the loopback port and the
/// per-provider in-flight flag the whole time. Without this command, retrying
/// before that timeout elapses just re-hits the same "already in progress"
/// error start_oauth returns. `.abort()` drops the task (and its
/// `TcpListener`) immediately; we then clear the flag ourselves rather than
/// waiting for the (now never-running) task to reach its own cleanup code —
/// aborting a task skips the rest of its body entirely.
#[tauri::command]
#[tracing::instrument(fields(provider = %body.provider))]
pub async fn cancel_oauth(body: CancelOAuthBody) -> Result<(), String> {
    let provider = body.provider;
    let (in_flight, handle_slot): (
        &'static AtomicBool,
        &'static Mutex<Option<tokio::task::JoinHandle<()>>>,
    ) = match provider.as_str() {
        "trello" => (&TRELLO_OAUTH_IN_FLIGHT, &TRELLO_OAUTH_HANDLE),
        "jira" => (&JIRA_OAUTH_IN_FLIGHT, &JIRA_OAUTH_HANDLE),
        other => return Err(format!("cancel_oauth: unsupported provider {other}")),
    };
    if let Some(handle) = handle_slot.lock().unwrap().take() {
        handle.abort();
    }
    in_flight.store(false, Ordering::SeqCst);
    tracing::info!(provider = %provider, "OAuth attempt cancelled");
    Ok(())
}

/// Run the jira/trello browser login in-process on the tray's runtime. Returns
/// immediately (`started=true`); a spawned task drives the flow and writes the
/// token store on success or the `.error` sentinel (with the REAL error string —
/// no log-tail guessing) on failure, which [`get_oauth_status`] surfaces.
///
/// Credentials are read from `.env` and passed explicitly to `meridian_oauth`
/// functions — avoiding `std::env::set_var` on a Tokio worker thread (POSIX
/// setenv is not thread-safe under concurrent env reads). A per-provider
/// [`AtomicBool`] prevents two flows from racing to bind the same loopback port.
fn start_oauth_in_process(provider: String) -> Result<StartOAuthResponse, String> {
    // Resolve credentials from .env WITHOUT mutating process env.
    let mode = crate::install::detect_install_mode();
    let dot_env = mode.env_path().map(parse_env).unwrap_or_default();
    // The actual file the credential errors below should point at — this is
    // `~/.meridian/.env` for a canonical/packaged install, but the repo-root
    // `.env` in dev mode (`InstallMode::Dev`), and there may be no file at all
    // yet. Hardcoding "~/.meridian/.env" in the error text was misleading for
    // every dev-mode run, where it's silently the wrong path.
    let env_path_desc = match mode.env_path() {
        Some(p) => p.display().to_string(),
        None => "~/.meridian/.env (none found yet — create it)".to_string(),
    };
    let jira_secret = dot_env
        .get("JIRA_OAUTH_CLIENT_SECRET")
        .cloned()
        .unwrap_or_else(meridian_oauth::jira::client_secret);
    let jira_client_id = dot_env
        .get("JIRA_OAUTH_CLIENT_ID")
        .cloned()
        .unwrap_or_else(meridian_oauth::jira::client_id);
    let jira_port = dot_env
        .get("JIRA_OAUTH_REDIRECT_PORT")
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(meridian_oauth::jira::redirect_port);
    let trello_key = dot_env
        .get("TRELLO_APP_KEY")
        .cloned()
        .unwrap_or_else(meridian_oauth::trello::app_key);
    let trello_port = dot_env
        .get("TRELLO_OAUTH_REDIRECT_PORT")
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(meridian_oauth::trello::redirect_port);

    // Validate credentials before spawning — surface "not configured" immediately
    // rather than letting the user wait for a browser callback that never works.
    if provider == "jira" && jira_secret.trim().is_empty() {
        return Err(format!(
            "Jira OAuth requires a client secret baked in at build time \
             (MERIDIAN_JIRA_OAUTH_CLIENT_SECRET). Source builds must set \
             JIRA_OAUTH_CLIENT_SECRET in {env_path_desc}, or use the API-token path instead."
        ));
    }
    if provider == "trello" && trello_key.trim().is_empty() {
        return Err(format!(
            "Trello OAuth requires a Power-Up app key. Set TRELLO_APP_KEY in \
             {env_path_desc}. Register a Power-Up at https://trello.com/power-ups/admin \
             and add http://127.0.0.1:9123/ as an allowed origin."
        ));
    }

    // Claim the per-provider in-flight slot. A second start_oauth call while
    // a flow is running returns an error — port 9123 can't be shared and the
    // token file would be written by two racing tasks.
    let (in_flight, handle_slot): (
        &'static AtomicBool,
        &'static Mutex<Option<tokio::task::JoinHandle<()>>>,
    ) = match provider.as_str() {
        "trello" => (&TRELLO_OAUTH_IN_FLIGHT, &TRELLO_OAUTH_HANDLE),
        _ => (&JIRA_OAUTH_IN_FLIGHT, &JIRA_OAUTH_HANDLE),
    };
    if in_flight
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return Err(format!(
            "{provider} OAuth is already in progress — check your browser"
        ));
    }

    // Clear any previous error sentinel before launching a fresh flow.
    if let Some(sentinel) = oauth_error_path(&provider) {
        let _ = std::fs::remove_file(&sentinel);
    }

    let task_provider = provider.clone();
    // `tokio::spawn` (not `tauri::async_runtime::spawn`) so we get a real
    // `tokio::task::JoinHandle` — its `.abort()` is what `cancel_oauth` uses
    // to drop the task's `TcpListener` and free the loopback port on retry.
    let handle = tokio::spawn(async move {
        let result: anyhow::Result<()> = match task_provider.as_str() {
            "jira" => meridian_oauth::jira::login(&jira_client_id, &jira_secret, jira_port)
                .await
                .map(|_site_url| ()),
            "trello" => meridian_oauth::trello::login(&trello_key, trello_port).await,
            // Return an Err VALUE — `bail!` would `return` from the whole async
            // block, forcing it to be Result-typed and breaking the trailing match.
            _ => Err(anyhow::anyhow!("unhandled OAuth provider: {task_provider}")),
        };
        // Always clear the in-flight flag before returning, regardless of outcome.
        in_flight.store(false, Ordering::SeqCst);
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
    *handle_slot.lock().unwrap() = Some(handle);

    tracing::info!(provider = %provider, "in-process OAuth login launched");
    Ok(StartOAuthResponse {
        started: true,
        provider,
        user_code: None,
        verification_uri: None,
    })
}

/// Run the GitHub OAuth **device flow** in-process (replacing the old
/// `meridian oauth-login github` `gh`-CLI subprocess — that ran the device flow
/// headless, so the user never saw the one-time code and it always timed out).
///
/// Requests the device/user code synchronously so the UI can display it
/// immediately, then spawns a background task that polls for the token and — on
/// success — writes `GITHUB_TOKEN` to the active `.env` (the daemon reads it
/// there) and reloads the daemon. On failure it writes the `.error` sentinel that
/// [`get_oauth_status`] surfaces. Returns `user_code`/`verification_uri` for the UI.
///
/// The browser is **not** opened here: the connect UI's device-flow checklist
/// gates its "Open GitHub" button on the user first copying the code, and that
/// button is the sole opener (via `openExternal`). Auto-opening would jump the
/// user to GitHub before they've copied, contradicting the guided steps.
async fn start_oauth_github_device(provider: String) -> Result<StartOAuthResponse, String> {
    // Resolve the client id: `.env` override wins, else the baked-in default.
    // Reading .env (not process env) matches start_oauth_in_process — the daemon
    // may not have exported it into the tray's environment.
    let mode = crate::install::detect_install_mode();
    let dot_env = mode.env_path().map(parse_env).unwrap_or_default();
    let client_id = dot_env
        .get("GITHUB_OAUTH_CLIENT_ID")
        .cloned()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(meridian_oauth::github::client_id);

    // Claim the in-flight slot so two concurrent connects don't both poll / both
    // write GITHUB_TOKEN. Cleared inside the spawned task (success or failure).
    if GITHUB_OAUTH_IN_FLIGHT
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return Err("GitHub OAuth is already in progress — check your browser".to_string());
    }
    // From here on, every early return MUST release the slot.
    let release = || GITHUB_OAUTH_IN_FLIGHT.store(false, Ordering::SeqCst);

    // Clear any previous error sentinel before launching a fresh flow.
    if let Some(sentinel) = oauth_error_path(&provider) {
        let _ = std::fs::remove_file(&sentinel);
    }

    // Request the device/user code up front so failures (no client id, device
    // flow disabled, network down) surface immediately instead of via a poll
    // timeout, and so the UI can show the code without a second round trip.
    let device = match meridian_oauth::github::request_device_code(
        &client_id,
        meridian_oauth::github::REQUIRED_SCOPES,
    )
    .await
    {
        Ok(d) => d,
        Err(e) => {
            release();
            let msg = format!("{e:#}");
            tracing::warn!(error = %msg, "GitHub device-code request failed");
            return Err(msg);
        }
    };

    // NOTE: we deliberately do NOT open the browser here. The connect UI gates
    // the "Open GitHub" button on the user having first copied the one-time code
    // (see IntegrationConnect.tsx's device-flow checklist) — auto-opening the tab
    // before they copy contradicts that guided flow. The UI's button is now the
    // sole trigger (it calls openExternal with this verification_uri).

    let user_code = device.user_code.clone();
    let verification_uri = device.verification_uri.clone();

    tauri::async_runtime::spawn(async move {
        let result = meridian_oauth::github::poll_for_token(
            &client_id,
            &device.device_code,
            device.interval,
            device.expires_in,
        )
        .await;
        GITHUB_OAUTH_IN_FLIGHT.store(false, Ordering::SeqCst);

        match result {
            Ok(token) => {
                if let Err(e) = persist_github_token(token).await {
                    let msg = format!("{e:#}");
                    tracing::warn!(error = %msg, "persisting GITHUB_TOKEN failed");
                    if let Some(sentinel) = oauth_error_path("github") {
                        let _ = std::fs::write(&sentinel, &msg);
                    }
                    return;
                }
                tracing::info!("GitHub device-flow login succeeded");
                // Best-effort reload so the token takes effect now, not next restart.
                if let Err(e) = crate::commands::daemon::reload_daemon().await {
                    tracing::debug!(error = %e, "daemon reload after GitHub connect (non-fatal)");
                }
            }
            Err(e) => {
                let msg = format!("{e:#}");
                tracing::warn!(error = %msg, "GitHub device-flow login failed");
                if let Some(sentinel) = oauth_error_path("github") {
                    let _ = std::fs::write(&sentinel, &msg);
                }
            }
        }
    });

    tracing::info!(user_code = %user_code, "GitHub device flow launched");
    Ok(StartOAuthResponse {
        started: true,
        provider,
        user_code: Some(user_code),
        verification_uri: Some(verification_uri),
    })
}

/// Write `GITHUB_TOKEN` to the active `.env` through the SAME resolver
/// [`save_integration_token`] / [`get_integrations`] use, creating the canonical
/// `~/.meridian/.env` if none exists yet. Runs the synchronous `.env` write on
/// the blocking pool.
async fn persist_github_token(token: String) -> anyhow::Result<()> {
    let mode = crate::install::detect_install_mode();
    let env_path = match mode.env_path() {
        Some(p) => p.to_owned(),
        None => crate::install::canonical_env_path()
            .ok_or_else(|| anyhow::anyhow!("could not resolve home directory for .env"))?,
    };
    let mut updates: BTreeMap<String, String> = BTreeMap::new();
    updates.insert("GITHUB_TOKEN".to_string(), token);
    tokio::task::spawn_blocking(move || upsert_env(&env_path, &updates))
        .await
        .map_err(|e| anyhow::anyhow!("spawn_blocking panicked: {e}"))?
        .map_err(|e| anyhow::anyhow!("could not write .env: {e}"))
}

/// Status returned by [`get_oauth_status`].
#[derive(Debug, Serialize)]
pub struct OAuthStatus {
    pub connected: bool,
    pub error: Option<String>,
}

/// Poll the completion status of an OAuth login for `provider`.
///
/// For loopback providers (jira/trello): returns `connected=true` once
/// `~/.meridian/oauth/<provider>.json` exists. For the GitHub device-flow
/// provider: checks `GITHUB_TOKEN` in the active `.env`. On failure,
/// [`start_oauth`]'s background task writes the real `anyhow` error string to
/// `~/.meridian/oauth/<provider>.error` — this function reads that sentinel and
/// returns it as `error`. The UI polls every 2 s so failures surface immediately
/// instead of waiting for the full timeout.
///
/// # Who calls this
/// `ui/components/IntegrationConnect.tsx` `OAuthSetup`.
///
/// # Related
/// - [`start_oauth`] — launches the in-process OAuth flow (loopback or device).
/// - [`get_integrations`] — broader connected-status check (used for success).
#[tauri::command]
#[tracing::instrument]
pub async fn get_oauth_status(provider: String) -> Result<OAuthStatus, String> {
    if !OAUTH_PROVIDERS.contains(&provider.as_str())
        && !ENV_OAUTH_PROVIDERS.contains(&provider.as_str())
    {
        return Err(format!("Unknown provider: {provider}"));
    }
    // gh-CLI providers write a token to .env rather than a .json store.
    let connected = if ENV_OAUTH_PROVIDERS.contains(&provider.as_str()) {
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

    fn updates(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    fn env_of(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    /// The reported bug: after the GitHub OAuth device flow writes `GITHUB_TOKEN`
    /// to `.env`, the project picker submits ONLY `project_ids`. Judged against
    /// the payload alone that read as "Missing: GITHUB_TOKEN" and the picker
    /// could never save.
    #[test]
    fn github_project_ids_alone_are_enough_when_the_token_is_already_saved() {
        let missing = missing_required(
            "github",
            &updates(&[("GITHUB_PROJECT_IDS", "PVT_1,PVT_2")]),
            &env_of(&[("GITHUB_TOKEN", "gho_live_token")]),
            false,
        );
        assert!(
            missing.is_empty(),
            "a token already in .env must satisfy the requirement, got {missing:?}"
        );
    }

    /// The guard that stops this from being "just drop the requirement": with no
    /// GitHub connection anywhere, a project-ids-only save must still be refused.
    #[test]
    fn github_project_ids_alone_still_fail_with_no_token_anywhere() {
        let missing = missing_required(
            "github",
            &updates(&[("GITHUB_PROJECT_IDS", "PVT_1")]),
            &env_of(&[]),
            false,
        );
        assert_eq!(missing, vec!["GITHUB_TOKEN"]);
    }

    /// A placeholder left over from `.env.example` is not a connection — the
    /// check uses `is_set`, not bare presence, so it agrees with the
    /// connected-state test in `get_integrations`.
    #[test]
    fn github_placeholder_token_does_not_satisfy_the_requirement() {
        let missing = missing_required(
            "github",
            &updates(&[("GITHUB_PROJECT_IDS", "PVT_1")]),
            &env_of(&[("GITHUB_TOKEN", "your-token-here")]),
            false,
        );
        assert_eq!(missing, vec!["GITHUB_TOKEN"]);
    }

    /// The placeholder rule is symmetric across the two halves of the filter: a
    /// placeholder *submitted* in the payload is refused exactly like one already
    /// on disk. Without this, the save would succeed and `get_integrations` —
    /// which tests the written value with the same predicate — would immediately
    /// report GitHub disconnected.
    #[test]
    fn a_submitted_placeholder_token_is_refused_like_a_stored_one() {
        let missing = missing_required(
            "github",
            &updates(&[("GITHUB_TOKEN", "your-token-here")]),
            &env_of(&[]),
            false,
        );
        assert_eq!(missing, vec!["GITHUB_TOKEN"]);
    }

    /// The destructive case the sibling test above does NOT cover: it passes an
    /// EMPTY `.env`, so it held under the old logic too. With a valid token
    /// already on disk, falling back to `.env` for a key that WAS submitted let
    /// the placeholder through the check — and `upsert_env` then wrote it over
    /// the working credential. A submitted placeholder must be refused whether
    /// or not something valid is already stored.
    #[test]
    fn a_submitted_placeholder_cannot_overwrite_a_stored_token() {
        let missing = missing_required(
            "github",
            &updates(&[("GITHUB_TOKEN", "your-token-here")]),
            &env_of(&[("GITHUB_TOKEN", "ghp_valid_and_working")]),
            false,
        );
        assert_eq!(missing, vec!["GITHUB_TOKEN"]);
    }

    /// The first-time PAT connect keeps working: the token is in the payload and
    /// nothing is on disk yet.
    #[test]
    fn github_pat_connect_from_scratch_is_accepted() {
        let missing = missing_required(
            "github",
            &updates(&[("GITHUB_TOKEN", "ghp_fresh")]),
            &env_of(&[]),
            false,
        );
        assert!(missing.is_empty());
    }

    /// Multi-key providers report every still-unset key, and a partial submit on
    /// top of an already-complete `.env` is accepted (editing one field of a
    /// connected tracker no longer requires re-entering the others).
    #[test]
    fn jira_reports_every_unset_key_and_accepts_a_partial_edit() {
        assert_eq!(
            missing_required(
                "jira",
                &updates(&[("JIRA_EMAIL", "a@b.c")]),
                &env_of(&[]),
                false
            ),
            vec!["JIRA_BASE_URL", "JIRA_API_TOKEN"]
        );
        assert!(missing_required(
            "jira",
            &updates(&[("JIRA_EMAIL", "new@b.c")]),
            &env_of(&[
                ("JIRA_BASE_URL", "https://x.atlassian.net"),
                ("JIRA_API_TOKEN", "ATATT3x"),
            ]),
            false,
        )
        .is_empty());
    }

    /// The Jira analogue of the GitHub project-ids-alone test above, but for a
    /// browser-OAuth session rather than a token in `.env`: a projects-only save
    /// (no basic-auth fields in the payload) must be accepted once OAuth is
    /// connected, even though NONE of the three basic-auth keys are set anywhere.
    #[test]
    fn jira_project_keys_alone_are_enough_when_oauth_is_connected() {
        let missing = missing_required(
            "jira",
            &updates(&[("JIRA_PROJECT_KEYS", "KAN,ENG")]),
            &env_of(&[]),
            true,
        );
        assert!(
            missing.is_empty(),
            "an OAuth session must satisfy the requirement for a projects-only save, got {missing:?}"
        );
    }

    /// The guard for the above: with no OAuth session AND no basic auth anywhere,
    /// a projects-only save must still be refused.
    #[test]
    fn jira_project_keys_alone_still_fail_with_no_auth_anywhere() {
        let missing = missing_required(
            "jira",
            &updates(&[("JIRA_PROJECT_KEYS", "KAN")]),
            &env_of(&[]),
            false,
        );
        assert_eq!(
            missing,
            vec!["JIRA_BASE_URL", "JIRA_EMAIL", "JIRA_API_TOKEN"]
        );
    }

    /// A real (re)connect attempt — the payload includes a basic-auth field —
    /// must still be validated normally even when a stale OAuth session exists;
    /// `oauth_connected` only excuses a PURE projects-only submit.
    #[test]
    fn jira_basic_auth_submit_is_still_validated_even_when_oauth_connected() {
        let missing = missing_required(
            "jira",
            &updates(&[("JIRA_EMAIL", "a@b.c")]),
            &env_of(&[]),
            true,
        );
        assert_eq!(missing, vec!["JIRA_BASE_URL", "JIRA_API_TOKEN"]);
    }

    /// A provider with no required entry (trello: its app key is baked in for
    /// production builds) is unconstrained.
    #[test]
    fn a_provider_with_no_required_keys_is_unconstrained() {
        assert!(missing_required("trello", &updates(&[]), &env_of(&[]), false).is_empty());
    }

    /// [`parse_jira_projects_page`] pulls id/key/name from `values[]` and passes
    /// `isLast` through verbatim — this is the pagination termination signal
    /// [`discover_jira_projects`] loops on.
    #[test]
    fn parse_jira_projects_page_reads_values_and_is_last() {
        let body = serde_json::json!({
            "values": [
                {"id": "10001", "key": "KAN", "name": "Kanban Project"},
                {"id": "10002", "key": "ENG", "name": "Engineering"},
            ],
            "isLast": false,
        });
        let (projects, is_last) = parse_jira_projects_page(&body);
        assert_eq!(projects.len(), 2);
        assert_eq!(projects[0].key, "KAN");
        assert_eq!(projects[1].name, "Engineering");
        assert!(!is_last);
    }

    /// A body that doesn't match the expected shape (e.g. an unexpected error
    /// page) must stop pagination rather than looping — `is_last` defaults to
    /// `true` and no projects are returned.
    #[test]
    fn parse_jira_projects_page_fails_safe_on_unparseable_body() {
        let (projects, is_last) = parse_jira_projects_page(&serde_json::json!({"unexpected": 1}));
        assert!(projects.is_empty());
        assert!(is_last);
    }

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

    #[test]
    fn flatten_github_projects_merges_viewer_and_org_boards_sorted() {
        let body = serde_json::json!({
            "data": {
                "viewer": {
                    "login": "akarsh",
                    "projectsV2": { "nodes": [{ "id": "PVT_1", "title": "Zebra board" }] },
                    "organizations": { "nodes": [
                        {
                            "login": "meridiona",
                            "projectsV2": { "nodes": [
                                { "id": "PVT_2", "title": "Roadmap" },
                                { "id": "PVT_3", "title": "Apple board" },
                            ] },
                        },
                    ] },
                },
            },
        });
        let projects = flatten_github_projects(&body);
        assert_eq!(projects.len(), 3);
        // Sorted by (owner, title): akarsh's board first, then meridiona's two
        // alphabetically.
        assert_eq!(projects[0].id, "PVT_1");
        assert_eq!(projects[0].owner, "akarsh");
        assert_eq!(projects[1].id, "PVT_3");
        assert_eq!(projects[1].title, "Apple board");
        assert_eq!(projects[2].id, "PVT_2");
    }

    #[test]
    fn flatten_github_projects_empty_on_missing_viewer() {
        let body = serde_json::json!({ "errors": [{ "message": "bad credentials" }] });
        assert!(flatten_github_projects(&body).is_empty());
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

    // ── connected_from_env ────────────────────────────────────────────────
    //
    // This list is what `set_worklog_provider` validates against, so a provider
    // wrongly reported connected here means a worklog ticket filed at a tracker
    // with no usable credentials — a failure the user only sees after approving.

    /// No OAuth files — the default for these tests, so only env creds count.
    fn no_oauth(_: &str) -> bool {
        false
    }

    #[test]
    fn connected_from_env_reports_nothing_for_an_empty_env() {
        assert!(connected_from_env(&env_of(&[]), no_oauth).is_empty());
    }

    #[test]
    fn connected_from_env_reads_the_token_backed_trackers() {
        let env = env_of(&[("LINEAR_API_KEY", "lin_abc"), ("GITHUB_TOKEN", "ghp_abc")]);
        assert_eq!(connected_from_env(&env, no_oauth), vec!["linear", "github"]);
    }

    #[test]
    fn connected_from_env_keeps_the_canonical_provider_order() {
        // The UI renders trackers in this order (PROVIDER_IDS / TRACKERS); the two
        // must agree or the same set reads differently in two places.
        let env = env_of(&[
            ("GITHUB_TOKEN", "ghp_abc"),
            ("LINEAR_API_KEY", "lin_abc"),
            ("AZURE_DEVOPS_PAT", "pat"),
            ("AZURE_DEVOPS_ORG", "acme"),
            ("JIRA_BASE_URL", "https://x.atlassian.net"),
            ("JIRA_EMAIL", "a@b.c"),
            ("JIRA_API_TOKEN", "tok"),
        ]);
        assert_eq!(
            connected_from_env(&env, |p| p == "trello"),
            vec!["jira", "linear", "github", "trello", "azure_devops"]
        );
    }

    #[test]
    fn connected_from_env_needs_all_three_jira_basic_fields() {
        // Two of three is a half-configured tracker: it would pass a naive check
        // and then fail every API call.
        let partial = env_of(&[
            ("JIRA_BASE_URL", "https://x.atlassian.net"),
            ("JIRA_EMAIL", "a@b.c"),
        ]);
        assert!(connected_from_env(&partial, no_oauth).is_empty());
    }

    #[test]
    fn connected_from_env_accepts_jira_by_oauth_without_any_env_creds() {
        // The OAuth flow writes a token store, not .env keys.
        assert_eq!(
            connected_from_env(&env_of(&[]), |p| p == "jira"),
            vec!["jira"]
        );
    }

    #[test]
    fn connected_from_env_needs_a_host_alongside_the_azure_pat() {
        let pat_only = env_of(&[("AZURE_DEVOPS_PAT", "pat")]);
        assert!(connected_from_env(&pat_only, no_oauth).is_empty());

        // Any one of the three host spellings is enough.
        for host in [
            "AZURE_DEVOPS_URL",
            "AZURE_DEVOPS_ORG",
            "AZURE_DEVOPS_ORG_URL",
        ] {
            let env = env_of(&[("AZURE_DEVOPS_PAT", "pat"), (host, "acme")]);
            assert_eq!(
                connected_from_env(&env, no_oauth),
                vec!["azure_devops"],
                "{host} should satisfy the Azure host requirement"
            );
        }
    }

    #[test]
    fn connected_from_env_rejects_env_example_placeholders() {
        // A user who copied .env.example has a GITHUB_TOKEN line and no token. It
        // must not count as connected — `is_set` is what catches that, and this
        // pins that connected_from_env actually goes through it.
        let env = env_of(&[
            ("GITHUB_TOKEN", "your-token-here"),
            ("LINEAR_API_KEY", "lin_real_key"),
        ]);
        assert_eq!(connected_from_env(&env, no_oauth), vec!["linear"]);
    }
}
