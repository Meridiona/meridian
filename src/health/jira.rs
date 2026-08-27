//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
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
    let tokens =
        match oauth_jira::ensure_fresh().await {
            Ok(t) => t,
            // A transient refresh failure (network/timeout/429/5xx) is not a dead
            // token — reporting it as critical "re-run oauth-login" is the same
            // false alarm the sync path used to raise. Downgrade to a warn; only a
            // terminal failure tells the user to re-authenticate.
            Err(e) if meridian_oauth::is_transient(&e) => return Check::warn(
                "auth",
                "L2",
                format!("OAuth token refresh temporarily failed ({e})"),
            )
            .with_remedy(
                "transient network or provider (429/5xx) issue; the daemon retries automatically",
            ),
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

/// Report whether the last Jira sync FAILED, not whether it was long ago.
///
/// This used to warn purely on elapsed time (`age > 3600s` => "fetch may be
/// failing silently"), which was a reasonable proxy only while a background
/// timer synced every few minutes no matter what. PM sync is on-demand now (see
/// [`crate::intelligence::run_pm_sync`]): a machine that was shut all weekend,
/// or a user who has not opened the dashboard since Friday, has a legitimately
/// old cache and nothing is wrong. Warning on that is a false alarm nobody can
/// act on - and a check that cries wolf during normal operation is worse than no
/// check, because it teaches people to ignore the one that matters.
///
/// So the signal is the OUTCOME instead: `pm_sync_state.last_error`, written by
/// `providers::record_sync_failure` and cleared by `clear_sync_error`. A recorded
/// failure warns and carries the provider's own message; anything else reports
/// elapsed time as context only, never as a fault.
async fn sync_freshness(pool: &SqlitePool) -> Check {
    match sqlx::query_as::<_, (Option<f64>, Option<String>)>(
        "SELECT (julianday('now') - julianday(last_synced_at)) * 86400.0, last_error
         FROM pm_sync_state WHERE provider = 'jira'",
    )
    .fetch_optional(pool)
    .await
    {
        // A recorded failure is the ONLY thing that warns. `last_error` is set by
        // the provider itself, so the text is the real cause rather than this
        // check's guess at one.
        Ok(Some((_, Some(err)))) if !err.trim().is_empty() => {
            Check::warn("ticket sync", "L3", format!("last sync failed: {err}")).with_remedy(
                "check the auth row above; a sync retries on the next on-demand trigger",
            )
        }
        // Elapsed time as context. An old cache is normal when nothing asked for a sync.
        Ok(Some((Some(age), _))) => Check::ok(
            "ticket sync",
            "L3",
            format!("last synced {:.0}m ago, no errors", age / 60.0),
        ),
        // A row with neither a parseable timestamp nor an error: nothing to report.
        Ok(Some((None, _))) => {
            Check::info("ticket sync", "L3", "no successful Jira sync recorded yet")
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::health::Severity;
    use sqlx::sqlite::SqlitePoolOptions;

    async fn db() -> SqlitePool {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::migrate!("src/migrations").run(&pool).await.unwrap();
        pool
    }

    /// Seed `pm_sync_state` with a sync `days_ago` in the past and an optional
    /// recorded error.
    async fn seed(pool: &SqlitePool, days_ago: f64, last_error: Option<&str>) {
        sqlx::query(
            "INSERT INTO pm_sync_state (provider, last_synced_at, last_error)
             VALUES ('jira', strftime('%Y-%m-%dT%H:%M:%SZ', 'now', ?), ?)",
        )
        .bind(format!("-{days_ago} days"))
        .bind(last_error)
        .execute(pool)
        .await
        .unwrap();
    }

    /// THE REGRESSION THIS EXISTS FOR. A three-day-old cache with no recorded
    /// failure is normal once syncing is on-demand - the machine was shut, or
    /// nobody opened the dashboard. The old elapsed-time rule warned at one hour,
    /// which would now fire on healthy installs every Monday morning.
    #[tokio::test]
    async fn a_long_quiet_period_with_no_error_is_not_a_fault() {
        let pool = db().await;
        seed(&pool, 3.0, None).await;
        let check = sync_freshness(&pool).await;
        assert_eq!(
            check.severity,
            Severity::Ok,
            "a stale-but-successful sync must not warn, got {check:?}"
        );
    }

    /// A recorded failure warns and carries the provider's own message, so the
    /// user sees the real cause rather than this check's guess at one.
    #[tokio::test]
    async fn a_recorded_failure_warns_with_the_providers_own_message() {
        let pool = db().await;
        seed(&pool, 0.01, Some("refresh_token is invalid")).await;
        let check = sync_freshness(&pool).await;
        assert_eq!(
            check.severity,
            Severity::Warn,
            "a recorded failure must warn"
        );
        assert!(
            check.detail.contains("refresh_token is invalid"),
            "the provider's message must survive into the detail, got {:?}",
            check.detail
        );
    }

    /// An empty string is not a failure. `clear_sync_error` blanks the column on
    /// success on some paths, and treating `''` as an error would leave a
    /// permanent warn on a perfectly healthy install.
    #[tokio::test]
    async fn a_blank_last_error_is_not_a_failure() {
        let pool = db().await;
        seed(&pool, 0.5, Some("   ")).await;
        let check = sync_freshness(&pool).await;
        assert_eq!(
            check.severity,
            Severity::Ok,
            "a blank last_error must not warn, got {check:?}"
        );
    }

    /// No row at all - a tracker that has never synced. Informational, never a
    /// fault: this is every fresh install for its first few minutes.
    #[tokio::test]
    async fn never_synced_is_informational() {
        let pool = db().await;
        let check = sync_freshness(&pool).await;
        assert_eq!(
            check.severity,
            Severity::Info,
            "never-synced must be info, got {check:?}"
        );
    }
}
