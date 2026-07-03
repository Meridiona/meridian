//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! GitHub connector tests — CDM glue (fetch-shape tests live in `fetch::tests`).

use super::*;

// -----------------------------------------------------------------------
// CDM (Stage 3b): locks that this connector routes raw items through
// GithubAdapter. Encodings are tested once in providers::cdm; the mapping
// itself in meridian_core::adapters::github.
// -----------------------------------------------------------------------

#[test]
fn cdm_columns_derives_from_raw_item() {
    let raw = serde_json::json!({
        "type": "ISSUE",
        "project": {"id": "PVT_board"},
        "fieldValues": {"nodes": [
            {"name": "In Review", "field": {"name": "Status"}}
        ]},
        "content": {
            "id": "I_kwDOabc123",
            "number": 42,
            "state": "OPEN",
            "author": {"id": "U_lead", "login": "lead", "name": "Lead"},
            "parent": {"id": "I_kwDOparent"},
            "closedAt": null
        }
    });
    let cdm = crate::intelligence::providers::cdm::derive(&GithubAdapter, &raw);
    // Stable key is the global node id, namespaced.
    assert_eq!(cdm.canonical_id.as_deref(), Some("github:I_kwDOabc123"));
    // OPEN → no derivable category (board columns are user-defined).
    assert_eq!(cdm.status_category, None);
    assert_eq!(cdm.reporter_name.as_deref(), Some("Lead"));
    assert_eq!(cdm.completed_at, None);
    assert_eq!(
        cdm.ancestor_path.as_deref(),
        Some(r#"["github:I_kwDOparent"]"#)
    );
    assert_eq!(cdm.project_ids.as_deref(), Some(r#"["github:PVT_board"]"#));
    assert!(cdm.raw_payload.is_some());
}

#[test]
fn cdm_columns_empty_on_unusable_payload() {
    // No content.id (the pre-CDM query shape) → adapter errors → all
    // columns NULL, never blocks the upsert.
    let cdm = crate::intelligence::providers::cdm::derive(
        &GithubAdapter,
        &serde_json::json!({
        "type": "ISSUE",
        "fieldValues": {"nodes": []},
        "content": {"number": 5, "title": "old shape"}
        }),
    );
    assert!(cdm.canonical_id.is_none());
    assert!(cdm.raw_payload.is_none());
    assert!(cdm.status_category.is_none());
}
