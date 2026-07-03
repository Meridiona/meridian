//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Shared CDM-column derivation (Stage 3b) — one helper for every connector.
//!
//! Each provider connector pairs its typed fetch with the raw JSON payload and
//! calls [`derive`] with its canonical adapter at upsert time; the result binds
//! straight into the seven CDM columns of `pm_tasks` (migration 056). This
//! replaces five per-connector copies of the identical struct + conversion.
//!
//! # Who calls this
//! The `upsert` path of every connector in this module tree:
//! [`super::jira`], [`super::linear`], [`super::azure_devops`],
//! [`super::github`], [`super::trello`].
//!
//! # Related
//! - `meridian_core::adapters` — the per-tracker `ProviderAdapter` impls whose
//!   output this flattens into column-ready strings.

use meridian_core::adapters::ProviderAdapter;
use serde_json::Value;

/// The CDM columns (migration 056) derived from a provider's raw API payload
/// via its canonical adapter. All `Option` so a structurally-unusable payload
/// (or one predating a connector's extended fetch) simply leaves the columns
/// NULL — never blocks the connector's existing upsert. The per-tracker
/// mapping itself lives in `meridian_core::adapters` and is tested there.
#[derive(Default)]
pub struct CdmColumns {
    pub canonical_id: Option<String>,
    pub status_category: Option<String>,
    pub raw_payload: Option<String>,
    pub reporter_name: Option<String>,
    pub completed_at: Option<String>,
    pub ancestor_path: Option<String>,
    pub project_ids: Option<String>,
}

/// Run `raw` through `adapter` and flatten the result into column-ready
/// strings: the status category as its snake_case serde wire form (e.g.
/// `"in_progress"`), the id vecs as JSON arrays, the reporter as a display
/// name, the payload re-serialised verbatim.
pub fn derive(adapter: &dyn ProviderAdapter, raw: &Value) -> CdmColumns {
    let Ok(c) = adapter.to_canonical(raw) else {
        return CdmColumns::default();
    };
    CdmColumns {
        canonical_id: Some(c.canonical_id),
        status_category: c
            .status_category
            .and_then(|sc| serde_json::to_value(sc).ok())
            .and_then(|v| v.as_str().map(String::from)),
        raw_payload: Some(c.raw_payload.to_string()),
        reporter_name: c.reporter.map(|r| r.display_name),
        completed_at: c.completed_at,
        ancestor_path: serde_json::to_string(&c.ancestor_path).ok(),
        project_ids: serde_json::to_string(&c.project_ids).ok(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use meridian_core::adapters::jira::JiraAdapter;

    // The column ENCODINGS (snake_case category, JSON arrays, NULL-on-error)
    // are locked here once; each connector's own tests only assert that it
    // routes through the right adapter.

    #[test]
    fn derive_flattens_to_column_ready_strings() {
        let raw = serde_json::json!({
            "id": "10042",
            "key": "KAN-42",
            "fields": {
                "status": {"name": "In Review", "statusCategory": {"key": "indeterminate"}},
                "reporter": {"accountId": "acc-2", "displayName": "Lead"},
                "parent": {"id": "10001"},
                "project": {"id": "10000"},
                "resolutiondate": null
            }
        });
        let cdm = derive(&JiraAdapter, &raw);
        assert_eq!(cdm.canonical_id.as_deref(), Some("jira:10042"));
        // Enum → snake_case wire form.
        assert_eq!(cdm.status_category.as_deref(), Some("in_review"));
        assert_eq!(cdm.reporter_name.as_deref(), Some("Lead"));
        assert_eq!(cdm.completed_at, None);
        // Vecs → JSON arrays.
        assert_eq!(cdm.ancestor_path.as_deref(), Some(r#"["jira:10001"]"#));
        assert_eq!(cdm.project_ids.as_deref(), Some(r#"["jira:10000"]"#));
        assert!(cdm.raw_payload.is_some());
    }

    #[test]
    fn derive_is_all_null_on_unusable_payload() {
        // Adapter error (missing stable id) → every column NULL, never a panic.
        let cdm = derive(&JiraAdapter, &serde_json::json!({"fields": {}}));
        assert!(cdm.canonical_id.is_none());
        assert!(cdm.status_category.is_none());
        assert!(cdm.raw_payload.is_none());
        assert!(cdm.reporter_name.is_none());
        assert!(cdm.completed_at.is_none());
        assert!(cdm.ancestor_path.is_none());
        assert!(cdm.project_ids.is_none());
    }
}
