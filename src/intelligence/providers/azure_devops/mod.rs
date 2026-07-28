//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Azure DevOps (VSTS) task connector. Fetches work items assigned to the
// authenticated user via the WIQL API, then batch-resolves full item detail.
// State filtering is done client-side using the per-type states API so no state
// name is ever injected into WIQL (handles custom states and apostrophes safely).
//
// task_key format: `{project}#{work_item_id}` e.g. `Meridian#42`.
// Supports all three URL shapes: dev.azure.com/{org}, {org}.visualstudio.com,
// and on-premises {server}/{collection}. The api_base field is the resolved root.
//
// Verified end-to-end against dev.azure.com (cloud). On-premises support is
// built to the REST API spec but not live-tested.

use anyhow::{Context, Result};
use meridian_core::adapters::azure_devops::AzureDevopsAdapter;
use serde::Deserialize;
use serde_json::json;
use sqlx::SqlitePool;
use std::collections::HashMap;

use crate::config::AzureDevOpsConfig;

mod db;
mod fetch;
#[cfg(test)]
mod tests;

use db::{prune, stamp_sync};
use fetch::{fetch_batch, fetch_state_categories, run_wiql, BATCH_SIZE};

const SYNC_INTERVAL_MINS: i64 = 5;

// ---------------------------------------------------------------------------
// REST response shapes
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct WorkItemDetail {
    id: u64,
    fields: WorkItemFields,
}

#[derive(Deserialize)]
struct AzureIdentity {
    #[serde(rename = "displayName")]
    display_name: String,
}

#[derive(Deserialize)]
struct WorkItemFields {
    #[serde(rename = "System.Title")]
    title: String,
    #[serde(rename = "System.WorkItemType")]
    work_item_type: String,
    #[serde(rename = "System.State")]
    state: String,
    #[serde(rename = "System.ChangedDate", default)]
    changed_date: Option<String>,
    #[serde(rename = "System.Description", default)]
    description: Option<String>,
    /// Acceptance criteria (Scrum/Agile PBI, Bug, Feature, Epic).
    #[serde(rename = "Microsoft.VSTS.Common.AcceptanceCriteria", default)]
    acceptance_criteria: Option<String>,
    /// Repro steps — present on Bug work items only.
    #[serde(rename = "Microsoft.VSTS.TCM.ReproSteps", default)]
    repro_steps: Option<String>,
    /// Semicolon-delimited tag list e.g. `"backend; auth; spike"`.
    #[serde(rename = "System.Tags", default)]
    tags: Option<String>,
    /// Full iteration path e.g. `MyProject\Sprint 14`. Last segment = sprint name.
    #[serde(rename = "System.IterationPath", default)]
    iteration_path: Option<String>,
    #[serde(rename = "System.TeamProject", default)]
    team_project: Option<String>,
    #[serde(rename = "System.AssignedTo", default)]
    assigned_to: Option<AzureIdentity>,
    /// Direct parent work item ID (set by the hierarchy relation).
    #[serde(rename = "System.Parent", default)]
    parent_id: Option<u64>,
    /// Start date — present on all process types (Scheduling namespace).
    #[serde(rename = "Microsoft.VSTS.Scheduling.StartDate", default)]
    start_date: Option<String>,
    /// Target date — cross-process equivalent of due date (TargetDate, not DueDate
    /// which is Agile-process-only).
    #[serde(rename = "Microsoft.VSTS.Scheduling.TargetDate", default)]
    target_date: Option<String>,
}

// ---------------------------------------------------------------------------
// HTML helpers
// ---------------------------------------------------------------------------

/// Strip HTML tags and decode the most common entities so description fields
/// stored in pm_tasks contain readable plain text rather than raw HTML markup.
/// Azure DevOps returns HTML for Description, AcceptanceCriteria, ReproSteps, etc.
fn html_to_plaintext(html: &str) -> String {
    // Remove script/style blocks entirely (including their content).
    let mut s = String::with_capacity(html.len());
    let mut chars = html.chars().peekable();
    let mut in_tag = false;
    while let Some(ch) = chars.next() {
        if ch == '<' {
            in_tag = true;
            // Peek ahead: <br, </p, </div, </li → emit a space so words don't run together.
            let lookahead: String = chars.clone().take(4).collect();
            let l = lookahead.to_ascii_lowercase();
            if l.starts_with("br")
                || l.starts_with("/p")
                || l.starts_with("/di")
                || l.starts_with("/li")
            {
                s.push(' ');
            }
        } else if ch == '>' {
            in_tag = false;
        } else if !in_tag {
            s.push(ch);
        }
    }
    // Decode the most common HTML entities.
    let s = s
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&nbsp;", " ")
        .replace("&#160;", " ");
    // Collapse runs of whitespace (including newlines) to a single space.
    let mut out = String::with_capacity(s.len());
    let mut prev_space = true;
    for ch in s.chars() {
        if ch.is_ascii_whitespace() || ch == '\u{00a0}' {
            if !prev_space {
                out.push(' ');
            }
            prev_space = true;
        } else {
            out.push(ch);
            prev_space = false;
        }
    }
    out.trim().to_owned()
}

/// Extract the last path segment from an Azure DevOps iteration path.
/// `"MyProject\\Sprint 14"` → `"Sprint 14"`.
fn sprint_from_iteration_path(path: &str) -> String {
    path.rsplit('\\').next().unwrap_or(path).trim().to_owned()
}

/// Whether an Azure StateCategory is terminal. StateCategory is a fixed, reliable
/// metaschema field: Proposed | InProgress | Resolved | Completed | Removed.
/// Only Completed / Removed are terminal.
fn native_terminal(category: &str) -> bool {
    matches!(category, "Completed" | "Removed")
}

// ---------------------------------------------------------------------------
// Sync entry points
// ---------------------------------------------------------------------------

/// Refresh Azure DevOps tasks if the cache is stale (> SYNC_INTERVAL_MINS).
pub async fn refresh_if_stale(
    pool: &SqlitePool,
    cfg: &AzureDevOpsConfig,
) -> Result<Option<Vec<String>>> {
    let last_sync: Option<(String,)> = sqlx::query_as(
        "SELECT last_synced_at FROM pm_sync_state WHERE provider = 'azure_devops' LIMIT 1",
    )
    .fetch_optional(pool)
    .await
    .context("reading azure_devops sync state")?;

    if let Some((ts,)) = last_sync {
        if let Ok(last) = chrono::DateTime::parse_from_rfc3339(&ts) {
            let age_mins = chrono::Utc::now()
                .signed_duration_since(last.with_timezone(&chrono::Utc))
                .num_minutes();
            if age_mins < SYNC_INTERVAL_MINS {
                return Ok(None);
            }
        }
    }
    force_refresh(pool, cfg).await
}

/// Unconditionally refresh the Azure DevOps task cache.
///
/// # Why `Option`, like the other four providers
/// This used to return `Result<Vec<String>>` and propagate `Err` after already
/// recording a classified failure. That `Err` reached
/// `intelligence::run_pm_sync`'s generic catch-all, which wrote a SECOND,
/// unclassified, truncated notice on top of the correct one — so a transient
/// Azure DevOps blip still raised "Reconnect Azure DevOps in Settings", and this
/// provider's fix was undone by its own call site.
///
/// The other four providers never hit that because their `refresh_if_stale`
/// resolves to `Ok(None)` once it has handled a provider failure itself. This
/// now matches them: `Ok(None)` means "failed, already recorded, do not
/// re-report". Returning `Ok(Some(vec![]))` instead would be wrong — the caller
/// treats any `Ok(Some(_))` as success and calls `clear_sync_error`, wiping the
/// notice we just raised.
#[tracing::instrument(skip(pool, cfg), fields(provider = "azure_devops"))]
pub async fn force_refresh(
    pool: &SqlitePool,
    cfg: &AzureDevOpsConfig,
) -> Result<Option<Vec<String>>> {
    // The shared client: same timeouts as every other provider, plus a connect
    // deadline this one never had (it set a total timeout only, so a hung
    // connect still consumed the full 30 s).
    let client = super::http::client();

    // 1. WIQL: all work items assigned to me.
    let all_ids = match run_wiql(&client, cfg).await {
        Ok(ids) => ids,
        Err(e) => {
            // Was `stamp_error(pool, &e.to_string())`, which both reported every
            // network blip as a credentials fault AND printed only the outermost
            // context, dropping the cause.
            super::record_sync_failure(pool, "azure_devops", "wiql", &e).await?;
            // NOT `Err(e)`: already recorded and classified. Propagating it
            // would reach `run_pm_sync`'s generic catch-all, which reports
            // unconditionally and truncates - overwriting this with the very
            // bug this branch removes.
            return Ok(None);
        }
    };
    tracing::debug!(count = all_ids.len(), "azure_devops: WIQL returned IDs");

    if all_ids.is_empty() {
        prune(pool, &[]).await?;
        stamp_sync(pool).await?;
        return Ok(Some(vec![]));
    }

    // 2. Batch-fetch full detail (≤BATCH_SIZE per request). `raw_by_id` is the
    //    CDM (Stage 3b) escape hatch — used only at the final upsert step, kept
    //    separate so the ancestor/epic-resolution logic below (which never needs
    //    the raw payload) stays untouched.
    let mut details: Vec<WorkItemDetail> = Vec::with_capacity(all_ids.len());
    let mut raw_by_id: HashMap<u64, serde_json::Value> = HashMap::with_capacity(all_ids.len());
    for chunk in all_ids.chunks(BATCH_SIZE) {
        // Classified like the WIQL stage above rather than a bare `?`. A batch
        // failure is just as fatal to the sync, and propagating it unrecorded
        // meant a provider failing consistently HERE escalated never.
        let batch = match fetch_batch(&client, cfg, chunk).await {
            Ok(b) => b,
            Err(e) => {
                super::record_sync_failure(pool, "azure_devops", "work_item_batch", &e).await?;
                return Ok(None);
            }
        };
        for (detail, raw) in batch {
            raw_by_id.insert(detail.id, raw);
            details.push(detail);
        }
    }

    // 2b. Fetch ancestor items (parents not already in the WIQL set) so we can
    //     resolve epic_title without a DB traversal. These are never upserted into
    //     pm_tasks — they only populate the in-memory lookup used by resolve_epic_title.
    let mut fetched_set: std::collections::HashSet<u64> = details.iter().map(|d| d.id).collect();
    let mut ancestor_details: Vec<WorkItemDetail> = Vec::new();
    let mut parents_needed: std::collections::HashSet<u64> = details
        .iter()
        .filter_map(|d| d.fields.parent_id)
        .filter(|pid| !fetched_set.contains(pid))
        .collect();
    'levels: for _ in 0..4 {
        if parents_needed.is_empty() {
            break;
        }
        let ids_to_fetch: Vec<u64> = parents_needed.drain().collect();
        let mut next: std::collections::HashSet<u64> = std::collections::HashSet::new();
        for chunk in ids_to_fetch.chunks(BATCH_SIZE) {
            match fetch_batch(&client, cfg, chunk).await {
                Ok(batch) => {
                    next.extend(
                        batch
                            .iter()
                            .filter_map(|(d, _)| d.fields.parent_id)
                            .filter(|pid| !fetched_set.contains(pid)),
                    );
                    fetched_set.extend(chunk.iter().copied());
                    ancestor_details.extend(batch.into_iter().map(|(d, _)| d));
                }
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        "could not fetch ADO ancestor items — epic_title may be incomplete"
                    );
                    break 'levels;
                }
            }
        }
        parents_needed = next;
    }
    // Build id → (title, work_item_type, parent_id) from all fetched items.
    let id_meta: HashMap<u64, (&str, &str, Option<u64>)> = details
        .iter()
        .chain(ancestor_details.iter())
        .map(|d| {
            (
                d.id,
                (
                    d.fields.title.as_str(),
                    d.fields.work_item_type.as_str(),
                    d.fields.parent_id,
                ),
            )
        })
        .collect();

    // 3. Per-type state→category maps (fetched once per type per sync).
    let mut type_states: HashMap<String, HashMap<String, String>> = HashMap::new();
    for item in &details {
        let wit = item.fields.work_item_type.clone();
        if let std::collections::hash_map::Entry::Vacant(e) = type_states.entry(wit) {
            let map = fetch_state_categories(&client, cfg, e.key()).await;
            e.insert(map);
        }
    }

    // 4. Filter and upsert — Completed / Removed items are dropped.
    struct UpsertItem<'a> {
        detail: &'a WorkItemDetail,
        status: super::status::ResolvedStatus,
    }
    let active: Vec<UpsertItem<'_>> = details
        .iter()
        .filter_map(|item| {
            let azure_category = type_states
                .get(&item.fields.work_item_type)
                .and_then(|m| m.get(&item.fields.state))
                .map(|s| s.as_str())
                .unwrap_or("InProgress");
            if matches!(azure_category, "Completed" | "Removed") {
                return None;
            }
            // Store the user-facing System.State name verbatim; the StateCategory
            // gives the reliable terminal signal (always false for kept items).
            Some(UpsertItem {
                detail: item,
                status: super::status::resolve(
                    "azure_devops",
                    &item.fields.state,
                    Some(native_terminal(azure_category)),
                ),
            })
        })
        .collect();

    // cfg.api_base namespaces canonical ids — always available for all three
    // supported URL shapes, and only needs to be a stable per-workspace string.
    let cdm_adapter = AzureDevopsAdapter::new(cfg.api_base.clone());
    let mut kept: Vec<String> = Vec::with_capacity(active.len());
    for u in &active {
        let task_key = format!("{}#{}", cfg.project, u.detail.id);
        let f = &u.detail.fields;

        // Build composite description: base + acceptance criteria + repro steps.
        // All three are HTML fields from the API — strip tags before storing.
        let mut desc_parts: Vec<String> = Vec::new();
        let base = html_to_plaintext(f.description.as_deref().unwrap_or(""));
        if !base.is_empty() {
            desc_parts.push(base);
        }
        if let Some(ac) = &f.acceptance_criteria {
            let ac = html_to_plaintext(ac);
            if !ac.is_empty() {
                desc_parts.push(format!("Acceptance Criteria: {ac}"));
            }
        }
        if let Some(rs) = &f.repro_steps {
            let rs = html_to_plaintext(rs);
            if !rs.is_empty() {
                desc_parts.push(format!("Repro Steps: {rs}"));
            }
        }
        let description = desc_parts.join("\n\n");

        // Tags: normalise to comma-separated, trimming each tag.
        let tags: Option<String> = f.tags.as_deref().map(|t| {
            t.split(';')
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(", ")
        });

        // Sprint: last segment of the iteration path.
        let sprint = f
            .iteration_path
            .as_deref()
            .map(sprint_from_iteration_path)
            .filter(|s| !s.is_empty());

        let changed = f.changed_date.as_deref().unwrap_or("");
        let browser_url = format!(
            "{}/{}/_workitems/edit/{}",
            cfg.api_base, cfg.project, u.detail.id
        );
        let project_key = f.team_project.as_deref();
        let assignee_name = f.assigned_to.as_ref().map(|a| a.display_name.as_str());
        let parent_key: Option<String> = resolve_epic_id(&id_meta, f.parent_id)
            .or(f.parent_id)
            .map(|id| format!("{}#{}", cfg.project, id));
        let epic_title: Option<&str> = resolve_epic_title(&id_meta, f.parent_id);

        // CDM columns (Stage 3b) from the raw work item via the shared adapter.
        let empty = json!({});
        let raw = raw_by_id.get(&u.detail.id).unwrap_or(&empty);
        let cdm = super::cdm::derive(&cdm_adapter, raw);

        sqlx::query(
            "INSERT INTO pm_tasks
               (task_key, provider, title, description_text, status_raw, is_terminal,
                issue_type, project_key, url, updated_at, sprint_name, tags,
                assignee_name, start_date, due_date, parent_key, epic_title,
                canonical_id, status_category, raw_payload, reporter_name,
                completed_at, ancestor_path, project_ids)
             VALUES (?, 'azure_devops', ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?,
                     ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(task_key) DO UPDATE SET
               provider         = 'azure_devops',
               title            = excluded.title,
               description_text = excluded.description_text,
               status_raw       = excluded.status_raw,
               is_terminal      = excluded.is_terminal,
               issue_type       = excluded.issue_type,
               project_key      = excluded.project_key,
               url              = excluded.url,
               updated_at       = excluded.updated_at,
               sprint_name      = excluded.sprint_name,
               tags             = excluded.tags,
               assignee_name    = excluded.assignee_name,
               start_date       = excluded.start_date,
               due_date         = excluded.due_date,
               parent_key       = excluded.parent_key,
               epic_title       = excluded.epic_title,
               canonical_id     = excluded.canonical_id,
               status_category  = excluded.status_category,
               raw_payload      = excluded.raw_payload,
               reporter_name    = excluded.reporter_name,
               completed_at     = excluded.completed_at,
               ancestor_path    = excluded.ancestor_path,
               project_ids      = excluded.project_ids",
        )
        .bind(&task_key)
        .bind(&f.title)
        .bind(&description)
        .bind(&u.status.raw)
        .bind(u.status.is_terminal)
        .bind(&f.work_item_type)
        .bind(project_key)
        .bind(&browser_url)
        .bind(changed)
        .bind(sprint.as_deref())
        .bind(tags.as_deref())
        .bind(assignee_name)
        .bind(f.start_date.as_deref())
        .bind(f.target_date.as_deref())
        .bind(parent_key.as_deref())
        .bind(epic_title)
        .bind(cdm.canonical_id)
        .bind(cdm.status_category)
        .bind(cdm.raw_payload)
        .bind(cdm.reporter_name)
        .bind(cdm.completed_at)
        .bind(cdm.ancestor_path)
        .bind(cdm.project_ids)
        .execute(pool)
        .await
        .with_context(|| format!("upserting Azure DevOps work item {}", u.detail.id))?;

        kept.push(task_key);
    }

    prune(pool, &kept).await?;
    stamp_sync(pool).await?;

    tracing::info!(
        provider = "azure_devops",
        total_assigned = all_ids.len(),
        active = kept.len(),
        "azure_devops tasks refreshed"
    );
    Ok(Some(kept))
}

// ---------------------------------------------------------------------------
// Epic resolution
// ---------------------------------------------------------------------------

/// Walk the parent chain from `parent_id` and return the title of the nearest
/// ancestor whose `work_item_type == "Epic"`. Returns `None` when `parent_id`
/// is `None`, when no Epic is found in the chain, or when the chain exceeds 10
/// hops (cycle guard).
fn resolve_epic_title<'a>(
    id_meta: &'a HashMap<u64, (&'a str, &'a str, Option<u64>)>,
    parent_id: Option<u64>,
) -> Option<&'a str> {
    let mut current = parent_id?;
    for _ in 0..10 {
        let (title, wit, pid) = id_meta.get(&current)?;
        if *wit == "Epic" {
            return Some(title);
        }
        current = (*pid)?;
    }
    None
}

/// Walk the parent chain from `parent_id` and return the id of the nearest
/// ancestor whose `work_item_type == "Epic"`, so all tasks under the same Epic
/// share a single `parent_key` regardless of intermediate Story/Feature hops.
fn resolve_epic_id(
    id_meta: &HashMap<u64, (&str, &str, Option<u64>)>,
    parent_id: Option<u64>,
) -> Option<u64> {
    let mut current = parent_id?;
    for _ in 0..10 {
        let (_, wit, pid) = id_meta.get(&current)?;
        if *wit == "Epic" {
            return Some(current);
        }
        current = (*pid)?;
    }
    None
}
