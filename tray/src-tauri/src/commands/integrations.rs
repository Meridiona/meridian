//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! `/api/integrations` GET ported to Rust — which trackers are connected.
//!
//! A faithful port of the route's GET: OAuth providers (jira/trello) are
//! detected by their token files under `~/.meridian/oauth/` (install-independent),
//! token providers (linear/github/azure) by their `.env` keys (placeholder
//! values don't count), and last sync errors come from the DB (meridian-core).
//!
//! Env-path note: env keys (linear/github/azure tokens) are read from `~/.meridian/.env`
//! (canonical, all install types) or a repo `.env` found by walking up from cwd (dev),
//! via `detect_install_mode`. Nothing is read on a bare `.app` launch.

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
