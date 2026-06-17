//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! `/api/integrations` GET ported to Rust — which trackers are connected.
//!
//! A faithful port of the route's GET: OAuth providers (jira/trello) are
//! detected by their token files under `~/.meridian/oauth/` (install-independent),
//! token providers (linear/github/azure) by their `.env` keys (placeholder
//! values don't count), and last sync errors come from the DB (meridian-core).
//!
//! Env-path note: the route used NODE_ENV to pick dev (repo `.env`) vs prod
//! (`~/.meridian/app/.env`). The tray mirrors the *daemon* instead — bundle
//! `.env` if present, else the first `.env` walking up from cwd (dotenvy's
//! behaviour) — so it reflects what the daemon actually reads. (Approximate when
//! several `.env`s coexist; the OAuth detection above is exact.)

use serde::Serialize;
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use tauri::State;

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

/// The `.env` the daemon reads: bundle `~/.meridian/app/.env` if present, else
/// the first `.env` walking up from cwd.
fn active_env_path() -> Option<PathBuf> {
    if let Some(bundle) = home().map(|h| h.join(".meridian/app/.env")) {
        if bundle.exists() {
            return Some(bundle);
        }
    }
    let mut dir = std::env::current_dir().ok()?;
    for _ in 0..8 {
        let candidate = dir.join(".env");
        if candidate.exists() {
            return Some(candidate);
        }
        if !dir.pop() {
            break;
        }
    }
    None
}

fn parse_env(path: &PathBuf) -> HashMap<String, String> {
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
    let env = active_env_path().map(|p| parse_env(&p)).unwrap_or_default();

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
