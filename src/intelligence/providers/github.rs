// meridian — normalises screenpipe activity into structured app sessions
//
// GitHub task connector. Pulls the OPEN issues assigned to the authenticated
// user into `pm_tasks` so the classifier can link sessions to them and the
// worklog driver can draft against them. Mirrors the Jira/Linear connectors:
// fetch → filter → upsert → prune, gated by `pm_sync_state`.
//
// We use the REST search API (`GET /search/issues?q=assignee:@me is:issue
// is:open`) which returns the user's assigned issues across every repo they can
// see, then scope in Rust to the configured org (or the explicit repo list) by
// parsing each item's `repository_url`. This sidesteps the org-vs-user search
// qualifier ambiguity (a configured owner may be a personal account or an org).
//
// task_key is `owner/repo#number` — the worklog poster parses it back to hit the
// issue-comments endpoint. GitHub issues have no native time tracking, so a
// "worklog" is posted as a structured issue comment (see pm_worklog/github.rs).

use anyhow::{Context, Result};
use serde::Deserialize;
use sqlx::SqlitePool;

use crate::config::GitHubConfig;

const SEARCH_URL: &str = "https://api.github.com/search/issues";
const MAX_RESULTS: usize = 100;
const SYNC_INTERVAL_MINS: i64 = 5;

// ---------------------------------------------------------------------------
// REST search response shapes
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct SearchResponse {
    items: Vec<SearchItem>,
}

#[derive(Deserialize)]
struct SearchItem {
    number: u64,
    title: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    html_url: Option<String>,
    repository_url: String,
    updated_at: String,
    #[serde(default)]
    assignee: Option<GhUser>,
    /// Present only on pull requests — used to skip PRs defensively.
    #[serde(default)]
    pull_request: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct GhUser {
    #[serde(default)]
    login: Option<String>,
}

/// One normalised, in-scope issue ready to upsert.
struct GhTask {
    task_key: String,
    repo_slug: String,
    title: String,
    body: String,
    status: &'static str,
    url: String,
    updated_at: String,
    assignee: Option<String>,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Extract `owner/repo` from a `https://api.github.com/repos/owner/repo` URL.
fn repo_slug_from_url(repository_url: &str) -> Option<String> {
    let tail = repository_url.split("/repos/").nth(1)?;
    let mut parts = tail.split('/');
    let owner = parts.next()?;
    let repo = parts.next()?;
    if owner.is_empty() || repo.is_empty() {
        return None;
    }
    Some(format!("{owner}/{repo}"))
}

/// Decide whether an in-scope item belongs to this connector's configuration.
/// If explicit repos are configured, the slug must be one of them; otherwise the
/// repo owner must equal the configured org/user.
fn in_scope(repo_slug: &str, github: &GitHubConfig) -> bool {
    if !github.repos.is_empty() {
        return github.repos.iter().any(|r| r == repo_slug);
    }
    repo_slug
        .split_once('/')
        .map(|(owner, _)| owner.eq_ignore_ascii_case(&github.org))
        .unwrap_or(false)
}

/// Map a GitHub issue state to meridian's status_category. Issues are open or
/// closed; finer status only exists inside a Project board (not modelled here).
fn map_state(state: Option<&str>) -> &'static str {
    match state {
        Some("closed") => "done",
        _ => "todo",
    }
}

/// Normalise a raw search item into a `GhTask`, or `None` if it is a PR / out of
/// scope / malformed.
fn normalise(item: &SearchItem, github: &GitHubConfig) -> Option<GhTask> {
    if item.pull_request.is_some() {
        return None; // PRs are not tasks
    }
    let repo_slug = repo_slug_from_url(&item.repository_url)?;
    if !in_scope(&repo_slug, github) {
        return None;
    }
    Some(GhTask {
        task_key: format!("{repo_slug}#{}", item.number),
        repo_slug: repo_slug.clone(),
        title: item.title.clone(),
        body: item.body.clone().unwrap_or_default(),
        status: map_state(item.state.as_deref()),
        url: item
            .html_url
            .clone()
            .unwrap_or_else(|| format!("https://github.com/{repo_slug}/issues/{}", item.number)),
        updated_at: item.updated_at.clone(),
        assignee: item.assignee.as_ref().and_then(|u| u.login.clone()),
    })
}

// ---------------------------------------------------------------------------
// Fetch
// ---------------------------------------------------------------------------

#[tracing::instrument(
    skip(github),
    fields(provider = "github", status_code = tracing::field::Empty)
)]
async fn fetch(github: &GitHubConfig) -> Result<Vec<SearchItem>> {
    let client = reqwest::Client::new();
    let resp = client
        .get(SEARCH_URL)
        .query(&[
            ("q", "assignee:@me is:issue is:open archived:false"),
            ("per_page", "100"),
            ("sort", "updated"),
            ("order", "desc"),
        ])
        .header("Authorization", format!("Bearer {}", github.token))
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .header("User-Agent", "meridian")
        .send()
        .await
        .context("GET /search/issues")?;

    let status = resp.status();
    tracing::Span::current().record("status_code", status.as_u16() as i64);
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        anyhow::bail!("GitHub /search/issues → {}: {}", status, text);
    }
    let parsed: SearchResponse =
        serde_json::from_str(&text).context("deserialising GitHub search")?;
    tracing::debug!(count = parsed.items.len(), "parsed GitHub search response");
    Ok(parsed.items)
}

// ---------------------------------------------------------------------------
// Upsert
// ---------------------------------------------------------------------------

async fn upsert(pool: &SqlitePool, tasks: &[GhTask]) -> Result<()> {
    for t in tasks {
        sqlx::query(
            "INSERT INTO pm_tasks
               (task_key, provider, title, description_text, status_category,
                issue_type, project_key, url, assignee_name, updated_at, fetched_at)
             VALUES (?, 'github', ?, ?, ?, 'Issue', ?, ?, ?, ?,
                     strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
             ON CONFLICT(task_key) DO UPDATE SET
               provider         = 'github',
               title            = excluded.title,
               description_text = excluded.description_text,
               status_category  = excluded.status_category,
               project_key      = excluded.project_key,
               url              = excluded.url,
               assignee_name    = excluded.assignee_name,
               updated_at       = excluded.updated_at,
               fetched_at       = excluded.fetched_at",
        )
        .bind(&t.task_key)
        .bind(&t.title)
        .bind(&t.body)
        .bind(t.status)
        .bind(&t.repo_slug)
        .bind(&t.url)
        .bind(&t.assignee)
        .bind(&t.updated_at)
        .execute(pool)
        .await
        .with_context(|| format!("upserting {}", t.task_key))?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Prune (scoped to provider = 'github')
// ---------------------------------------------------------------------------

async fn prune(pool: &SqlitePool, fetched_keys: &[String]) -> Result<usize> {
    let placeholders = fetched_keys
        .iter()
        .map(|_| "?")
        .collect::<Vec<_>>()
        .join(",");

    let emb_sql = format!(
        "DELETE FROM pm_task_embeddings WHERE task_key IN \
         (SELECT task_key FROM pm_tasks WHERE provider = 'github' AND task_key NOT IN ({placeholders}))"
    );
    let mut q = sqlx::query(&emb_sql);
    for key in fetched_keys {
        q = q.bind(key.as_str());
    }
    q.execute(pool)
        .await
        .context("pruning github pm_task_embeddings")?;

    let task_sql = format!(
        "DELETE FROM pm_tasks WHERE provider = 'github' AND task_key NOT IN ({placeholders})"
    );
    let mut q = sqlx::query(&task_sql);
    for key in fetched_keys {
        q = q.bind(key.as_str());
    }
    let result = q.execute(pool).await.context("pruning github pm_tasks")?;
    Ok(result.rows_affected() as usize)
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

#[tracing::instrument(skip(pool, github))]
pub async fn refresh_if_stale(
    pool: &SqlitePool,
    github: &GitHubConfig,
) -> Result<Option<Vec<String>>> {
    let threshold = format!("-{SYNC_INTERVAL_MINS} minutes");
    let (is_fresh,): (i64,) = sqlx::query_as(
        "SELECT EXISTS(
             SELECT 1 FROM pm_sync_state
             WHERE provider = 'github'
               AND last_synced_at > strftime('%Y-%m-%dT%H:%M:%SZ', 'now', ?)
         )",
    )
    .bind(&threshold)
    .fetch_one(pool)
    .await
    .context("checking github sync state")?;

    if is_fresh != 0 {
        return Ok(None);
    }

    match fetch(github).await {
        Ok(items) => {
            let raw_count = items.len();
            let tasks: Vec<GhTask> = items.iter().filter_map(|i| normalise(i, github)).collect();
            let keys: Vec<String> = tasks.iter().map(|t| t.task_key.clone()).collect();
            upsert(pool, &tasks).await?;
            sqlx::query(
                "INSERT INTO pm_sync_state (provider, last_synced_at)
                 VALUES ('github', strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
                 ON CONFLICT(provider) DO UPDATE SET last_synced_at = excluded.last_synced_at",
            )
            .execute(pool)
            .await
            .context("updating github sync state")?;

            if raw_count < MAX_RESULTS {
                if !keys.is_empty() {
                    match prune(pool, &keys).await {
                        Ok(0) => {}
                        Ok(p) => tracing::info!(pruned_count = p, "pruned stale github tasks"),
                        Err(e) => tracing::warn!(error = %e, "github prune failed"),
                    }
                } else if let Err(e) = sqlx::query("DELETE FROM pm_tasks WHERE provider = 'github'")
                    .execute(pool)
                    .await
                {
                    tracing::warn!(error = %e, "github full-clear failed");
                }
            }
            tracing::info!(upserted_count = keys.len(), "github tasks refreshed");
            Ok(Some(keys))
        }
        Err(e) => {
            tracing::warn!(error = %e, "github fetch failed — keeping stale cache");
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(org: &str, repos: &[&str]) -> GitHubConfig {
        GitHubConfig {
            token: "x".into(),
            org: org.into(),
            repos: repos.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn slug_parsing() {
        assert_eq!(
            repo_slug_from_url("https://api.github.com/repos/acme/api").as_deref(),
            Some("acme/api")
        );
        assert_eq!(
            repo_slug_from_url("https://api.github.com/repos/acme").as_deref(),
            None
        );
        assert_eq!(repo_slug_from_url("garbage").as_deref(), None);
    }

    #[test]
    fn scope_by_org_case_insensitive() {
        assert!(in_scope("Acme/api", &cfg("acme", &[])));
        assert!(!in_scope("other/api", &cfg("acme", &[])));
    }

    #[test]
    fn scope_by_explicit_repos() {
        let c = cfg("acme", &["acme/api", "acme/web"]);
        assert!(in_scope("acme/api", &c));
        assert!(!in_scope("acme/infra", &c)); // not in the list, even though same org
    }

    #[test]
    fn state_mapping() {
        assert_eq!(map_state(Some("open")), "todo");
        assert_eq!(map_state(Some("closed")), "done");
        assert_eq!(map_state(None), "todo");
    }

    #[test]
    fn normalise_builds_task_key_and_skips_prs() {
        let item: SearchItem = serde_json::from_str(
            r#"{"number":42,"title":"Fix it","body":"do x","state":"open",
                "html_url":"https://github.com/acme/api/issues/42",
                "repository_url":"https://api.github.com/repos/acme/api",
                "updated_at":"2026-06-01T00:00:00Z","assignee":{"login":"sam"}}"#,
        )
        .unwrap();
        let t = normalise(&item, &cfg("acme", &[])).unwrap();
        assert_eq!(t.task_key, "acme/api#42");
        assert_eq!(t.repo_slug, "acme/api");
        assert_eq!(t.assignee.as_deref(), Some("sam"));

        let pr: SearchItem = serde_json::from_str(
            r#"{"number":7,"title":"PR","repository_url":"https://api.github.com/repos/acme/api",
                "updated_at":"2026-06-01T00:00:00Z","pull_request":{"url":"x"}}"#,
        )
        .unwrap();
        assert!(normalise(&pr, &cfg("acme", &[])).is_none());
    }
}
