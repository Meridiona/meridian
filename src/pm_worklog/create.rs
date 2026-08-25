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
//   jira   → first configured project key, else the   (JiraConfig.project_keys
//            project inferred from an existing Jira      or `sample_key` prefix)
//            task_key (OAuth users have no project key)
//   linear → first configured team id                (LinearConfig.team_ids)
//   azure  → configured project                      (AzureDevOpsConfig.project)
//   github → owner/repo from `sample_key`, else the  (see `create_github`)
//            single repository linked to the
//            configured Projects v2 board
//   trello → first list of the first board           (TrelloConfig.board_ids)
// A target that can't be resolved is a hard error (surfaced on the proposal), so
// we never create a ticket in the wrong place.
//
// GitHub is the one provider whose create is more than a POST: its sync only
// keeps board items assigned to the viewer, so a bare issue is invisible to
// Meridian forever. That whole flow (repo resolution, self-assign, add to the
// board) lives in [`super::create_github`].

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};

use crate::config::{
    AzureDevOpsConfig, Config, GitHubConfig, JiraConfig, LinearConfig, PmProviderConfig,
    TrelloConfig,
};
use crate::intelligence::oauth::jira::resolve as jira_resolve;
use crate::intelligence::oauth::trello as oauth_trello;
use crate::intelligence::ticket_update::azure_devops as azure_ticket_update;
use crate::intelligence::ticket_update::jira as jira_ticket_update;

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
        "jira" => {
            jira_create(
                jira_cfg(config)?,
                title,
                description,
                issue_type,
                sample_key,
            )
            .await
        }
        "linear" => linear_create(linear_cfg(config)?, title, description).await,
        "github" => {
            super::create_github::create(github_cfg(config)?, title, description, sample_key).await
        }
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

/// Resolve what to actually create when the wanted type is unavailable in the
/// target project/process (e.g. no "Bug" type configured — common on
/// team-managed Jira projects and Azure's Basic process template). Falls back
/// to "Task" with the type prefixed onto the title so the information isn't
/// silently lost.
fn bug_fallback(wanted: &'static str, has_bug_type: bool, title: &str) -> (&'static str, String) {
    if wanted == "Bug" && !has_bug_type {
        ("Task", format!("Bug: {title}"))
    } else {
        (wanted, title.to_string())
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
    sample_key: Option<&str>,
) -> Result<String> {
    // Project resolution: prefer the configured `JIRA_PROJECT_KEYS`, but that's
    // only set for token setups — an OAuth-connected Jira leaves `project_keys`
    // empty (see intelligence/oauth/jira.rs). So fall back to inferring the
    // project from an existing Jira task_key (`KAN-171` → `KAN`; Jira project
    // keys never contain a hyphen), the same way GitHub derives owner/repo from
    // a sample key. This is what makes "propose a new ticket" work for the common
    // OAuth user, who otherwise has no project key anywhere.
    let project: String = match jira.project_keys.first() {
        Some(p) => p.clone(),
        None => {
            let sample = sample_key.context(
                "Jira create needs a project key - none is configured and there is no \
                 existing Jira ticket to infer one from",
            )?;
            sample
                .rsplit_once('-')
                .map(|(proj, _num)| proj)
                .filter(|proj| !proj.is_empty())
                .map(str::to_string)
                .with_context(|| format!("could not infer a Jira project key from '{sample}'"))?
        }
    };
    let project = project.as_str();
    let ctx = jira_resolve(jira).await.context("resolving Jira auth")?;
    // Bound the up-to-3 sequential calls (type check, self-assign, create) so a
    // slow/unresponsive tracker can't hang the approved-proposal sweep.
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .context("building Jira create HTTP client")?;

    let wanted = norm_issue_type(issue_type);
    let has_bug = wanted != "Bug" || jira_has_issue_type(&ctx, &client, project, "Bug").await?;
    let (type_name, title) = bug_fallback(wanted, has_bug, title);

    let mut payload = json!({
        "fields": {
            "project": { "key": project },
            "summary": title,
            "issuetype": { "name": type_name },
            "description": {
                "type": "doc", "version": 1,
                "content": [ { "type": "paragraph",
                    "content": [ { "type": "text", "text": description } ] } ]
            }
        }
    });
    // Self-assign so the ticket is discoverable by the `assignee = currentUser()`
    // sync query (see intelligence/providers/jira/fetch.rs) — an unassigned
    // ticket never enters pm_tasks, so its title never appears on the timeline.
    // Best-effort: a lookup failure shouldn't block ticket creation.
    match jira_ticket_update::my_account_id(&ctx, &client).await {
        Ok(account_id) => payload["fields"]["assignee"] = json!({ "accountId": account_id }),
        Err(e) => tracing::warn!(
            error = %e,
            "Jira create: couldn't resolve self accountId — creating unassigned"
        ),
    }
    let url = ctx.api_url("/rest/api/3/issue");
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

/// True if `name` is one of the project's configured issue types. Used to
/// avoid a hard 400 (`Specify a valid issue type`) when a project's scheme
/// doesn't include "Bug" (common on team-managed projects) — the caller
/// falls back to creating a "Task" prefixed with "Bug: " instead.
async fn jira_has_issue_type(
    ctx: &meridian_oauth::jira::JiraReqCtx,
    client: &reqwest::Client,
    project: &str,
    name: &str,
) -> Result<bool> {
    let url = ctx.api_url(&format!("/rest/api/3/project/{project}?fields=issueTypes"));
    let resp = ctx
        .apply(client.get(&url))
        .header("Accept", "application/json")
        .send()
        .await
        .with_context(|| format!("network error checking Jira issue types at {url}"))?;
    let body = resp
        .text()
        .await
        .context("reading Jira project issue-types response")?;
    let v: Value = serde_json::from_str(&body).context("parsing Jira project issue types")?;
    Ok(v.get("issueTypes")
        .and_then(|t| t.as_array())
        .is_some_and(|types| {
            types
                .iter()
                .any(|t| t.get("name").and_then(|n| n.as_str()) == Some(name))
        }))
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
    let auth = format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(format!(":{}", cfg.pat).as_bytes())
    );
    // Bound the up-to-3 sequential calls so a slow/unresponsive tracker can't
    // hang the approved-proposal sweep.
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .context("building Azure DevOps create HTTP client")?;

    let wanted = norm_issue_type(issue_type);
    let has_bug = wanted != "Bug" || azure_has_work_item_type(cfg, &client, &auth, "Bug").await?;
    let (work_item_type, title) = bug_fallback(wanted, has_bug, title);

    // Azure work-item type goes in the URL (`$Task` / `$Bug`).
    let url = format!(
        "{}/{}/_apis/wit/workitems/${work_item_type}?api-version=7.0",
        cfg.api_base, cfg.project,
    );
    let mut patch = vec![
        json!({ "op": "add", "path": "/fields/System.Title", "value": title }),
        json!({ "op": "add", "path": "/fields/System.Description", "value": description }),
    ];
    // Self-assign so the work item is discoverable by the `AssignedTo = @me`
    // sync query (see intelligence/providers/azure_devops/fetch.rs) — an
    // unassigned item never enters pm_tasks, so its title never appears on
    // the timeline. Best-effort: a lookup failure shouldn't block creation.
    match azure_ticket_update::my_unique_name(&client, cfg).await {
        Ok(me) => {
            patch.push(json!({ "op": "add", "path": "/fields/System.AssignedTo", "value": me }))
        }
        Err(e) => tracing::warn!(
            error = %e,
            "Azure DevOps create: couldn't resolve self identity — creating unassigned"
        ),
    }
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

/// True if `name` is one of the project's work-item types. Azure's process
/// template (e.g. Basic) may not include "Bug" — the caller falls back to
/// creating a "Task" prefixed with "Bug: " instead of a hard 404.
async fn azure_has_work_item_type(
    cfg: &AzureDevOpsConfig,
    client: &reqwest::Client,
    auth: &str,
    name: &str,
) -> Result<bool> {
    let url = format!(
        "{}/{}/_apis/wit/workitemtypes?api-version=7.0",
        cfg.api_base, cfg.project
    );
    let resp = client
        .get(&url)
        .header("Authorization", auth)
        .send()
        .await
        .with_context(|| format!("network error checking Azure work-item types at {url}"))?;
    let body = resp
        .text()
        .await
        .context("reading Azure work-item-types response")?;
    let v: Value = serde_json::from_str(&body).context("parsing Azure work-item types")?;
    Ok(v.get("value")
        .and_then(|t| t.as_array())
        .is_some_and(|types| {
            types
                .iter()
                .any(|t| t.get("name").and_then(|n| n.as_str()) == Some(name))
        }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn norm_issue_type_maps_bug_case_insensitively() {
        assert_eq!(norm_issue_type("bug"), "Bug");
        assert_eq!(norm_issue_type("BUG"), "Bug");
        assert_eq!(norm_issue_type("Task"), "Task");
        assert_eq!(norm_issue_type("Story"), "Task");
    }

    #[test]
    fn bug_fallback_keeps_bug_when_project_has_it() {
        let (kind, title) = bug_fallback("Bug", true, "Fix the thing");
        assert_eq!(kind, "Bug");
        assert_eq!(title, "Fix the thing");
    }

    #[test]
    fn bug_fallback_demotes_to_task_and_prefixes_title_when_missing() {
        let (kind, title) = bug_fallback("Bug", false, "Fix the thing");
        assert_eq!(kind, "Task");
        assert_eq!(title, "Bug: Fix the thing");
    }

    #[test]
    fn bug_fallback_is_noop_for_task() {
        let (kind, title) = bug_fallback("Task", false, "Do the thing");
        assert_eq!(kind, "Task");
        assert_eq!(title, "Do the thing");
    }
}
