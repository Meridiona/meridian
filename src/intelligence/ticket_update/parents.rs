//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Parent picker for the "not linked to a parent" hygiene fix. "Parent" means the
// level ABOVE the child, which differs per tracker:
//   * Jira          Epic → Story/Task/Bug (peers) → Sub-task. A standard issue's
//                   parent is an Epic; a Sub-task's parent is a standard issue.
//   * Azure DevOps  Epic → Feature → User Story/Bug → Task. Parent is one level up.
//   * GitHub        native sub-issues — parent is another issue.
//   * Linear        sub-issues — parent is another issue (any issue can parent).
//   * Trello        no native hierarchy.
//
// We return the candidate parents (click one → set via the normal parent write),
// a human label for the parent level ("Epic" / "parent task" / …), and a deep
// link to CREATE a new parent in the tracker (creating a parent is a multi-field
// flow we don't own, so it redirects — as the user asked).
//
// Hierarchies verified June 2026 (Atlassian / Microsoft Learn docs).

use anyhow::{Context, Result};
use serde_json::Value;

use crate::config::{Config, PmProviderConfig};
use crate::intelligence::oauth::jira::{resolve, JiraReqCtx};

/// One selectable parent.
#[derive(Debug, Clone)]
pub struct ParentOption {
    pub key: String,
    pub title: String,
}

/// Candidate parents + the level label + a create-parent deep link.
#[derive(Debug, Clone)]
pub struct ParentsResult {
    pub parents: Vec<ParentOption>,
    /// Singular noun for the parent level, e.g. "Epic", "parent task".
    pub parent_label: String,
    /// Empty if the provider has no deep link we can build.
    pub create_url: String,
}

impl ParentsResult {
    pub fn to_json(&self) -> Value {
        serde_json::json!({
            "parents": self.parents.iter().map(|p| serde_json::json!({ "key": p.key, "title": p.title })).collect::<Vec<_>>(),
            "parent_label": self.parent_label,
            "create_url": self.create_url,
        })
    }

    fn empty(label: &str) -> Self {
        Self {
            parents: vec![],
            parent_label: label.to_string(),
            create_url: String::new(),
        }
    }
}

/// List valid parents for `task_key` + a create-parent deep link.
pub async fn list(config: &Config, provider: &str, task_key: &str) -> Result<ParentsResult> {
    let pcfg = config
        .pm_providers
        .iter()
        .find(|p| p.provider_name() == provider);
    let pcfg = match pcfg {
        Some(p) => p,
        None => anyhow::bail!("provider {provider:?} is not configured"),
    };

    match pcfg {
        PmProviderConfig::Jira(cfg) => {
            let ctx = resolve(cfg)
                .await
                .context("resolving Jira auth for parents")?;
            jira_parents(&ctx, task_key).await
        }
        PmProviderConfig::Linear(cfg) => linear::parents(cfg, task_key).await,
        PmProviderConfig::AzureDevOps(cfg) => azure::parents(cfg, task_key).await,
        PmProviderConfig::GitHub(cfg) => github::parents(cfg, task_key).await,
        // Trello has no native card hierarchy — nothing to pick.
        PmProviderConfig::Trello(_) => Ok(ParentsResult::empty("parent")),
    }
}

// ---------------------------------------------------------------------------
// Jira — level-aware (Epic for a standard issue; standard issue for a sub-task)
// ---------------------------------------------------------------------------

/// Project key from a Jira issue key: `KAN-109` → `KAN`.
fn project_key_of(task_key: &str) -> &str {
    task_key
        .rsplit_once('-')
        .map(|(p, _)| p)
        .unwrap_or(task_key)
}

async fn jira_parents(ctx: &JiraReqCtx, task_key: &str) -> Result<ParentsResult> {
    let client = reqwest::Client::new();
    let project = project_key_of(task_key).to_string();

    // Is the child a sub-task? Decides whether the parent is an Epic or a standard issue.
    let child_is_subtask = jira_is_subtask(ctx, &client, task_key)
        .await
        .unwrap_or(false);
    let target = if child_is_subtask {
        ParentTarget::Standard
    } else {
        ParentTarget::Epic
    };
    let label = match target {
        ParentTarget::Epic => "Epic",
        ParentTarget::Standard => "parent task",
    };

    let parents = jira_candidate_parents(ctx, &client, &project, target)
        .await
        .unwrap_or_default();
    let create_url = jira_create_url(ctx, &client, &project, target)
        .await
        .unwrap_or_default();

    Ok(ParentsResult {
        parents,
        parent_label: label.to_string(),
        create_url,
    })
}

#[derive(Clone, Copy, PartialEq)]
enum ParentTarget {
    Epic,
    Standard,
}

async fn jira_is_subtask(ctx: &JiraReqCtx, client: &reqwest::Client, key: &str) -> Result<bool> {
    let url = ctx.api_url(&format!("/rest/api/3/issue/{key}?fields=issuetype"));
    let resp = ctx
        .apply(client.get(&url))
        .header("Accept", "application/json")
        .send()
        .await?;
    let text = resp.text().await.unwrap_or_default();
    let v: Value = serde_json::from_str(&text).context("parsing issue type")?;
    Ok(v.pointer("/fields/issuetype/subtask")
        .and_then(|b| b.as_bool())
        .unwrap_or(false))
}

async fn jira_candidate_parents(
    ctx: &JiraReqCtx,
    client: &reqwest::Client,
    project: &str,
    target: ParentTarget,
) -> Result<Vec<ParentOption>> {
    let jql = match target {
        ParentTarget::Epic => format!(
            "project = \"{project}\" AND issuetype = Epic AND statusCategory != Done ORDER BY updated DESC"
        ),
        // Standard issues (one level above a sub-task): everything that isn't an
        // Epic; sub-tasks are filtered out client-side via the issuetype field.
        ParentTarget::Standard => format!(
            "project = \"{project}\" AND issuetype != Epic AND statusCategory != Done ORDER BY updated DESC"
        ),
    };
    let url = ctx.api_url("/rest/api/3/search/jql");
    let body =
        serde_json::json!({ "jql": jql, "maxResults": 100, "fields": ["summary", "issuetype"] });
    let resp = ctx
        .apply(client.post(&url))
        .header("Accept", "application/json")
        .json(&body)
        .send()
        .await
        .context("POST /search/jql for parents")?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        anyhow::bail!("Jira parent search → {status}: {text}");
    }
    let v: Value = serde_json::from_str(&text).context("parsing parent search")?;
    Ok(parse_jira_parents(&v, target))
}

fn parse_jira_parents(v: &Value, target: ParentTarget) -> Vec<ParentOption> {
    v.get("issues")
        .and_then(|i| i.as_array())
        .map(|arr| {
            arr.iter()
                .filter(|issue| {
                    // For the sub-task case, drop any sub-task from the candidates.
                    if target == ParentTarget::Standard {
                        issue
                            .pointer("/fields/issuetype/subtask")
                            .and_then(|b| b.as_bool())
                            != Some(true)
                    } else {
                        true
                    }
                })
                .filter_map(|issue| {
                    let key = issue.get("key")?.as_str()?.to_string();
                    let title = issue
                        .pointer("/fields/summary")
                        .and_then(|s| s.as_str())
                        .unwrap_or("")
                        .to_string();
                    Some(ParentOption { key, title })
                })
                .collect()
        })
        .unwrap_or_default()
}

async fn jira_create_url(
    ctx: &JiraReqCtx,
    client: &reqwest::Client,
    project: &str,
    target: ParentTarget,
) -> Result<String> {
    let url = ctx.api_url(&format!("/rest/api/3/project/{project}"));
    let resp = ctx
        .apply(client.get(&url))
        .header("Accept", "application/json")
        .send()
        .await?;
    let text = resp.text().await.unwrap_or_default();
    let v: Value = serde_json::from_str(&text).context("parsing project")?;
    let pid = v
        .get("id")
        .and_then(|i| i.as_str())
        .context("project missing id")?;
    let type_id = match target {
        ParentTarget::Epic => pick_type_id_named(&v, "Epic"),
        ParentTarget::Standard => pick_standard_type_id(&v),
    };
    let base = ctx.site_base();
    Ok(match type_id {
        Some(tid) => format!("{base}/secure/CreateIssue.jspa?pid={pid}&issuetype={tid}"),
        None => format!("{base}/secure/CreateIssue!default.jspa?pid={pid}"),
    })
}

fn issue_types(project: &Value) -> &[Value] {
    project
        .get("issueTypes")
        .and_then(|t| t.as_array())
        .map(|a| a.as_slice())
        .unwrap_or(&[])
}

fn pick_type_id_named(project: &Value, name: &str) -> Option<String> {
    issue_types(project)
        .iter()
        .find(|t| t.get("name").and_then(|n| n.as_str()) == Some(name))
        .and_then(|t| t.get("id").and_then(|i| i.as_str()).map(String::from))
}

/// A standard (non-Epic, non-sub-task) issue type id, preferring Task then Story.
fn pick_standard_type_id(project: &Value) -> Option<String> {
    let types = issue_types(project);
    let is_standard = |t: &&Value| {
        let name = t.get("name").and_then(|n| n.as_str()).unwrap_or("");
        let subtask = t.get("subtask").and_then(|b| b.as_bool()).unwrap_or(false);
        !subtask && name != "Epic"
    };
    for pref in ["Task", "Story"] {
        if let Some(t) = types
            .iter()
            .find(|t| t.get("name").and_then(|n| n.as_str()) == Some(pref))
        {
            return t.get("id").and_then(|i| i.as_str()).map(String::from);
        }
    }
    types
        .iter()
        .find(is_standard)
        .and_then(|t| t.get("id").and_then(|i| i.as_str()).map(String::from))
}

// ---------------------------------------------------------------------------
// Linear — any open issue in the same team can be a parent
// ---------------------------------------------------------------------------

mod linear {
    use super::*;
    use crate::config::LinearConfig;

    const URL: &str = "https://api.linear.app/graphql";

    pub async fn parents(cfg: &LinearConfig, task_key: &str) -> Result<ParentsResult> {
        let client = reqwest::Client::new();
        // Resolve the issue's team, then its open issues (excluding the child itself).
        let query = "query Parents($id: String!) { issue(id: $id) { id team { id issues(first: 100, filter: { state: { type: { neq: \"completed\" } } }) { nodes { id identifier title } } } } }";
        let payload = serde_json::json!({ "query": query, "variables": { "id": task_key } });
        let data = graphql(&client, cfg, &payload).await.unwrap_or(Value::Null);
        let self_id = data
            .pointer("/issue/id")
            .and_then(|s| s.as_str())
            .unwrap_or("");
        let parents = data
            .pointer("/issue/team/issues/nodes")
            .and_then(|n| n.as_array())
            .map(|arr| {
                arr.iter()
                    .filter(|n| n.get("id").and_then(|i| i.as_str()) != Some(self_id))
                    .filter_map(|n| {
                        Some(ParentOption {
                            key: n.get("identifier")?.as_str()?.to_string(),
                            title: n
                                .get("title")
                                .and_then(|t| t.as_str())
                                .unwrap_or("")
                                .to_string(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(ParentsResult {
            parents,
            parent_label: "parent issue".to_string(),
            create_url: String::new(),
        })
    }

    async fn graphql(
        client: &reqwest::Client,
        cfg: &LinearConfig,
        payload: &Value,
    ) -> Result<Value> {
        let resp = client
            .post(URL)
            .header("Authorization", &cfg.api_key)
            .header("Content-Type", "application/json")
            .json(payload)
            .send()
            .await
            .context("Linear GraphQL")?;
        let text = resp.text().await.unwrap_or_default();
        let v: Value = serde_json::from_str(&text).context("parsing Linear response")?;
        v.get("data")
            .cloned()
            .context("Linear response missing data")
    }
}

// ---------------------------------------------------------------------------
// Azure DevOps — parent is one level up (Task→Story→Feature→Epic)
// ---------------------------------------------------------------------------

mod azure {
    use super::*;
    use crate::config::AzureDevOpsConfig;
    use crate::pm_worklog::azure_devops::parse_task_key;
    use base64::Engine;

    pub async fn parents(cfg: &AzureDevOpsConfig, task_key: &str) -> Result<ParentsResult> {
        let item = parse_task_key(task_key)?;
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(20))
            .build()
            .context("building HTTP client")?;

        let child_type = work_item_type(&client, cfg, &item.project, item.id)
            .await
            .unwrap_or_default();
        let parent_type = parent_type_of(&child_type);
        let label = parent_type
            .map(|t| format!("parent {t}"))
            .unwrap_or_else(|| "parent work item".to_string());

        let parents = match parent_type {
            Some(pt) => wiql_candidates(&client, cfg, &item.project, pt)
                .await
                .unwrap_or_default(),
            None => vec![],
        };
        let base = cfg.api_base.trim_end_matches('/');
        let create_url = match parent_type {
            Some(pt) => format!(
                "{base}/{}/_workitems/create/{}",
                item.project,
                urlencode(pt)
            ),
            None => format!("{base}/{}/_workitems/create/", item.project),
        };
        Ok(ParentsResult {
            parents,
            parent_label: label,
            create_url,
        })
    }

    /// Azure hierarchy: Task → User Story → Feature → Epic → (top).
    pub(super) fn parent_type_of(child: &str) -> Option<&'static str> {
        match child {
            "Task" => Some("User Story"),
            "User Story" | "Bug" | "Product Backlog Item" => Some("Feature"),
            "Feature" => Some("Epic"),
            _ => None,
        }
    }

    async fn work_item_type(
        client: &reqwest::Client,
        cfg: &AzureDevOpsConfig,
        project: &str,
        id: u64,
    ) -> Result<String> {
        let base = cfg.api_base.trim_end_matches('/');
        let url = format!(
            "{base}/{project}/_apis/wit/workitems/{id}?fields=System.WorkItemType&api-version=7.1"
        );
        let resp = client
            .get(&url)
            .header("Authorization", basic(cfg))
            .send()
            .await?;
        let text = resp.text().await.unwrap_or_default();
        let v: Value = serde_json::from_str(&text).context("parsing work item type")?;
        Ok(v.pointer("/fields/System.WorkItemType")
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .to_string())
    }

    async fn wiql_candidates(
        client: &reqwest::Client,
        cfg: &AzureDevOpsConfig,
        project: &str,
        parent_type: &str,
    ) -> Result<Vec<ParentOption>> {
        let base = cfg.api_base.trim_end_matches('/');
        // WIQL returns ids only; fetch titles in a follow-up batch GET.
        let wiql = format!(
            "SELECT [System.Id] FROM workitems WHERE [System.TeamProject] = @project AND \
             [System.WorkItemType] = '{parent_type}' AND [System.State] <> 'Closed' AND \
             [System.State] <> 'Done' ORDER BY [System.ChangedDate] DESC"
        );
        let url = format!("{base}/{project}/_apis/wit/wiql?api-version=7.1");
        let resp = client
            .post(&url)
            .header("Authorization", basic(cfg))
            .header("Content-Type", "application/json")
            .json(&serde_json::json!({ "query": wiql }))
            .send()
            .await
            .context("POST wiql")?;
        let text = resp.text().await.unwrap_or_default();
        let v: Value = serde_json::from_str(&text).context("parsing wiql")?;
        let ids: Vec<u64> = v
            .get("workItems")
            .and_then(|w| w.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|w| w.get("id").and_then(|i| i.as_u64()))
                    .take(50)
                    .collect()
            })
            .unwrap_or_default();
        if ids.is_empty() {
            return Ok(vec![]);
        }
        let id_csv = ids
            .iter()
            .map(|i| i.to_string())
            .collect::<Vec<_>>()
            .join(",");
        let batch = format!(
            "{base}/_apis/wit/workitems?ids={id_csv}&fields=System.Id,System.Title&api-version=7.1"
        );
        let resp = client
            .get(&batch)
            .header("Authorization", basic(cfg))
            .send()
            .await?;
        let text = resp.text().await.unwrap_or_default();
        let v: Value = serde_json::from_str(&text).context("parsing work item batch")?;
        Ok(v.get("value")
            .and_then(|a| a.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|w| {
                        let id = w.get("id").and_then(|i| i.as_u64())?;
                        let title = w
                            .pointer("/fields/System.Title")
                            .and_then(|t| t.as_str())
                            .unwrap_or("")
                            .to_string();
                        Some(ParentOption {
                            key: format!("{project}#{id}"),
                            title,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default())
    }

    fn basic(cfg: &AzureDevOpsConfig) -> String {
        let raw = format!(":{}", cfg.pat);
        format!(
            "Basic {}",
            base64::engine::general_purpose::STANDARD.encode(raw.as_bytes())
        )
    }

    fn urlencode(s: &str) -> String {
        s.replace(' ', "%20")
    }
}

// ---------------------------------------------------------------------------
// GitHub — sub-issues: parent is another issue in the same repo
// ---------------------------------------------------------------------------

mod github {
    use super::*;
    use crate::config::GitHubConfig;
    use crate::pm_worklog::github::parse_task_key;

    pub async fn parents(cfg: &GitHubConfig, task_key: &str) -> Result<ParentsResult> {
        let issue = parse_task_key(task_key)?;
        let client = reqwest::Client::new();
        let url = format!(
            "https://api.github.com/repos/{}/{}/issues?state=open&per_page=100",
            issue.owner, issue.repo
        );
        let resp = gh(cfg, client.get(&url))
            .send()
            .await
            .context("GET repo issues")?;
        let text = resp.text().await.unwrap_or_default();
        let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        let parents = parse_issue_candidates(&v, issue.number, &issue.owner, &issue.repo);
        let create_url = format!(
            "https://github.com/{}/{}/issues/new",
            issue.owner, issue.repo
        );
        Ok(ParentsResult {
            parents,
            parent_label: "parent issue".to_string(),
            create_url,
        })
    }

    pub(super) fn parse_issue_candidates(
        v: &Value,
        self_num: u64,
        owner: &str,
        repo: &str,
    ) -> Vec<ParentOption> {
        v.as_array()
            .map(|arr| {
                arr.iter()
                    // The issues endpoint also returns PRs — drop anything with a
                    // pull_request member, and the issue itself.
                    .filter(|i| i.get("pull_request").is_none())
                    .filter(|i| i.get("number").and_then(|n| n.as_u64()) != Some(self_num))
                    .filter_map(|i| {
                        let num = i.get("number")?.as_u64()?;
                        let title = i
                            .get("title")
                            .and_then(|t| t.as_str())
                            .unwrap_or("")
                            .to_string();
                        Some(ParentOption {
                            key: format!("{owner}/{repo}#{num}"),
                            title,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    fn gh(cfg: &GitHubConfig, rb: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        rb.header("Authorization", format!("Bearer {}", cfg.token))
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .header("User-Agent", "meridian")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_github_issue_candidates() {
        let v = serde_json::json!([
            { "number": 5, "title": "Tracking issue" },
            { "number": 7, "title": "A PR", "pull_request": { "url": "x" } },
            { "number": 9, "title": "Self" },
        ]);
        let p = super::github::parse_issue_candidates(&v, 9, "acme", "api");
        // PR (7) and self (9) dropped; only #5 remains.
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].key, "acme/api#5");
        assert_eq!(p[0].title, "Tracking issue");
    }

    #[test]
    fn derives_project_key() {
        assert_eq!(project_key_of("KAN-109"), "KAN");
        assert_eq!(project_key_of("PROJ-1"), "PROJ");
    }

    #[test]
    fn parses_epic_candidates() {
        let v = serde_json::json!({
            "issues": [
                { "key": "KAN-5", "fields": { "summary": "Onboarding", "issuetype": { "subtask": false } } },
            ]
        });
        let p = parse_jira_parents(&v, ParentTarget::Epic);
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].key, "KAN-5");
    }

    #[test]
    fn standard_target_filters_out_subtasks() {
        let v = serde_json::json!({
            "issues": [
                { "key": "KAN-7", "fields": { "summary": "Story", "issuetype": { "subtask": false } } },
                { "key": "KAN-8", "fields": { "summary": "A subtask", "issuetype": { "subtask": true } } },
            ]
        });
        let p = parse_jira_parents(&v, ParentTarget::Standard);
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].key, "KAN-7");
    }

    #[test]
    fn picks_named_and_standard_types() {
        let project = serde_json::json!({
            "id": "10000",
            "issueTypes": [
                { "id": "1", "name": "Sub-task", "subtask": true },
                { "id": "2", "name": "Task", "subtask": false },
                { "id": "3", "name": "Epic", "subtask": false },
            ]
        });
        assert_eq!(pick_type_id_named(&project, "Epic"), Some("3".into()));
        assert_eq!(pick_standard_type_id(&project), Some("2".into()));
    }

    #[test]
    fn azure_parent_levels() {
        assert_eq!(azure::parent_type_of("Task"), Some("User Story"));
        assert_eq!(azure::parent_type_of("User Story"), Some("Feature"));
        assert_eq!(azure::parent_type_of("Feature"), Some("Epic"));
        assert_eq!(azure::parent_type_of("Epic"), None);
    }
}
