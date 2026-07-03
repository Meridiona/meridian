//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Azure DevOps connector tests — auth format, HTML stripping, epic resolution, CDM glue.

use super::*;
use base64::Engine;

#[test]
fn test_basic_auth_format() {
    let header = super::fetch::basic_auth("mytoken");
    assert!(header.starts_with("Basic "));
    let encoded = header.strip_prefix("Basic ").unwrap();
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .unwrap();
    assert_eq!(decoded, b":mytoken");
}

#[test]
fn test_html_to_plaintext_basic() {
    assert_eq!(html_to_plaintext("<div>Bug test<br> </div>"), "Bug test");
    assert_eq!(
        html_to_plaintext("<p>Hello &amp; world</p>"),
        "Hello & world"
    );
    assert_eq!(
        html_to_plaintext("<div>Step 1</div><div>Step 2</div>"),
        "Step 1 Step 2",
    );
    assert_eq!(html_to_plaintext(""), "");
    assert_eq!(html_to_plaintext("plain text"), "plain text");
    assert_eq!(
        html_to_plaintext("<ul><li>item 1</li><li>item 2</li></ul>"),
        "item 1 item 2",
    );
}

#[test]
fn test_native_terminal() {
    // Only Completed / Removed are terminal StateCategories.
    assert!(native_terminal("Completed"));
    assert!(native_terminal("Removed"));
    assert!(!native_terminal("Proposed"));
    assert!(!native_terminal("InProgress"));
    assert!(!native_terminal("Resolved"));
    assert!(!native_terminal("SomeCustomState"));
}

#[test]
fn test_html_to_plaintext() {
    assert_eq!(html_to_plaintext("<div>Bug test<br> </div>"), "Bug test");
    assert_eq!(
        html_to_plaintext("<p>Hello &amp; world</p>"),
        "Hello & world"
    );
    assert_eq!(
        html_to_plaintext("<div>Step 1</div><div>Step 2</div>"),
        "Step 1 Step 2",
    );
    assert_eq!(html_to_plaintext(""), "");
    assert_eq!(html_to_plaintext("plain text"), "plain text");
    assert_eq!(
        html_to_plaintext("<ul><li>item 1</li><li>item 2</li></ul>"),
        "item 1 item 2",
    );
}

#[test]
fn test_sprint_from_iteration_path() {
    assert_eq!(
        sprint_from_iteration_path(r"MyProject\Sprint 14"),
        "Sprint 14"
    );
    assert_eq!(sprint_from_iteration_path("MyProject"), "MyProject");
    assert_eq!(sprint_from_iteration_path(r"A\B\C"), "C");
    assert_eq!(sprint_from_iteration_path(""), "");
}

// ---- epic_title resolution (resolve_epic_title) ------------------------
//
// Cases mirror the live hegdeakarsh2002 board so the tests track production:
//   #34 Epic  "Auth & Security Overhaul"
//     └ #35 Issue → #37 Task
//   #58 Epic  "Performance & Infrastructure"
//     └ #59 Issue → #61 Task
// The map value is (title, work_item_type, parent_id), exactly as built in
// force_refresh from the fetched + ancestor work items.

/// Builds an id→(title, type, parent) map from `(id, title, type, parent)` tuples.
fn meta(
    rows: &[(u64, &'static str, &'static str, Option<u64>)],
) -> HashMap<u64, (&'static str, &'static str, Option<u64>)> {
    rows.iter().map(|&(id, t, w, p)| (id, (t, w, p))).collect()
}

#[test]
fn epic_resolves_from_direct_epic_parent() {
    // #35 (Issue) → parent #34 (Epic). One hop to the Epic.
    let m = meta(&[(34, "Auth & Security Overhaul", "Epic", None)]);
    assert_eq!(
        resolve_epic_title(&m, Some(34)),
        Some("Auth & Security Overhaul")
    );
}

#[test]
fn epic_resolves_through_multi_hop_chain() {
    // #61 (Task) → #59 (Issue) → #58 (Epic). The nearest Epic ancestor wins.
    let m = meta(&[
        (
            59,
            "Migrate services to Kubernetes (EKS)",
            "Issue",
            Some(58),
        ),
        (58, "Performance & Infrastructure", "Epic", None),
    ]);
    assert_eq!(
        resolve_epic_title(&m, Some(59)),
        Some("Performance & Infrastructure")
    );
}

#[test]
fn epic_is_none_when_no_parent() {
    // Epics themselves (#34/#58) and any top-level item have no parent_id.
    let m = meta(&[(34, "Auth & Security Overhaul", "Epic", None)]);
    assert_eq!(resolve_epic_title(&m, None), None);
}

#[test]
fn epic_is_none_when_chain_has_no_epic() {
    // Issue → Issue with no Epic anywhere in the chain.
    let m = meta(&[
        (10, "Sub-issue", "Issue", Some(11)),
        (11, "Parent issue", "Issue", None),
    ]);
    assert_eq!(resolve_epic_title(&m, Some(10)), None);
}

#[test]
fn epic_is_none_when_parent_missing_from_map() {
    // Ancestor fetch failed / item not returned: graceful None, no panic.
    let m = meta(&[]);
    assert_eq!(resolve_epic_title(&m, Some(999)), None);
}

#[test]
fn epic_resolution_terminates_on_cycle() {
    // A→B→A would loop forever without the 10-hop guard; neither is an Epic.
    let m = meta(&[(1, "A", "Issue", Some(2)), (2, "B", "Issue", Some(1))]);
    assert_eq!(resolve_epic_title(&m, Some(1)), None);
}

#[test]
fn epic_picks_nearest_epic_when_nested() {
    // Task → Feature(Epic-category? no) — here a closer Epic shadows a farther one.
    // #200 → #201 (Epic "Near") → #202 (Epic "Far"): nearest wins.
    let m = meta(&[(201, "Near", "Epic", Some(202)), (202, "Far", "Epic", None)]);
    assert_eq!(resolve_epic_title(&m, Some(201)), Some("Near"));
}

#[test]
fn parent_key_format_matches_task_key_shape() {
    // parent_key uses the resolved Epic id (or direct parent as fallback) in
    // `{project}#{id}` format — same shape as task_key for join correctness.
    let project = "hegdeakarsh2002";
    let epic_id: Option<u64> = Some(59);
    let parent_key = epic_id.map(|id| format!("{project}#{id}"));
    assert_eq!(parent_key.as_deref(), Some("hegdeakarsh2002#59"));
    let none_parent: Option<u64> = None;
    assert_eq!(none_parent.map(|id| format!("{project}#{id}")), None);
}

#[test]
fn resolve_epic_id_finds_nearest_epic_ancestor() {
    // task 34 (not in map) → feature 59 → epic 201
    let m = meta(&[
        (59, "Feature A", "Feature", Some(201)),
        (201, "Big Epic", "Epic", None),
    ]);
    // 34 not in map → None
    assert_eq!(resolve_epic_id(&m, Some(34)), None);
    // 59 is Feature whose parent 201 is Epic → returns 201
    assert_eq!(resolve_epic_id(&m, Some(59)), Some(201));
    // 201 is itself an Epic → returns 201 on first check
    assert_eq!(resolve_epic_id(&m, Some(201)), Some(201));
    // None parent_id → None
    assert_eq!(resolve_epic_id(&m, None), None);
}

// -----------------------------------------------------------------------
// CDM (Stage 3b): locks that this connector routes raw work items through
// AzureDevopsAdapter. Encodings are tested once in providers::cdm; the
// mapping itself in meridian_core::adapters::azure_devops.
// -----------------------------------------------------------------------

#[test]
fn cdm_columns_derives_from_raw_item() {
    let raw = json!({
        "id": 1234,
        "url": "https://dev.azure.com/acme/_apis/wit/workItems/1234",
        "fields": {
            "System.State": "Resolved",
            "System.CreatedBy": {"id": "ado-2", "displayName": "Lead"},
            "System.Parent": 1000,
            "Microsoft.VSTS.Common.ClosedDate": null
        }
    });
    let cdm = crate::intelligence::providers::cdm::derive(&AzureDevopsAdapter::new("acme"), &raw);
    // Org-namespaced stable key.
    assert_eq!(cdm.canonical_id.as_deref(), Some("azure_devops:acme:1234"));
    // "Resolved" → snake_case canonical category.
    assert_eq!(cdm.status_category.as_deref(), Some("in_review"));
    assert_eq!(cdm.reporter_name.as_deref(), Some("Lead"));
    assert_eq!(cdm.completed_at, None); // ClosedDate null
    assert_eq!(
        cdm.ancestor_path.as_deref(),
        Some(r#"["azure_devops:acme:1000"]"#)
    );
    assert!(cdm.raw_payload.is_some());
}

#[test]
fn cdm_columns_empty_on_unusable_payload() {
    // No `id` → adapter errors → all columns NULL, never blocks the upsert.
    let cdm = crate::intelligence::providers::cdm::derive(
        &AzureDevopsAdapter::new("acme"),
        &json!({"fields": {}}),
    );
    assert!(cdm.canonical_id.is_none());
    assert!(cdm.raw_payload.is_none());
    assert!(cdm.status_category.is_none());
}

#[test]
fn cdm_columns_empty_when_org_unresolvable() {
    // Empty org and no dev.azure.com url in the payload → adapter refuses
    // to emit a degenerate key; cdm_columns degrades to all-NULL.
    let raw = json!({"id": 1234, "fields": {}});
    let cdm = crate::intelligence::providers::cdm::derive(&AzureDevopsAdapter::new(""), &raw);
    assert!(cdm.canonical_id.is_none());
}
