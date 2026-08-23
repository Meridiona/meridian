//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! GitHub ticket creation — the REST issue POST plus the two follow-ups that make
//! a created issue actually visible to Meridian.
//!
//! # Why this is not just one POST
//! The GitHub sync (`crate::intelligence::providers::github::fetch`) walks the
//! configured Projects v2 boards and keeps an item only when BOTH hold:
//!
//!   1. the issue is an item on one of the configured boards, and
//!   2. the viewer is one of its assignees.
//!
//! Creating a bare issue satisfies neither, so the row never entered `pm_tasks`
//! and `plan_tasks::create` fell through to its `'local'` shadow row every time —
//! with a note blaming a failed self-assign, which for GitHub was only half the
//! story. This module therefore self-assigns on create and calls
//! `addProjectV2ItemById` afterwards, mirroring what `jira_create` already does
//! with its `assignee` field.
//!
//! # Repo resolution
//! `GitHubConfig` carries Projects-v2 node ids, not a repo, so the owner/repo to
//! file in has to come from somewhere. Previously the ONLY source was an existing
//! GitHub task_key (`owner/repo#123`) — which a user with an empty board does not
//! have, and the create failed outright with "no GitHub task to infer owner/repo"
//! (issue #843). We now fall back to the boards' own linked repositories, but only
//! when the answer is unambiguous: filing an issue is an outward-facing side
//! effect that cannot be taken back, so zero or several candidates is a clear
//! error telling the user what to do, never an arbitrary pick.
//!
//! # Scope
//! `addProjectV2ItemById` is a WRITE and needs the read-write `project` scope;
//! `read:project` is not enough. [`meridian_oauth::github::REQUIRED_SCOPES`] asks
//! for it, but a token minted before that change carries only `read:project`, so
//! the mutation is best-effort: a scope failure leaves the (real, already created)
//! issue in place and logs an actionable warning rather than failing the create.
//!
//! # Who calls this
//! [`super::create::create_ticket`] for `provider = "github"` — reached from the
//! approved-proposal sweep and from `plan-task-create` (the day-plan composer).
//!
//! # Related
//! - [`super::github`] — the worklog/comment poster and `parse_task_key`.
//! - [`crate::intelligence::providers::github::fetch`] — the sync walk whose two
//!   filters this module exists to satisfy.

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};

use crate::config::GitHubConfig;

/// GitHub's GraphQL endpoint — Projects v2 has no REST surface.
const GRAPHQL_URL: &str = "https://api.github.com/graphql";
/// Ceiling for each of the up-to-4 sequential calls (viewer, project repos,
/// create, add-to-board) so a slow GitHub can't hang the approved-proposal sweep.
const HTTP_TIMEOUT_S: u64 = 20;

/// One configured Projects v2 board and the repositories linked to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct BoardRepos {
    /// The board's `PVT_…` node id, as configured in `GITHUB_PROJECT_IDS`.
    pub project_id: String,
    /// `owner/repo` slugs linked to the board, in GitHub's own order.
    pub repos: Vec<String>,
}

/// Where a new issue is filed and which board it joins.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CreateTarget {
    pub owner: String,
    pub repo: String,
    /// The board to add the created issue to. `None` when no board is configured,
    /// in which case the issue is created but never enters `pm_tasks`.
    pub project_id: Option<String>,
}

/// File an issue on GitHub, self-assigned and added to the user's project board,
/// and return its `owner/repo#number` task key.
///
/// `sample_key` is any existing GitHub task key; when present its repo wins, which
/// keeps a user with a populated board filing where they always have. Errors only
/// when the issue itself could not be created — the board add is best-effort (see
/// the module header on scope), because an issue that exists must not be reported
/// as a failure the caller might retry into a duplicate.
pub(super) async fn create(
    github: &GitHubConfig,
    title: &str,
    description: &str,
    sample_key: Option<&str>,
) -> Result<String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(HTTP_TIMEOUT_S))
        .build()
        .context("building GitHub create HTTP client")?;

    // The boards' linked repositories answer two questions: which repo to file in
    // (when no existing task names one) and which board the new issue belongs on.
    // Worth one GraphQL round trip on a rare, user-initiated action — but when a
    // sample key already settles the repo, a failed walk must not block the
    // create; fall back to the first configured board instead.
    let boards = match fetch_board_repos(github, &client).await {
        Ok(b) => b,
        Err(e) if sample_key.is_some() => {
            tracing::warn!(
                error = %e,
                "GitHub create: couldn't list the boards' repositories - \
                 falling back to the first configured board"
            );
            Vec::new()
        }
        Err(e) => {
            return Err(e).context("listing the repositories linked to your GitHub project board")
        }
    };
    let target = resolve_target(sample_key, github.project_ids.first(), &boards)?;

    // Self-assign so the sync's `assignees contains viewer` filter passes. Unlike
    // Jira's (which needs an accountId lookup) this is just the login, but the
    // same best-effort rule applies: a lookup failure must not block the create.
    let assignee = match fetch_viewer_login(github, &client).await {
        Ok(login) => Some(login),
        Err(e) => {
            tracing::warn!(
                error = %e,
                "GitHub create: couldn't resolve the viewer login - creating unassigned"
            );
            None
        }
    };

    let url = format!(
        "https://api.github.com/repos/{}/{}/issues",
        target.owner, target.repo
    );
    let resp = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", github.token))
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .header("User-Agent", "meridian")
        .json(&issue_payload(title, description, assignee.as_deref()))
        .send()
        .await
        .context("network error reaching GitHub")?;
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        bail!("GitHub create returned {status}: {body}");
    }
    let v: Value = serde_json::from_str(&body).context("parsing GitHub create response")?;
    let created = created_issue(&v)?;
    let task_key = format!("{}/{}#{}", target.owner, target.repo, created.number);

    tracing::info!(
        task_key,
        assigned = assignee.is_some(),
        "github create: issue filed"
    );

    match &target.project_id {
        Some(project_id) => {
            // Best-effort by design: the issue is already real. A failure here
            // means it won't sync back, which `plan_tasks::create`'s shadow row
            // and its note already cover.
            if let Err(e) = add_to_board(github, &client, project_id, &created.node_id).await {
                tracing::warn!(
                    error = %e,
                    task_key,
                    "github create: could not add the issue to the project board"
                );
            } else {
                tracing::info!(task_key, "github create: issue added to the project board");
            }
        }
        None => tracing::warn!(
            task_key,
            "github create: no project board configured - the issue won't sync back into Meridian"
        ),
    }

    Ok(task_key)
}

// ── Pure decisions (unit-tested; no network) ─────────────────────────────────

/// Decide which repo to file in and which board to join.
///
/// `sample_key` (an existing `owner/repo#123`) wins when present. Otherwise the
/// boards' linked repositories must name exactly ONE distinct repo — see the
/// module header on why an ambiguous answer is an error rather than a guess.
/// `first_project` is the first configured board, used when nothing better
/// identifies which board the target repo belongs to.
pub(super) fn resolve_target(
    sample_key: Option<&str>,
    first_project: Option<&String>,
    boards: &[BoardRepos],
) -> Result<CreateTarget> {
    if let Some(sample) = sample_key {
        let item = super::github::parse_task_key(sample).context("parsing GitHub sample key")?;
        return Ok(CreateTarget {
            project_id: board_for_repo(&format!("{}/{}", item.owner, item.repo), boards)
                .or_else(|| first_project.cloned()),
            owner: item.owner,
            repo: item.repo,
        });
    }

    let mut distinct: Vec<&str> = Vec::new();
    for board in boards {
        for repo in &board.repos {
            if !distinct.iter().any(|r| r.eq_ignore_ascii_case(repo)) {
                distinct.push(repo);
            }
        }
    }
    match distinct.as_slice() {
        [] => bail!(
            "GitHub create needs a repository to file the issue in. Your project board has no \
             linked repository - link one on GitHub (project menu > Settings > Manage access / \
             linked repositories), or create the issue on GitHub once so Meridian can learn it."
        ),
        [only] => {
            let (owner, repo) = only
                .split_once('/')
                .with_context(|| format!("GitHub returned a malformed repository slug {only:?}"))?;
            Ok(CreateTarget {
                owner: owner.to_string(),
                repo: repo.to_string(),
                project_id: board_for_repo(only, boards).or_else(|| first_project.cloned()),
            })
        }
        many => bail!(
            "GitHub create can't tell which repository to use - your project board links {} of \
             them ({}). Create the issue on GitHub once and Meridian will file the next one \
             alongside it.",
            many.len(),
            many.join(", ")
        ),
    }
}

/// The first board that links `repo`, so the created issue joins the board it
/// actually belongs to rather than whichever was configured first.
fn board_for_repo(repo: &str, boards: &[BoardRepos]) -> Option<String> {
    boards
        .iter()
        .find(|b| b.repos.iter().any(|r| r.eq_ignore_ascii_case(repo)))
        .map(|b| b.project_id.clone())
}

/// The REST create body. `assignees` is what makes the created issue pass the
/// sync's viewer filter — omitted entirely (not sent empty) when the login could
/// not be resolved, so GitHub applies its own default rather than an empty list.
fn issue_payload(title: &str, description: &str, assignee: Option<&str>) -> Value {
    let mut body = json!({ "title": title, "body": description });
    if let Some(login) = assignee {
        body["assignees"] = json!([login]);
    }
    body
}

/// The identifiers a created issue is known by: `number` for the human task key,
/// `node_id` for the GraphQL mutation that puts it on the board.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CreatedIssue {
    pub number: i64,
    pub node_id: String,
}

/// Read both identifiers out of the REST create response.
pub(super) fn created_issue(v: &Value) -> Result<CreatedIssue> {
    let number = v
        .get("number")
        .and_then(Value::as_i64)
        .context("GitHub create response missing `number`")?;
    let node_id = v
        .get("node_id")
        .and_then(Value::as_str)
        .context("GitHub create response missing `node_id`")?
        .to_string();
    Ok(CreatedIssue { number, node_id })
}

/// Flatten a `node(id:…){ …ProjectV2 { repositories } }` batch into one
/// [`BoardRepos`] per board that answered. A board GitHub couldn't resolve (a
/// stale id, an org whose SSO the token isn't authorised for) is skipped rather
/// than failing the whole walk — GraphQL reports those as a partial `errors`
/// entry alongside a usable `data`.
pub(super) fn board_repos_from_response(v: &Value, project_ids: &[String]) -> Vec<BoardRepos> {
    project_ids
        .iter()
        .enumerate()
        .filter_map(|(i, project_id)| {
            let node = v.pointer(&format!("/data/p{i}"))?;
            let repos = node
                .pointer("/repositories/nodes")?
                .as_array()?
                .iter()
                .filter_map(|r| r.get("nameWithOwner").and_then(Value::as_str))
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect();
            Some(BoardRepos {
                project_id: project_id.clone(),
                repos,
            })
        })
        .collect()
}

/// One aliased sub-query per configured board, so all of them resolve in a single
/// round trip. Aliases are positional (`p0`, `p1`, …) and
/// [`board_repos_from_response`] reads them back by the same index.
fn board_repos_query(project_ids: &[String]) -> String {
    let selections: Vec<String> = project_ids
        .iter()
        .enumerate()
        .map(|(i, id)| {
            format!(
                "p{i}: node(id: {}) {{ ... on ProjectV2 {{ \
                 repositories(first: 20) {{ nodes {{ nameWithOwner }} }} }} }}",
                json!(id)
            )
        })
        .collect();
    format!("query {{ {} }}", selections.join(" "))
}

/// The `addProjectV2ItemById` mutation body — the call that actually puts the
/// created issue on the user's board.
fn add_to_board_payload(project_id: &str, content_id: &str) -> Value {
    json!({
        "query": "mutation($projectId:ID!,$contentId:ID!){\
            addProjectV2ItemById(input:{projectId:$projectId,contentId:$contentId})\
            { item { id } } }",
        "variables": { "projectId": project_id, "contentId": content_id }
    })
}

/// A GraphQL-level failure. GitHub answers one with HTTP **200** plus an `errors`
/// array, so a status check alone reports success on a mutation that did nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct GqlError {
    /// GitHub's machine-readable classifier (`errors[].type`), when present.
    pub kind: Option<String>,
    pub message: String,
}

/// The GraphQL error worth reporting out of a response, if any.
///
/// A scope failure wins over any other entry. A batched board query can come
/// back with several errors, and taking `first()` blindly would hide the one
/// whose advice is actionable ("reconnect GitHub") behind, say, a `NOT_FOUND`
/// for an unrelated stale board id.
pub(super) fn graphql_error(v: &Value) -> Option<GqlError> {
    let errors = v.get("errors")?.as_array()?;
    let read = |e: &Value| GqlError {
        kind: e.get("type").and_then(Value::as_str).map(str::to_string),
        message: e
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("unknown GitHub GraphQL error")
            .to_string(),
    };
    errors
        .iter()
        .map(read)
        .find(is_missing_project_scope)
        .or_else(|| errors.first().map(read))
}

/// True when the response carries a `data` payload worth reading despite any
/// accompanying `errors`.
///
/// GitHub reports a board it cannot resolve — a stale `PVT_…` id, or an org
/// whose SAML SSO this token is not authorised for — as an `errors` entry
/// **beside a usable `data`**, HTTP 200. Confirmed live: a two-board query with
/// one bad id returns `{"data":{"p0":{…},"p1":null},"errors":[{"type":"NOT_FOUND",…}]}`.
/// Treating that as fatal is what would make ONE stale board id block every
/// create — the exact class of failure issue #843 was about. The same rule is
/// already applied by `discover_github_projects` in the tray.
pub(super) fn has_usable_data(v: &Value) -> bool {
    v.get("data").is_some_and(|d| !d.is_null())
}

/// True when the token lacks the read-write `project` scope. Worth singling out:
/// the fix is for the user to reconnect GitHub, and no generic "GraphQL error"
/// would tell them that.
///
/// Keyed on GitHub's own `type: "INSUFFICIENT_SCOPES"` classifier (confirmed live
/// against the API — see `REAL_INSUFFICIENT_SCOPES_ERROR` in the tests) rather
/// than on words in the prose. Substring-matching the message is what makes a
/// check like this misfire: Projects v2 errors routinely mention both "project"
/// and "scope", and rewriting one of those into "reconnect GitHub" sends the user
/// to do something that cannot help. The message is consulted only as a fallback,
/// for a response that carries no `type` at all.
pub(super) fn is_missing_project_scope(err: &GqlError) -> bool {
    match &err.kind {
        Some(kind) => kind == "INSUFFICIENT_SCOPES",
        None => err
            .message
            .contains("has not been granted the required scopes"),
    }
}

// ── Network calls ────────────────────────────────────────────────────────────

/// POST a GraphQL `payload`, returning the parsed body. Surfaces a GraphQL-level
/// `errors` entry as a real error — see [`graphql_error`] on why HTTP status is
/// not enough.
async fn graphql(
    github: &GitHubConfig,
    client: &reqwest::Client,
    payload: &Value,
) -> Result<Value> {
    let resp = client
        .post(GRAPHQL_URL)
        .header("Authorization", format!("Bearer {}", github.token))
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", "meridian")
        .json(payload)
        .send()
        .await
        .context("network error reaching the GitHub GraphQL API")?;
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        bail!("GitHub GraphQL returned {status}: {body}");
    }
    let v: Value = serde_json::from_str(&body).context("parsing the GitHub GraphQL response")?;
    if let Some(err) = graphql_error(&v) {
        // Fatal only when nothing usable came back — see [`has_usable_data`].
        // A partial response is the normal way GitHub reports one unresolvable
        // board out of several, and `board_repos_from_response` already skips
        // whichever came back null.
        if !has_usable_data(&v) {
            if is_missing_project_scope(&err) {
                bail!(
                    "your GitHub connection is missing the project scope - reconnect GitHub in \
                     Settings > Integrations to grant it ({})",
                    err.message
                );
            }
            bail!("GitHub GraphQL error: {}", err.message);
        }
        tracing::warn!(
            error = %err.message,
            kind = err.kind.as_deref().unwrap_or("unknown"),
            "github create: partial GraphQL errors - continuing with the boards that resolved"
        );
    }
    Ok(v)
}

/// The authenticated user's login, for the create's `assignees`.
async fn fetch_viewer_login(github: &GitHubConfig, client: &reqwest::Client) -> Result<String> {
    let v = graphql(github, client, &json!({ "query": "{ viewer { login } }" })).await?;
    v.pointer("/data/viewer/login")
        .and_then(Value::as_str)
        .map(str::to_string)
        .context("GitHub viewer response missing `login`")
}

/// The repositories linked to each configured board.
async fn fetch_board_repos(
    github: &GitHubConfig,
    client: &reqwest::Client,
) -> Result<Vec<BoardRepos>> {
    if github.project_ids.is_empty() {
        return Ok(Vec::new());
    }
    let payload = json!({ "query": board_repos_query(&github.project_ids) });
    let v = graphql(github, client, &payload).await?;
    let boards = board_repos_from_response(&v, &github.project_ids);
    tracing::debug!(
        boards = boards.len(),
        repos = boards.iter().map(|b| b.repos.len()).sum::<usize>(),
        "github create: resolved the boards' linked repositories"
    );
    Ok(boards)
}

/// Put the created issue on the board.
async fn add_to_board(
    github: &GitHubConfig,
    client: &reqwest::Client,
    project_id: &str,
    content_id: &str,
) -> Result<()> {
    graphql(
        github,
        client,
        &add_to_board_payload(project_id, content_id),
    )
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn board(id: &str, repos: &[&str]) -> BoardRepos {
        BoardRepos {
            project_id: id.to_string(),
            repos: repos.iter().map(|s| s.to_string()).collect(),
        }
    }

    /// The regression from issue #843: a user whose board has no GitHub task yet
    /// hit a hard "no GitHub task to infer owner/repo" error. The board's single
    /// linked repository is the answer, and it must be used.
    #[test]
    fn resolves_the_repo_from_the_board_when_there_is_no_sample_key() {
        let boards = [board("PVT_1", &["acme/api"])];
        let t = resolve_target(None, Some(&"PVT_1".to_string()), &boards).unwrap();
        assert_eq!(t.owner, "acme");
        assert_eq!(t.repo, "api");
        assert_eq!(t.project_id.as_deref(), Some("PVT_1"));
    }

    /// An existing task key still wins - a populated board keeps filing where the
    /// user already files, regardless of what else the board links.
    #[test]
    fn a_sample_key_wins_over_the_boards_linked_repos() {
        let boards = [board("PVT_1", &["acme/other"])];
        let t = resolve_target(Some("acme/api#7"), Some(&"PVT_1".to_string()), &boards).unwrap();
        assert_eq!((t.owner.as_str(), t.repo.as_str()), ("acme", "api"));
    }

    /// Filing an issue can't be undone, so an ambiguous board is refused - and the
    /// message has to name the candidates and the way out.
    #[test]
    fn refuses_to_guess_when_the_board_links_several_repos() {
        let boards = [board("PVT_1", &["acme/api", "acme/web"])];
        let err = resolve_target(None, Some(&"PVT_1".to_string()), &boards)
            .unwrap_err()
            .to_string();
        assert!(err.contains("acme/api"), "{err}");
        assert!(err.contains("acme/web"), "{err}");
    }

    /// The same repo linked to two boards is ONE candidate, not an ambiguity.
    #[test]
    fn the_same_repo_on_two_boards_is_not_ambiguous() {
        let boards = [board("PVT_1", &["acme/api"]), board("PVT_2", &["Acme/API"])];
        let t = resolve_target(None, Some(&"PVT_1".to_string()), &boards).unwrap();
        assert_eq!(t.repo, "api");
    }

    /// No linked repo anywhere: the old opaque error is replaced by one that says
    /// what to do.
    #[test]
    fn explains_what_to_do_when_no_repo_can_be_found() {
        let err = resolve_target(None, None, &[]).unwrap_err().to_string();
        assert!(err.contains("link one on GitHub"), "{err}");
    }

    /// The issue joins the board that links its repo, not merely the first
    /// configured one.
    #[test]
    fn picks_the_board_that_actually_links_the_target_repo() {
        let boards = [board("PVT_1", &["acme/web"]), board("PVT_2", &["acme/api"])];
        let t = resolve_target(Some("acme/api#7"), Some(&"PVT_1".to_string()), &boards).unwrap();
        assert_eq!(t.project_id.as_deref(), Some("PVT_2"));
    }

    /// Nothing links the repo (a sample key from outside the board) - fall back to
    /// the first configured board rather than dropping the add entirely.
    #[test]
    fn falls_back_to_the_first_board_when_none_links_the_repo() {
        let boards = [board("PVT_1", &["acme/web"])];
        let t = resolve_target(Some("other/thing#1"), Some(&"PVT_1".to_string()), &boards).unwrap();
        assert_eq!(t.project_id.as_deref(), Some("PVT_1"));
    }

    /// Self-assignment is the OTHER half of the sync filter - without it a created
    /// issue sits on the board and still never enters `pm_tasks`.
    #[test]
    fn the_create_payload_self_assigns() {
        let p = issue_payload("Fix it", "details", Some("octocat"));
        assert_eq!(p["assignees"], json!(["octocat"]));
        assert_eq!(p["title"], "Fix it");
        assert_eq!(p["body"], "details");
    }

    /// An unresolved login must omit `assignees`, not send an empty array (which
    /// GitHub reads as an explicit "assign nobody").
    #[test]
    fn the_create_payload_omits_assignees_when_the_login_is_unknown() {
        let p = issue_payload("Fix it", "details", None);
        assert!(p.get("assignees").is_none());
    }

    /// Verbatim shape of a real REST issue-create response (trimmed): `node_id` is
    /// the GraphQL handle the board mutation needs, and it must be read from the
    /// same body as `number`.
    #[test]
    fn reads_number_and_node_id_from_a_real_create_response() {
        let body = json!({
            "number": 1347,
            "node_id": "I_kwDOABCD12MAAAAB",
            "title": "Found a bug",
            "state": "open",
        });
        assert_eq!(
            created_issue(&body).unwrap(),
            CreatedIssue {
                number: 1347,
                node_id: "I_kwDOABCD12MAAAAB".to_string(),
            }
        );
    }

    #[test]
    fn a_create_response_without_a_node_id_is_an_error() {
        assert!(created_issue(&json!({ "number": 1 })).is_err());
        assert!(created_issue(&json!({ "node_id": "x" })).is_err());
    }

    #[test]
    fn the_board_mutation_carries_both_ids() {
        let p = add_to_board_payload("PVT_1", "I_abc");
        assert!(p["query"]
            .as_str()
            .unwrap()
            .contains("addProjectV2ItemById"));
        assert_eq!(p["variables"]["projectId"], "PVT_1");
        assert_eq!(p["variables"]["contentId"], "I_abc");
    }

    /// Board ids are interpolated into the query text, so they must be JSON-quoted
    /// rather than pasted raw.
    #[test]
    fn the_board_query_quotes_every_id_and_aliases_them_positionally() {
        let q = board_repos_query(&["PVT_1".to_string(), "PVT_2".to_string()]);
        assert!(q.contains("p0: node(id: \"PVT_1\")"), "{q}");
        assert!(q.contains("p1: node(id: \"PVT_2\")"), "{q}");
        assert!(q.contains("nameWithOwner"), "{q}");
    }

    /// Verbatim body from running [`board_repos_query`]'s exact output against the
    /// live API with two real boards: `p0` is an org-owned project with one linked
    /// repository, `p1` a user-owned project with none. Both shapes are pinned
    /// here because the whole no-sample-key path is built on them and no amount of
    /// hand-authored JSON could have told us GitHub really answers this way.
    const REAL_BOARD_REPOS_RESPONSE: &str = r#"{"data":{
        "p0":{"repositories":{"nodes":[{"nameWithOwner":"Meridiona/meridian"}]}},
        "p1":{"repositories":{"nodes":[]}}}}"#;

    #[test]
    fn reads_linked_repos_back_by_alias_from_a_real_response() {
        let body: Value = serde_json::from_str(REAL_BOARD_REPOS_RESPONSE).unwrap();
        let ids = vec!["PVT_org".to_string(), "PVT_user".to_string()];
        assert_eq!(
            board_repos_from_response(&body, &ids),
            vec![
                board("PVT_org", &["Meridiona/meridian"]),
                board("PVT_user", &[]),
            ]
        );
    }

    /// The end-to-end shape of the reported bug, driven off that same real body: a
    /// board with one linked repo and a second with none resolves unambiguously.
    #[test]
    fn a_real_response_resolves_to_the_one_linked_repo() {
        let body: Value = serde_json::from_str(REAL_BOARD_REPOS_RESPONSE).unwrap();
        let ids = vec!["PVT_org".to_string(), "PVT_user".to_string()];
        let boards = board_repos_from_response(&body, &ids);
        let t = resolve_target(None, ids.first(), &boards).unwrap();
        assert_eq!(
            (t.owner.as_str(), t.repo.as_str()),
            ("Meridiona", "meridian")
        );
        assert_eq!(t.project_id.as_deref(), Some("PVT_org"));
    }

    /// An empty board contributes no candidate rather than a blank one - GitHub
    /// returns `"nodes": []`, not `null`, for a project with nothing linked.
    #[test]
    fn a_board_with_no_linked_repos_is_not_a_candidate() {
        let boards = [board("PVT_user", &[])];
        assert!(resolve_target(None, Some(&"PVT_user".to_string()), &boards).is_err());
    }

    /// A board GitHub couldn't resolve comes back as `null` beside a usable
    /// sibling. Skip it; don't let it collapse the whole walk.
    #[test]
    fn skips_a_board_that_did_not_resolve() {
        let body = json!({ "data": {
            "p0": Value::Null,
            "p1": { "repositories": { "nodes": [{ "nameWithOwner": "acme/web" }] } },
        }});
        let ids = vec!["PVT_1".to_string(), "PVT_2".to_string()];
        assert_eq!(
            board_repos_from_response(&body, &ids),
            vec![board("PVT_2", &["acme/web"])]
        );
    }

    /// GraphQL answers a failure with HTTP 200 + `errors`, so the body is the only
    /// place it is visible.
    #[test]
    fn surfaces_a_graphql_error_from_an_http_200_body() {
        let body = json!({ "data": Value::Null, "errors": [{ "message": "boom" }] });
        assert_eq!(
            graphql_error(&body),
            Some(GqlError {
                kind: None,
                message: "boom".to_string()
            })
        );
        assert_eq!(graphql_error(&json!({ "data": { "viewer": {} } })), None);
        assert_eq!(graphql_error(&json!({ "errors": [] })), None);
    }

    /// Verbatim response from calling `addProjectV2ItemById` live with a token
    /// holding only `read:project` — the exact failure every already-connected
    /// user's token will hit. It is the evidence for the scope change, and for
    /// keying the predicate on `type` rather than on the prose.
    const REAL_INSUFFICIENT_SCOPES_ERROR: &str = r#"{"errors":[{"type":"INSUFFICIENT_SCOPES",
        "message":"Your token has not been granted the required scopes to execute this query. The 'addProjectV2ItemById' field requires one of the following scopes: ['project'], but your token has only been granted the: ['gist', 'read:org', 'read:project', 'repo', 'workflow'] scopes. Please modify your token's scopes at: https://github.com/settings/tokens."}]}"#;

    /// Verbatim body from running the batched board query live with one good id
    /// and one stale one: HTTP 200, a USABLE `data` (`p0` resolved), `p1` null,
    /// and a `NOT_FOUND` in `errors`.
    ///
    /// This is the shape that made the previous code wrong. `graphql` bailed on
    /// any `errors` entry, so `fetch_board_repos` failed outright and - with no
    /// sample key - `create` propagated that. One stale board id made every
    /// GitHub task creation impossible, which is the failure #843 set out to fix.
    const REAL_PARTIAL_BOARD_RESPONSE: &str = r#"{"data":{
        "p0":{"repositories":{"nodes":[{"nameWithOwner":"Meridiona/meridian"}]}},
        "p1":null},
        "errors":[{"type":"NOT_FOUND","path":["p1"],
        "message":"Could not resolve to a node with the global id of 'PVT_stale'"}]}"#;

    #[test]
    fn a_partial_board_response_is_not_fatal() {
        let body: Value = serde_json::from_str(REAL_PARTIAL_BOARD_RESPONSE).unwrap();
        assert!(
            has_usable_data(&body),
            "one unresolvable board must not discard the boards that did resolve"
        );
        assert_eq!(
            graphql_error(&body).and_then(|e| e.kind).as_deref(),
            Some("NOT_FOUND")
        );
    }

    /// The end-to-end consequence: the good board still yields its repo, and the
    /// stale one is skipped rather than blocking the create.
    #[test]
    fn a_stale_board_id_does_not_block_creating_the_task() {
        let body: Value = serde_json::from_str(REAL_PARTIAL_BOARD_RESPONSE).unwrap();
        let ids = vec!["PVT_good".to_string(), "PVT_stale".to_string()];
        let boards = board_repos_from_response(&body, &ids);
        assert_eq!(boards, vec![board("PVT_good", &["Meridiona/meridian"])]);
        let t = resolve_target(None, ids.first(), &boards).unwrap();
        assert_eq!(
            (t.owner.as_str(), t.repo.as_str()),
            ("Meridiona", "meridian")
        );
    }

    /// A response with no usable `data` stays fatal - that is a real failure, not
    /// a board we can skip.
    #[test]
    fn a_response_with_no_usable_data_is_still_fatal() {
        for body in [
            json!({ "errors": [{ "message": "boom" }] }),
            json!({ "data": Value::Null, "errors": [{ "message": "boom" }] }),
        ] {
            assert!(!has_usable_data(&body));
        }
    }

    /// A mixed response must not hide the one error whose advice the user can
    /// act on behind an unrelated entry that happens to come first.
    #[test]
    fn a_scope_failure_wins_over_an_unrelated_first_error() {
        let body = json!({ "errors": [
            { "type": "NOT_FOUND", "message": "Could not resolve to a node" },
            { "type": "INSUFFICIENT_SCOPES", "message": "needs ['project']" },
        ]});
        let err = graphql_error(&body).unwrap();
        assert!(is_missing_project_scope(&err), "{err:?}");
    }

    #[test]
    fn recognises_a_real_missing_project_scope() {
        let body: Value = serde_json::from_str(REAL_INSUFFICIENT_SCOPES_ERROR).unwrap();
        let err = graphql_error(&body).unwrap();
        assert_eq!(err.kind.as_deref(), Some("INSUFFICIENT_SCOPES"));
        assert!(is_missing_project_scope(&err));
    }

    /// The misfire this predicate is written to avoid: a Projects v2 error that
    /// mentions BOTH "project" and "scope" but is nothing to do with the token.
    /// Telling that user to reconnect GitHub sends them to do something useless.
    #[test]
    fn an_unrelated_project_error_is_not_a_scope_failure() {
        let err = GqlError {
            kind: Some("NOT_FOUND".to_string()),
            message: "Could not resolve to a ProjectV2 with this id (scope: repository)"
                .to_string(),
        };
        assert!(!is_missing_project_scope(&err));
    }

    /// A response with no `type` at all falls back to the message, so an older or
    /// differently-shaped GraphQL error still produces the right advice.
    #[test]
    fn falls_back_to_the_message_when_there_is_no_type() {
        assert!(is_missing_project_scope(&GqlError {
            kind: None,
            message: "Your token has not been granted the required scopes to execute this query."
                .to_string(),
        }));
        assert!(!is_missing_project_scope(&GqlError {
            kind: None,
            message: "Could not resolve to a node".to_string(),
        }));
    }
}
