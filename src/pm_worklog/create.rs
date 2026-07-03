//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Ticket CREATION across providers — the write-back path a tier-3 proposal takes
// when the user approves it. Mirrors the auth of each provider's `post_worklog`,
// but hits the create endpoint instead of the worklog/comment endpoint, and
// returns the new ticket's `task_key` (the same human identifier the rest of the
// pipeline uses). The approved-proposal sweep (`post::process_approved_proposals`)
// calls [`create_ticket`], stamps the key onto the proposal, then drafts an
// approved worklog so the normal post sweep comments on the new ticket.
//
// Creation TARGET resolution per provider:
//   jira   → first configured project key            (JiraConfig.project_keys)
//   linear → first configured team id                (LinearConfig.team_ids)
//   azure  → configured project                      (AzureDevOpsConfig.project)
//   github → owner/repo parsed from `sample_key`     (an existing GitHub task)
//   trello → first list of the first board           (TrelloConfig.board_ids)
// A target that can't be resolved is a hard error (surfaced on the proposal), so
// we never create a ticket in the wrong place.

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};

use crate::config::{
    AzureDevOpsConfig, Config, GitHubConfig, JiraConfig, LinearConfig, PmProviderConfig,
    TrelloConfig,
};
use crate::intelligence::oauth::jira::resolve as jira_resolve;
use crate::intelligence::oauth::trello as oauth_trello;

/// Create a real ticket for `provider` from a proposal's (title, description) and
/// return its `task_key`. `sample_key` is any existing task_key of that provider
/// (used to resolve GitHub's owner/repo); ignored by providers that carry their
/// target in config. `issue_type` is the proposed type (`Task` / `Bug`); only
/// Jira and Azure DevOps model an issue type natively — Linear/GitHub/Trello have
/// no type concept and ignore it.
pub async fn create_ticket(
    config: &Config,
    provider: &str,
    title: &str,
    description: &str,
    issue_type: &str,
    sample_key: Option<&str>,
) -> Result<String> {
    match provider {
        "jira" => jira_create(jira_cfg(config)?, title, description, issue_type).await,
        "linear" => linear_create(linear_cfg(config)?, title, description).await,
        "github" => github_create(github_cfg(config)?, title, description, sample_key).await,
        "trello" => trello_create(trello_cfg(config)?, title, description).await,
        "azure_devops" => azure_create(azure_cfg(config)?, title, description, issue_type).await,
        other => bail!("create_ticket: unknown provider '{other}'"),
    }
}

/// Normalise a proposed issue type to the canonical pair we create. The proposer
/// emits `Task` or `Bug`; anything unexpected falls back to `Task`.
fn norm_issue_type(issue_type: &str) -> &'static str {
    if issue_type.eq_ignore_ascii_case("bug") {
        "Bug"
    } else {
        "Task"
    }
}

// ── Config finders ────────────────────────────────────────────────────────────

fn jira_cfg(c: &Config) -> Result<&JiraConfig> {
    c.pm_providers
        .iter()
        .find_map(|p| match p {
            PmProviderConfig::Jira(j) => Some(j),
            _ => None,
        })
        .context("Jira is not configured on this daemon")
}
fn linear_cfg(c: &Config) -> Result<&LinearConfig> {
    c.pm_providers
        .iter()
        .find_map(|p| match p {
            PmProviderConfig::Linear(l) => Some(l),
            _ => None,
        })
        .context("Linear is not configured on this daemon")
}
fn github_cfg(c: &Config) -> Result<&GitHubConfig> {
    c.pm_providers
        .iter()
        .find_map(|p| match p {
            PmProviderConfig::GitHub(g) => Some(g),
            _ => None,
        })
        .context("GitHub is not configured on this daemon")
}
fn trello_cfg(c: &Config) -> Result<&TrelloConfig> {
    c.pm_providers
        .iter()
        .find_map(|p| match p {
            PmProviderConfig::Trello(t) => Some(t),
            _ => None,
        })
        .context("Trello is not configured on this daemon")
}
fn azure_cfg(c: &Config) -> Result<&AzureDevOpsConfig> {
    c.pm_providers
        .iter()
        .find_map(|p| match p {
            PmProviderConfig::AzureDevOps(a) => Some(a),
            _ => None,
        })
        .context("Azure DevOps is not configured on this daemon")
}

// ── Jira: POST /rest/api/3/issue ──────────────────────────────────────────────

async fn jira_create(
    jira: &JiraConfig,
    title: &str,
    description: &str,
    issue_type: &str,
) -> Result<String> {
    let project = jira
        .project_keys
        .first()
        .context("Jira create needs a project key (none configured)")?;
    let payload = json!({
        "fields": {
            "project": { "key": project },
            "summary": title,
            "issuetype": { "name": norm_issue_type(issue_type) },
            "description": {
                "type": "doc", "version": 1,
                "content": [ { "type": "paragraph",
                    "content": [ { "type": "text", "text": description } ] } ]
            }
        }
    });
    let ctx = jira_resolve(jira).await.context("resolving Jira auth")?;
    let url = ctx.api_url("/rest/api/3/issue");
    let client = reqwest::Client::new();
    let resp = ctx
        .apply(client.post(&url))
        .header("Accept", "application/json")
        .json(&payload)
        .send()
        .await
        .with_context(|| format!("network error reaching Jira at {url}"))?;
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        bail!("Jira create returned {status}: {body}");
    }
    let v: Value = serde_json::from_str(&body).context("parsing Jira create response")?;
    v.get("key")
        .and_then(|k| k.as_str())
        .map(str::to_string)
        .context("Jira create response missing `key`")
}

// ── Linear: GraphQL issueCreate ───────────────────────────────────────────────

async fn linear_create(linear: &LinearConfig, title: &str, description: &str) -> Result<String> {
    let team = linear
        .team_ids
        .first()
        .context("Linear create needs a team id (none configured)")?;
    let query = "mutation($teamId:String!,$title:String!,$desc:String){\
        issueCreate(input:{teamId:$teamId,title:$title,description:$desc})\
        { success issue { identifier } } }";
    let payload = json!({
        "query": query,
        "variables": { "teamId": team, "title": title, "desc": description }
    });
    let client = reqwest::Client::new();
    let resp = client
        .post("https://api.linear.app/graphql")
        .header("Authorization", &linear.api_key)
        .header("Content-Type", "application/json")
        .json(&payload)
        .send()
        .await
        .context("network error reaching Linear")?;
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        bail!("Linear create returned {status}: {body}");
    }
    let v: Value = serde_json::from_str(&body).context("parsing Linear create response")?;
    v.pointer("/data/issueCreate/issue/identifier")
        .and_then(|i| i.as_str())
        .map(str::to_string)
        .with_context(|| format!("Linear create returned no identifier: {body}"))
}

// ── GitHub: POST /repos/{owner}/{repo}/issues ─────────────────────────────────

async fn github_create(
    github: &GitHubConfig,
    title: &str,
    description: &str,
    sample_key: Option<&str>,
) -> Result<String> {
    // Derive owner/repo from an existing GitHub task (config carries Projects-v2
    // node ids, not a repo). `sample_key` is `owner/repo#123`.
    let sample = sample_key
        .context("GitHub create needs an existing repo (no GitHub task to infer owner/repo)")?;
    let item = super::github::parse_task_key(sample).context("parsing GitHub sample key")?;
    let url = format!(
        "https://api.github.com/repos/{}/{}/issues",
        item.owner, item.repo
    );
    let client = reqwest::Client::new();
    let resp = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", github.token))
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .header("User-Agent", "meridian")
        .json(&json!({ "title": title, "body": description }))
        .send()
        .await
        .context("network error reaching GitHub")?;
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        bail!("GitHub create returned {status}: {body}");
    }
    let v: Value = serde_json::from_str(&body).context("parsing GitHub create response")?;
    let number = v
        .get("number")
        .and_then(|n| n.as_i64())
        .context("GitHub create response missing `number`")?;
    Ok(format!("{}/{}#{number}", item.owner, item.repo))
}

// ── Trello: POST /cards (resolve a list from the first board) ──────────────────

async fn trello_create(trello: &TrelloConfig, title: &str, description: &str) -> Result<String> {
    let board = trello
        .board_ids
        .first()
        .context("Trello create needs a board id (none configured)")?;
    let token = oauth_trello::load_token().context("loading Trello OAuth token")?;
    let client = reqwest::Client::new();

    // A card is created in a LIST, not a board — fetch the board's first list.
    let lists_url = format!(
        "https://api.trello.com/1/boards/{board}/lists?key={}&token={token}",
        trello.app_key
    );
    let lists: Value = client
        .get(&lists_url)
        .send()
        .await
        .context("network error listing Trello lists")?
        .json()
        .await
        .context("parsing Trello lists")?;
    let list_id = lists
        .as_array()
        .and_then(|a| a.first())
        .and_then(|l| l.get("id"))
        .and_then(|i| i.as_str())
        .context("Trello board has no lists to create a card in")?;

    let create_url = format!(
        "https://api.trello.com/1/cards?idList={list_id}&key={}&token={token}",
        trello.app_key
    );
    let resp = client
        .post(&create_url)
        .query(&[("name", title), ("desc", description)])
        .send()
        .await
        .context("network error creating Trello card")?;
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        bail!("Trello create returned {status}: {body}");
    }
    let v: Value = serde_json::from_str(&body).context("parsing Trello create response")?;
    v.get("shortLink")
        .and_then(|s| s.as_str())
        .map(str::to_string)
        .context("Trello create response missing `shortLink`")
}

// ── Azure DevOps: POST /_apis/wit/workitems/$Task ─────────────────────────────

async fn azure_create(
    cfg: &AzureDevOpsConfig,
    title: &str,
    description: &str,
    issue_type: &str,
) -> Result<String> {
    use base64::Engine;
    // Azure work-item type goes in the URL (`$Task` / `$Bug`).
    let url = format!(
        "{}/{}/_apis/wit/workitems/${}?api-version=7.0",
        cfg.api_base,
        cfg.project,
        norm_issue_type(issue_type)
    );
    let auth = format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(format!(":{}", cfg.pat).as_bytes())
    );
    let patch = json!([
        { "op": "add", "path": "/fields/System.Title", "value": title },
        { "op": "add", "path": "/fields/System.Description", "value": description }
    ]);
    let client = reqwest::Client::new();
    let resp = client
        .post(&url)
        .header("Authorization", auth)
        .header("Content-Type", "application/json-patch+json")
        .json(&patch)
        .send()
        .await
        .context("network error reaching Azure DevOps")?;
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        bail!("Azure DevOps create returned {status}: {body}");
    }
    let v: Value = serde_json::from_str(&body).context("parsing Azure create response")?;
    let id = v
        .get("id")
        .and_then(|i| i.as_i64())
        .context("Azure create response missing `id`")?;
    Ok(id.to_string())
}
