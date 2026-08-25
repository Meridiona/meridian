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

use super::active_filter::{
    is_excluded_type_name, is_unknown_issue_type_error, is_work_item, ACTIVE_TASK_JQL,
    PORTABLE_TASK_JQL,
};
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

#[tracing::instrument(
    skip(ctx),
    fields(
        provider = "jira",
        latency_ms = tracing::field::Empty,
        status_code = tracing::field::Empty,
    )
)]
pub(super) async fn fetch(ctx: &JiraReqCtx, start_date_field: Option<&str>) -> Result<ActiveFetch> {
    // The exclusions in `ACTIVE_TASK_JQL` name issue types, and Jira 400s the
    // WHOLE query when a site does not have one of them - so a board without
    // `Story` or `Feature` lost its entire refresh, not just the exclusion.
    // Fall back to a portable query and enforce the same policy client-side.
    let all = match search(ctx, start_date_field, ACTIVE_TASK_JQL).await {
        Ok(rows) => rows,
        Err(e) if is_unknown_issue_type_error(&e) => {
            tracing::warn!(
                provider = "jira",
                error = %crate::errors::chain(&e),
                "jira: this site does not have every issue type the task query names - \
                 retrying without the type exclusions and filtering them locally"
            );
            search(ctx, start_date_field, PORTABLE_TASK_JQL).await?
        }
        Err(e) => return Err(e),
    };
    let returned_by_server = all.len();
    // Both filters, on BOTH paths. `is_work_item` is the structural one
    // (containers by hierarchy level); `is_excluded_type_name` carries the name
    // policy that the JQL applies on the primary path and cannot on the
    // portable fallback.
    let issues: Vec<_> = all
        .into_iter()
        .filter(|(i, _)| is_work_item(i) && !is_excluded_type_name(i))
        .collect();
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

    /// A site without `Story` or `Feature` must still refresh.
    ///
    /// `ACTIVE_TASK_JQL` names those types, and Jira rejects the WHOLE query
    /// with 400 when an installation does not have one - so the exclusion took
    /// the entire active-task fetch down with it, on exactly the boards that
    /// never created them.
    #[test]
    fn an_absent_issue_type_routes_to_the_portable_query() {
        let err: anyhow::Error = crate::intelligence::providers::http::HttpStatusError::new(
            400,
            "Jira /search/jql",
            r#"{"errorMessages":["The value 'Feature' does not exist for the field 'type'."],"errors":{}}"#,
        )
        .into();
        assert!(super::is_unknown_issue_type_error(&err));
    }

    /// Every OTHER 400 must still fail loudly. Falling back on a genuinely
    /// malformed query would silently widen the result set instead of surfacing
    /// the bug.
    #[test]
    fn other_failures_do_not_route_to_the_portable_query() {
        for (status, body) in [
            (400, r#"{"errorMessages":["Field 'nope' does not exist."]}"#),
            (401, r#"{"errorMessages":["Unauthorized"]}"#),
            (429, "rate limited"),
            (500, "boom"),
        ] {
            let err: anyhow::Error = crate::intelligence::providers::http::HttpStatusError::new(
                status,
                "Jira /search/jql",
                body,
            )
            .into();
            assert!(
                !super::is_unknown_issue_type_error(&err),
                "status {status} must not silently fall back"
            );
        }
        // A non-HTTP error must not either.
        assert!(!super::is_unknown_issue_type_error(&anyhow::anyhow!(
            "network unreachable"
        )));
    }

    /// The portable fallback drops the JQL exclusions, so the name policy has to
    /// hold client-side or it silently stops applying on those sites.
    #[test]
    fn the_name_policy_is_enforced_client_side_too() {
        for name in ["Story", "story", "Feature", "Epic"] {
            assert!(
                super::is_excluded_type_name(&issue_named(name)),
                "{name} must be excluded whichever query returned it"
            );
        }
        for name in ["Task", "Bug", "Sub-task", "Improvement"] {
            assert!(
                !super::is_excluded_type_name(&issue_named(name)),
                "{name} is real work and must survive"
            );
        }
    }

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
    fn epic_is_excluded_and_the_filter_stays_a_denylist() {
        assert!(
            ACTIVE_TASK_JQL.contains("type != Epic"),
            "epics are containers, not work — they arrive as parent_key/epic_title"
        );
        assert!(
            !ACTIVE_TASK_JQL.contains("type IN"),
            "must not be an allowlist: a forgotten type is silent, product-wide data loss"
        );
    }

    /// `Story` and `Feature` are excluded by product decision, not because they
    /// fail any test the included types pass — both are `hierarchyLevel: 0`, the
    /// same rung as Task, and [`is_work_item`] would admit both.
    ///
    /// Pinned as its own test so each exclusion stays a deliberate, visible line
    /// rather than something that quietly disappears in a later edit. If this
    /// test is ever deleted, deleting it should BE the point of the change.
    ///
    /// Excluded in the JQL rather than client-side on purpose: `MAX_RESULTS` is
    /// a hard 100-row ceiling with no pagination, so a type dropped after the
    /// fetch still burns a slot.
    #[test]
    fn story_and_feature_are_excluded_server_side() {
        assert!(ACTIVE_TASK_JQL.contains("type != Story"));
        assert!(ACTIVE_TASK_JQL.contains("type != Feature"));
    }

    /// The scope after every exclusion: Task, Bug, sub-tasks and custom types.
    ///
    /// Asserted as absences because the whole design is a denylist — naming what
    /// is IN would reintroduce the allowlist trap that hid `Bug` and then
    /// `Story`. Custom work types are covered precisely BECAUSE they are never
    /// named anywhere in this string.
    #[test]
    fn nothing_else_is_excluded() {
        for kept in ["Task", "Bug", "Sub-task", "Subtask"] {
            assert!(
                !ACTIVE_TASK_JQL.contains(&format!("type != {kept}")),
                "{kept} must stay in scope"
            );
        }
    }

    /// Builds the minimal `JiraIssue` shape `is_work_item` reads. Deserialised
    /// from JSON rather than constructed, so the test exercises the same
    /// `#[serde(rename = "hierarchyLevel")]` path a live response takes — a
    /// struct literal would pass even if the rename were wrong.
    /// Same fixture shape as [`issue_at_level`], varying the TYPE NAME instead
    /// of the hierarchy rung - the axis the portable-fallback policy filters on.
    fn issue_named(name: &str) -> JiraIssue {
        serde_json::from_str(&format!(
            r#"{{"key":"K-1","fields":{{
                 "summary":"s","status":{{"name":"To Do","statusCategory":{{"key":"new"}}}},
                 "issuetype":{{"name":"{name}"}},"project":{{"key":"K"}},
                 "updated":"2026-01-01T00:00:00.000+0000"}}}}"#
        ))
        .expect("fixture must match the real JiraIssue shape")
    }

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
