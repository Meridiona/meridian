//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Jira fetch path — the /search/jql call and start-date field discovery.
//!
//! Split from the connector root purely for file size; the response shapes the
//! rest of the connector consumes ([`JiraIssue`] etc.) stay in [`super`] so the
//! upsert path owns them.
//!
//! # Who calls this
//! [`super::refresh_if_stale`], via `fetch::fetch` / `fetch::discover_start_date_field`.

use anyhow::{Context, Result};
use serde::Deserialize;

use super::JiraIssue;
use crate::intelligence::oauth::jira::JiraReqCtx;

pub(super) const MAX_RESULTS: usize = 100;

#[derive(Deserialize)]
struct JiraSearchResponse {
    issues: Vec<JiraIssue>,
}

// ---------------------------------------------------------------------------
// Field discovery
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct JiraFieldMeta {
    id: String,
    name: String,
}

/// Call /rest/api/3/field and return the ID of the field whose name best
/// matches "start date":
///   1. exact case-insensitive match on "start date"
///   2. name contains both "start" and "date" (case-insensitive)
///
/// Returns None if no match or if the request fails.
pub(super) async fn discover_start_date_field(ctx: &JiraReqCtx) -> Option<String> {
    let client = crate::intelligence::providers::http::client();
    let url = ctx.api_url("/rest/api/3/field");
    let resp = ctx.apply(client.get(&url)).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let fields: Vec<JiraFieldMeta> = resp.json().await.ok()?;

    // Priority 1: exact match
    if let Some(f) = fields
        .iter()
        .find(|f| f.name.eq_ignore_ascii_case("start date"))
    {
        return Some(f.id.clone());
    }
    // Priority 2: name contains both "start" and "date"
    fields
        .iter()
        .find(|f| {
            let n = f.name.to_lowercase();
            n.contains("start") && n.contains("date")
        })
        .map(|f| f.id.clone())
}

// ---------------------------------------------------------------------------
// Fetch
// ---------------------------------------------------------------------------

/// The active-task scope: everything assigned to you that isn't Done, except
/// Epics.
///
/// # Why this is a denylist, not an allowlist
///
/// This JQL is the ONLY thing that puts a Jira ticket into `pm_tasks`, which
/// makes an omission here invisible rather than noisy. A type left off the list
/// is absent from the Tasks page, absent from the worklog matcher's candidates
/// (so hours spent on it can only ever come back "no match"), and — worst — a
/// ticket of that type filed through the plan composer gets mirrored as a
/// `'local'` shadow row, because `plan_tasks::create` reads "not in pm_tasks
/// after a sync" as "self-assign failed". That shadow is meant to be temporary,
/// healed by the next sync's UPSERT; for an unfetchable type the heal can never
/// arrive, so a real Jira ticket sits in Meridian as a personal task forever.
///
/// That is not hypothetical — it is exactly what `Bug` being missing from the
/// old allowlist did. The list was then `(Task, Feature, Bug)`, which still
/// omitted **`Story`**, the default primary work type in every Jira Scrum
/// project. Any user on a standard Scrum board was therefore syncing almost
/// nothing, silently. Fixing that by appending `Story` would have left the same
/// trap armed for the next custom type someone's board uses.
///
/// So the filter names only what must stay OUT. Epics are excluded because they
/// are containers, not work: they reach Meridian as the `parent_key`/`epic_title`
/// of their children (see `mod.rs`'s upsert), never as rows of their own.
///
/// # Story is excluded by product decision, not by principle
///
/// `Story` is `hierarchyLevel: 0` — the same rung as Task and Bug, and on a
/// Scrum board it is the primary thing a person is assigned and works on. It is
/// excluded here because the product owner asked for it, NOT because it fails
/// any test the other included types pass.
///
/// The cost is real and worth restating before anyone "simplifies" this line
/// away or reinstates it casually: on a board that uses Stories as its main work
/// type, this leaves Meridian syncing only Tasks and Bugs, which for many teams
/// is close to an empty board. If users report "my tickets do not show up", this
/// clause is the first thing to check.
///
/// It is excluded SERVER-side, in the JQL, rather than by a name check next to
/// [`is_work_item`]. That is deliberate: [`MAX_RESULTS`] is a hard 100-row
/// ceiling with no pagination, so a type filtered client-side still consumes a
/// slot and is then thrown away, making truncation strictly worse. Filtered in
/// the JQL, Stories never occupy the budget at all.
///
/// Safe against a site that has no `Story` type: Jira does not error on an
/// unrecognised type name in a `!=` clause (verified against a live Cloud site),
/// unlike an `IN` allowlist.
///
/// # Sub-tasks are IN
///
/// Reversing the earlier "tasks/features and their epics, never subtasks" rule:
/// on many boards the subtask IS the unit of work someone spends a day on, and
/// excluding it meant that day could not be logged against anything. `type !=
/// Epic` admits them without naming them, which matters because the type's name
/// is site-specific — Jira ships both `Subtask` and `Sub-task` as defaults, and
/// custom subtask types are common. (`type IN subtaskIssueTypes()` is the
/// function that resolves them by NAME-independent category if this ever needs
/// to select subtasks specifically.)
///
/// One consequence to know: for a subtask, Jira's `parent` is its Story/Task,
/// not its Epic, so `epic_title` holds the parent task's summary. That is
/// harmless where the value is consumed — `task_triage` uses it only as a
/// "context anchor" presence check, and a subtask always has a parent, so it
/// never trips `NoContextAnchor`.
///
/// # Consistency
///
/// Jira was the only provider that filtered by type at all — Linear, GitHub and
/// Trello fetch everything assigned to you, and Azure's WIQL is `AssignedTo =
/// @me` alone. This brings Jira in line with them.
const ACTIVE_TASK_JQL: &str = "assignee = currentUser() AND statusCategory != Done \
     AND type != Epic AND type != Story ORDER BY updated DESC";

/// One active-task fetch: the work items to upsert, plus how many rows the
/// server actually returned before [`is_work_item`] filtered any out.
///
/// These two counts must not be conflated, and that is the entire reason this
/// type exists rather than a bare `Vec`. The caller uses the count to decide
/// whether the page may have been TRUNCATED at [`MAX_RESULTS`], and it only
/// prunes `pm_tasks` when it was not — pruning against a partial list deletes
/// live tickets. Using the post-filter length there would make a full page look
/// partial the moment a single container row was dropped, silently turning the
/// truncation guard off on exactly the boards this filter was added for.
pub(super) struct ActiveFetch {
    pub(super) issues: Vec<(JiraIssue, serde_json::Value)>,
    pub(super) returned_by_server: usize,
}

/// The rung at and above which a Jira issue type is a CONTAINER rather than
/// work: `1` is Epic, `2`+ are the Premium/Advanced-Roadmaps tiers (Initiative,
/// Capability, and custom ones an org invents).
const CONTAINER_HIERARCHY_LEVEL: i64 = 1;

/// Is this issue something a person does work on, as opposed to a bucket that
/// holds such things?
///
/// Filtering on the LEVEL rather than the NAME is the whole point. [`ACTIVE_TASK_JQL`]
/// can only say `type != Epic`, because JQL has no portable way to express
/// hierarchy — and a name is not a reliable proxy for a rung. "Feature" is
/// `hierarchyLevel: 0` (ordinary work) on some boards and a container tier above
/// Story on others; "Initiative" and "Capability" are containers that the JQL
/// clause never mentions at all. Without this check those arrive as work items,
/// which is precisely the category error excluding Epic exists to prevent.
///
/// **An unknown level counts as work.** Older Jira Server/Data Center responses
/// omit `hierarchyLevel` entirely, and the two failure modes are not symmetric:
/// wrongly including a container puts one extra row on the board, which is
/// visible and correctable, while wrongly excluding real work makes it
/// unloggable and silent — the same asymmetry that made the old `type IN (...)`
/// allowlist so expensive. So `None` is admitted.
fn is_work_item(issue: &JiraIssue) -> bool {
    issue
        .fields
        .issuetype
        .hierarchy_level
        .is_none_or(|level| level < CONTAINER_HIERARCHY_LEVEL)
}

#[tracing::instrument(
    skip(ctx),
    fields(
        provider = "jira",
        latency_ms = tracing::field::Empty,
        status_code = tracing::field::Empty,
    )
)]
pub(super) async fn fetch(ctx: &JiraReqCtx, start_date_field: Option<&str>) -> Result<ActiveFetch> {
    let all = search(ctx, start_date_field, ACTIVE_TASK_JQL).await?;
    let returned_by_server = all.len();
    let issues: Vec<_> = all.into_iter().filter(|(i, _)| is_work_item(i)).collect();
    if issues.len() < returned_by_server {
        // Worth a line: this only fires on boards with container tiers the JQL's
        // `type != Epic` cannot name, so it is the signal that such a board exists.
        tracing::debug!(
            dropped = returned_by_server - issues.len(),
            kept = issues.len(),
            "jira: skipped container-tier issues above the work rung"
        );
    }
    Ok(ActiveFetch {
        issues,
        returned_by_server,
    })
}

/// Fetch specific issues by key regardless of assignee/status/type — used to
/// backfill a `pm_tasks` row for a ticket that has worklog history but was
/// never (or is no longer) covered by the active-task scope of [`fetch`] (e.g.
/// it's Done, or assigned to someone else). Without this, such a ticket's
/// title can never appear on the timeline once its `pm_tasks` row is missing,
/// since [`fetch`]'s JQL will never surface it again.
pub(super) async fn fetch_by_keys(
    ctx: &JiraReqCtx,
    start_date_field: Option<&str>,
    keys: &[String],
) -> Result<Vec<(JiraIssue, serde_json::Value)>> {
    if keys.is_empty() {
        return Ok(Vec::new());
    }
    let jql = key_in_jql(keys);
    search(ctx, start_date_field, &jql).await
}

/// Builds a `key IN ("A-1","A-2")` JQL clause. Strips embedded quotes from
/// each key defensively — a Jira key is always `[A-Z]+-[0-9]+`, so this never
/// fires in practice, but it keeps a malformed key from breaking the query.
fn key_in_jql(keys: &[String]) -> String {
    let list = keys
        .iter()
        .map(|k| format!("\"{}\"", k.replace('"', "")))
        .collect::<Vec<_>>()
        .join(",");
    format!("key IN ({list})")
}

async fn search(
    ctx: &JiraReqCtx,
    start_date_field: Option<&str>,
    jql: &str,
) -> Result<Vec<(JiraIssue, serde_json::Value)>> {
    let client = crate::intelligence::providers::http::client();
    let url = ctx.api_url("/rest/api/3/search/jql");

    let mut fields = vec![
        "summary",
        "description",
        "issuetype",
        "project",
        "updated",
        "parent",
        "status",
        "duedate",
        "assignee",
        "labels",
        "customfield_10020",
        // CDM (Stage 3b): fed to the canonical adapter for the new columns.
        "reporter",
        "priority",
        "resolutiondate",
    ];
    if let Some(id) = start_date_field {
        fields.push(id);
    }

    let body = serde_json::json!({
        "jql": jql,
        "maxResults": MAX_RESULTS,
        "fields": fields,
    });

    let start = std::time::Instant::now();
    let resp = ctx
        .apply(client.post(&url))
        .json(&body)
        .send()
        .await
        .context("POST /search/jql")?;

    let status = resp.status();
    let elapsed_ms = start.elapsed().as_millis() as i64;
    tracing::Span::current().record("status_code", status.as_u16() as i64);
    tracing::Span::current().record("latency_ms", elapsed_ms);

    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        // Typed rather than formatted: the STATUS decides whether the caller
        // raises a "Reconnect Jira" banner (401/403) or stays quiet and retries
        // (429/5xx). A `bail!` string throws that away — see
        // `crate::intelligence::providers::http::HttpStatusError`.
        return Err(crate::intelligence::providers::http::HttpStatusError::new(
            status.as_u16(),
            "Jira /search/jql",
            &text,
        )
        .into());
    }

    // Parse once as a Value so we can keep each raw issue object verbatim for
    // the canonical adapter (Stage 3b), then deserialise the typed view from
    // the same body. The `issues` array order is identical, so we zip them.
    let body_val: serde_json::Value = resp.json().await.context("deserialising Jira response")?;
    let raw_issues: Vec<serde_json::Value> = body_val
        .get("issues")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let data: JiraSearchResponse =
        serde_json::from_value(body_val).context("parsing Jira response")?;
    let issue_count = data.issues.len();
    let keys: Vec<&str> = data.issues.iter().map(|i| i.key.as_str()).collect();
    tracing::debug!(count = issue_count, ?keys, "parsed Jira response");
    Ok(data.issues.into_iter().zip(raw_issues).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_in_jql_quotes_and_joins_keys() {
        let keys = vec!["KAN-64".to_string(), "KAN-67".to_string()];
        assert_eq!(key_in_jql(&keys), r#"key IN ("KAN-64","KAN-67")"#);
    }

    #[test]
    fn key_in_jql_single_key() {
        let keys = vec!["KAN-1".to_string()];
        assert_eq!(key_in_jql(&keys), r#"key IN ("KAN-1")"#);
    }

    #[test]
    fn key_in_jql_strips_embedded_quotes() {
        let keys = vec!["KAN\"-1".to_string()];
        assert_eq!(key_in_jql(&keys), r#"key IN ("KAN-1")"#);
    }

    /// This JQL is the ONLY door a Jira ticket enters `pm_tasks` through, so a
    /// type it excludes is invisible everywhere downstream — Tasks page, worklog
    /// candidates, plan — with no error anywhere. That is why the filter must
    /// stay a DENYLIST: the allowlist form shipped without `Bug` (bugs could
    /// never be worklogged) and then without `Story` (the default work type on
    /// every Scrum board, so those users synced almost nothing).
    ///
    /// The regression this guards is someone "tightening" it back into a
    /// `type IN (...)` list, which reintroduces the same silent data loss for
    /// whichever type they forget.
    #[test]
    fn active_task_jql_admits_every_type_except_epic_and_story() {
        assert!(
            ACTIVE_TASK_JQL.contains("type != Epic"),
            "epics are containers, not work — they arrive as parent_key/epic_title"
        );
        assert!(
            !ACTIVE_TASK_JQL.contains("type IN"),
            "must not be an allowlist: a forgotten type is silent, product-wide data loss"
        );
    }

    /// Story is excluded by product decision, not because it fails any test the
    /// included types pass — it is `hierarchyLevel: 0`, the same rung as Task.
    ///
    /// Pinned as its own test so the exclusion is a deliberate, visible line
    /// rather than something that quietly disappears in a future edit. If this
    /// test is ever deleted, deleting it should be the point of the change.
    ///
    /// Excluded in the JQL rather than client-side on purpose: `MAX_RESULTS` is
    /// a hard 100-row ceiling with no pagination, so a type dropped after the
    /// fetch still burns a slot.
    #[test]
    fn story_is_excluded_server_side() {
        assert!(ACTIVE_TASK_JQL.contains("type != Story"));
    }

    /// Builds the minimal `JiraIssue` shape `is_work_item` reads. Deserialised
    /// from JSON rather than constructed, so the test exercises the same
    /// `#[serde(rename = "hierarchyLevel")]` path a live response takes — a
    /// struct literal would pass even if the rename were wrong.
    fn issue_at_level(level: Option<i64>) -> JiraIssue {
        let hierarchy = match level {
            Some(l) => format!(r#","hierarchyLevel":{l}"#),
            None => String::new(),
        };
        serde_json::from_str(&format!(
            r#"{{"key":"K-1","fields":{{
                 "summary":"s","status":{{"name":"To Do","statusCategory":{{"key":"new"}}}},
                 "issuetype":{{"name":"X"{hierarchy}}},"project":{{"key":"K"}},
                 "updated":"2026-01-01T00:00:00.000+0000"}}}}"#
        ))
        .expect("fixture must match the real JiraIssue shape")
    }

    /// The rungs a person actually works on. Sub-tasks (-1) matter most here:
    /// they were excluded outright until this change.
    #[test]
    fn work_rungs_are_kept() {
        assert!(is_work_item(&issue_at_level(Some(-1))), "sub-task");
        assert!(
            is_work_item(&issue_at_level(Some(0))),
            "task/story/bug/feature"
        );
    }

    /// Epic (1) and the Premium tiers above it (Initiative, Capability, custom)
    /// are buckets, not work. Level 2 is the case the JQL cannot express at all:
    /// `type != Epic` never names those types, so without this they would arrive
    /// as work items.
    #[test]
    fn container_rungs_are_dropped() {
        assert!(!is_work_item(&issue_at_level(Some(1))), "epic");
        assert!(
            !is_work_item(&issue_at_level(Some(2))),
            "initiative/capability"
        );
        assert!(!is_work_item(&issue_at_level(Some(3))));
    }

    /// Jira Server/DC omits the field. Admitting the unknown is the deliberate
    /// choice: one stray container row is visible and correctable, whereas
    /// dropping real work makes it unloggable with no error anywhere — the exact
    /// failure the old allowlist kept producing.
    #[test]
    fn an_unknown_rung_is_treated_as_work() {
        assert!(is_work_item(&issue_at_level(None)));
    }

    /// Sub-tasks are deliberately IN scope, reversing the older
    /// "tasks/features and their epics, never subtasks" rule: on many boards the
    /// subtask is the unit of work someone actually spends a day on.
    ///
    /// Asserted via the absence of an exclusion rather than a name, because the
    /// type's name is site-specific — Jira ships both `Subtask` and `Sub-task`
    /// as defaults, and custom subtask types are common.
    #[test]
    fn active_task_jql_does_not_exclude_subtasks() {
        let jql = ACTIVE_TASK_JQL.to_lowercase();
        assert!(
            !jql.contains("subtask"),
            "no subtask exclusion may creep back in"
        );
        assert!(!jql.contains("sub-task"));
        assert!(!jql.contains("standardissuetypes"));
    }

    /// The scope is "mine, and not finished". Both halves matter: without the
    /// assignee clause we would mirror the whole project; without the status one
    /// every closed ticket would crowd the matcher's candidate list.
    #[test]
    fn active_task_jql_is_scoped_to_my_unfinished_work() {
        assert!(ACTIVE_TASK_JQL.contains("assignee = currentUser()"));
        assert!(ACTIVE_TASK_JQL.contains("statusCategory != Done"));
    }
}
