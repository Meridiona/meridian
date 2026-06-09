// meridian — normalises screenpipe activity into structured app sessions
//
// Jira integration health (L2/L3). The auth probe (/myself) is the only thing
// that distinguishes an expired token from a transient blip — today the daemon
// collapses both into a silent warn. Sync freshness + candidate count come from
// meridian.db (content-free). Creds are read from the env (loaded via dotenv at
// startup), so this works without reaching into Config internals.

use crate::config::Config;
use crate::health::Check;
use crate::intelligence::oauth::{jira as oauth_jira, store as oauth_store};
use sqlx::SqlitePool;
use std::time::Duration;

/// Cache older than this (2× the 30-min sync interval) ⇒ fetch likely failing.
const SYNC_STALE_SECS: f64 = 3600.0;

pub async fn checks(_cfg: &Config, pool: Option<&SqlitePool>) -> Vec<Check> {
    let mut out = Vec::new();

    // OAuth takes precedence over basic auth (same order the daemon resolves).
    if oauth_store::exists("jira") {
        out.push(auth_oauth().await);
    } else {
        let base = std::env::var("JIRA_BASE_URL")
            .or_else(|_| std::env::var("JIRA_URL"))
            .ok()
            .filter(|s| !s.is_empty());
        let email = std::env::var("JIRA_EMAIL").ok().filter(|s| !s.is_empty());
        let token = std::env::var("JIRA_API_TOKEN")
            .ok()
            .filter(|s| !s.is_empty());

        match (base, email, token) {
            (Some(b), Some(e), Some(t)) => out.push(auth_basic(&b, &e, &t).await),
            _ => out.push(Check::info(
                "auth",
                "L2",
                "Jira not configured (no OAuth login, no JIRA_BASE_URL / EMAIL / API_TOKEN)",
            )),
        }
    }

    if let Some(p) = pool {
        out.push(sync_freshness(p).await);
        out.push(candidate_count(p).await);
    }
    out
}

fn http_client() -> Result<reqwest::Client, reqwest::Error> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(6))
        .build()
}

/// Probe `/myself` with a static API token (legacy basic auth).
async fn auth_basic(base: &str, email: &str, token: &str) -> Check {
    let url = format!("{}/rest/api/3/myself", base.trim_end_matches('/'));
    let client = match http_client() {
        Ok(c) => c,
        Err(e) => return Check::warn("auth", "L2", format!("http client error ({e})")),
    };
    classify_auth(client.get(&url).basic_auth(email, Some(token)).send().await).await
}

/// Probe `/myself` via the OAuth gateway, refreshing the access token first.
async fn auth_oauth() -> Check {
    let tokens = match oauth_jira::ensure_fresh().await {
        Ok(t) => t,
        Err(e) => {
            return Check::critical("auth", "L2", format!("OAuth token refresh failed ({e})"))
                .with_remedy("re-run `meridian oauth-login jira`")
        }
    };
    let url = format!(
        "https://api.atlassian.com/ex/jira/{}/rest/api/3/myself",
        tokens.cloud_id
    );
    let client = match http_client() {
        Ok(c) => c,
        Err(e) => return Check::warn("auth", "L2", format!("http client error ({e})")),
    };
    classify_auth(
        client
            .get(&url)
            .bearer_auth(&tokens.access_token)
            .send()
            .await,
    )
    .await
}

/// Map a `/myself` response into a health `Check`, shared by both auth paths.
async fn classify_auth(send: reqwest::Result<reqwest::Response>) -> Check {
    match send {
        Err(_) => Check::critical("auth", "L2", "Jira API unreachable")
            .with_remedy("check network connectivity and Jira base URL"),
        Ok(resp) => match resp.status().as_u16() {
            200 => {
                let body: serde_json::Value = resp.json().await.unwrap_or(serde_json::Value::Null);
                let who = body
                    .get("emailAddress")
                    .and_then(|v| v.as_str())
                    .or_else(|| body.get("displayName").and_then(|v| v.as_str()))
                    .unwrap_or("ok");
                Check::ok("auth", "L2", format!("credentials valid ({who})"))
            }
            401 => Check::critical("auth", "L2", "401 — token expired or invalid").with_remedy(
                "regenerate JIRA_API_TOKEN, or re-run `meridian oauth-login jira` for OAuth",
            ),
            403 => Check::critical("auth", "L2", "403 — token lacks required scope")
                .with_remedy("grant read:jira-work (and write:jira-work for worklogs)"),
            429 => {
                Check::warn("auth", "L2", "429 — rate limited").with_remedy("back off and retry")
            }
            s => Check::warn("auth", "L2", format!("unexpected HTTP {s}")),
        },
    }
}

async fn sync_freshness(pool: &SqlitePool) -> Check {
    match sqlx::query_scalar::<_, Option<f64>>(
        "SELECT (julianday('now') - julianday(MAX(last_synced_at))) * 86400.0
         FROM pm_sync_state WHERE provider = 'jira'",
    )
    .fetch_one(pool)
    .await
    {
        Ok(Some(age)) if age > SYNC_STALE_SECS => Check::warn(
            "ticket sync",
            "L3",
            format!(
                "cache {:.0}m stale — fetch may be failing silently",
                age / 60.0
            ),
        )
        .with_remedy("check the auth row above; the daemon refreshes every 30m"),
        Ok(Some(age)) => Check::ok(
            "ticket sync",
            "L3",
            format!("fresh ({:.0}m ago)", age / 60.0),
        ),
        Ok(None) => Check::info("ticket sync", "L3", "no Jira sync recorded yet"),
        Err(e) => Check::warn(
            "ticket sync",
            "L3",
            format!("could not read sync state ({e})"),
        ),
    }
}

async fn candidate_count(pool: &SqlitePool) -> Check {
    match sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM pm_tasks WHERE provider = 'jira'")
        .fetch_one(pool)
        .await
    {
        Ok(0) => Check::warn(
            "candidate tickets",
            "L3",
            "0 candidates — classifier can only return untracked/overhead",
        )
        .with_remedy("check the JQL / JIRA_PROJECT_KEYS; ensure assigned open issues exist"),
        Ok(n) if n >= 100 => Check::warn(
            "candidate tickets",
            "L3",
            format!("{n} (at the 100-result cap) — tickets beyond it are invisible"),
        )
        .with_remedy("the JQL fetch caps at 100; narrow it or add pagination"),
        Ok(n) => Check::ok("candidate tickets", "L3", format!("{n} open tickets")),
        Err(e) => Check::warn(
            "candidate tickets",
            "L3",
            format!("could not count pm_tasks ({e})"),
        ),
    }
}
