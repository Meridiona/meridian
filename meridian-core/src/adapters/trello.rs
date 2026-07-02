//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Trello adapter — reference [`ProviderAdapter`] for Trello cards.
//!
//! Maps a Trello card (REST `/1/members/me/cards` shape) onto
//! [`CanonicalTask`], handling the Trello-specific traps from the six-provider
//! audit:
//!
//! - **Stable id = the 24-hex card `id`** (always returned by the API, even
//!   under a `fields=` filter). The human `shortLink` (8-char, used as the
//!   connector's task_key) is kept in `custom_fields.short_link`.
//! - **No status category.** Trello has no workflow semantics — a card is on a
//!   user-named list, and the connector's fetch doesn't resolve list names. The
//!   one reliable signal is `closed` (archived) → Done (Done vs Cancelled is
//!   indistinguishable for an archived card, matching the migration-056
//!   backfill's `is_terminal → 'done'` precedent); open cards get `None`.
//!   `status_raw` carries the list *id* (`idList`) — opaque but verbatim.
//! - **Multi-member `members`** → the multi-assignee `Vec<PersonRef>`.
//! - **No type / priority / reporter concepts** → `Task` with empty
//!   `kind_raw`, [`Priority::None`], `reporter: None` (the card creator is not
//!   in the card payload — it lives in the actions log, a separate endpoint).
//! - **The board is the project**: `idBoard` → a 1-element `project_ids`.
//! - **No created/updated timestamps on the card**: `dateLastActivity` is the
//!   closest thing to `updated_at`. (`created_at` is technically derivable
//!   from the id's leading 8 hex chars — a unix timestamp — but that's a
//!   derivation, not API data, so it's deliberately left `None`.)
//!
//! # Who calls this
//! `cdm_columns()` in the daemon's Trello connector
//! (`src/intelligence/providers/trello.rs`) at upsert time.
//!
//! # Related
//! - [`crate::adapters::ProviderAdapter`] — the trait this implements.
//! - [`crate::canonical_task`] — the output shape.

use super::ProviderAdapter;
use crate::canonical_task::{
    CanonicalTask, PersonRef, Priority, Provider, StatusCategory, TaskKind,
};
use anyhow::Context;
use serde_json::{json, Value};
use std::collections::BTreeMap;

/// Maps Trello cards to [`CanonicalTask`].
///
/// Config-less: a card carries everything needed (ids, urls, board/list ids)
/// so there's nothing per-workspace to thread in.
#[derive(Debug, Default, Clone, Copy)]
pub struct TrelloAdapter;

impl ProviderAdapter for TrelloAdapter {
    fn provider(&self) -> Provider {
        Provider::Trello
    }

    fn to_canonical(&self, raw: &Value) -> anyhow::Result<CanonicalTask> {
        let provider_id = raw
            .get("id")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .context("trello: card missing `id`")?
            .to_string();

        let mut custom_fields: BTreeMap<String, Value> = BTreeMap::new();
        // Human-facing key kept retrievable; the 24-hex id stays the key.
        if let Some(sl) = raw.get("shortLink").and_then(Value::as_str) {
            custom_fields.insert("short_link".to_string(), json!(sl));
        }
        if let Some(list) = str_field(raw, "idList") {
            custom_fields.insert("id_list".to_string(), json!(list));
        }
        if let Some(dc) = raw.get("dueComplete").and_then(Value::as_bool) {
            custom_fields.insert("due_complete".to_string(), json!(dc));
        }

        let closed = raw
            .get("closed")
            .and_then(Value::as_bool)
            .unwrap_or_default();

        let assignees = raw
            .get("members")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(|m| person(Some(m))).collect())
            .unwrap_or_default();

        let project_ids = str_field(raw, "idBoard")
            .map(|b| vec![CanonicalTask::canonical_id_for(Provider::Trello, &b)])
            .unwrap_or_default();

        let labels = raw
            .get("labels")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|l| l.get("name").and_then(Value::as_str))
                    .filter(|s| !s.is_empty())
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default();

        Ok(CanonicalTask {
            canonical_id: CanonicalTask::canonical_id_for(Provider::Trello, &provider_id),
            provider: Provider::Trello,
            provider_id,
            url: str_field(raw, "shortUrl")
                .or_else(|| str_field(raw, "url"))
                .unwrap_or_default(),
            title: str_field(raw, "name").unwrap_or_default(),
            description: str_field(raw, "desc").unwrap_or_default(),
            // Cards have no type concept.
            kind: TaskKind::Task,
            kind_raw: String::new(),
            // The list id is opaque but it IS the card's verbatim "where on the
            // board" — resolving the list *name* needs a second API call the
            // connector doesn't make.
            status_raw: str_field(raw, "idList").unwrap_or_default(),
            // Archived → Done (Done/Cancelled indistinguishable); open → None.
            status_category: closed.then_some(StatusCategory::Done),
            priority: Priority::None,
            assignees,
            // Creator lives in the actions log, not the card payload.
            reporter: None,
            // Trello has no card hierarchy.
            ancestor_path: Vec::new(),
            project_ids,
            // Derivable from the id's leading hex timestamp, but not API data.
            created_at: None,
            updated_at: str_field(raw, "dateLastActivity"),
            // No archived-at timestamp on the card payload.
            completed_at: None,
            start_date: str_field(raw, "start"),
            due_date: str_field(raw, "due"),
            labels,
            custom_fields,
            raw_payload: raw.clone(),
        })
    }
}

/// A non-empty string field, treating JSON null / absent / "" as `None`.
fn str_field(v: &Value, key: &str) -> Option<String> {
    v.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(String::from)
}

/// A Trello member object → [`PersonRef`]. Trello never exposes emails on
/// card members.
fn person(v: Option<&Value>) -> Option<PersonRef> {
    let v = v?;
    if v.is_null() {
        return None;
    }
    let provider_user_id = v
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let display_name = v
        .get("fullName")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .or_else(|| v.get("username").and_then(Value::as_str))
        .unwrap_or_default()
        .to_string();
    if provider_user_id.is_empty() && display_name.is_empty() {
        return None;
    }
    Some(PersonRef {
        email: None,
        display_name,
        provider_user_id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_card() -> Value {
        json!({
            "id": "5f4e6a1b2c3d4e5f6a7b8c9d",
            "shortLink": "HSkL1pnj",
            "name": "Adopt the canonical model",
            "desc": "Map Trello cards into CanonicalTask.",
            "idBoard": "board123",
            "idList": "list456",
            "dateLastActivity": "2026-06-28T12:00:00.000Z",
            "shortUrl": "https://trello.com/c/HSkL1pnj",
            "closed": false,
            "due": "2026-07-01T12:00:00.000Z",
            "start": "2026-06-20T00:00:00.000Z",
            "dueComplete": false,
            "labels": [{"name": "backend"}, {"name": "cdm"}, {"name": ""}],
            "members": [
                {"id": "m1", "fullName": "Developer", "username": "dev"},
                {"id": "m2", "fullName": "", "username": "pair"}
            ]
        })
    }

    #[test]
    fn maps_a_full_card() {
        let task = TrelloAdapter.to_canonical(&sample_card()).unwrap();

        // 24-hex id is the stable key; shortLink retrievable in custom_fields.
        assert_eq!(task.provider_id, "5f4e6a1b2c3d4e5f6a7b8c9d");
        assert_eq!(task.canonical_id, "trello:5f4e6a1b2c3d4e5f6a7b8c9d");
        assert_eq!(
            task.custom_fields.get("short_link"),
            Some(&json!("HSkL1pnj"))
        );

        // No type/priority/reporter concepts.
        assert_eq!(task.kind, TaskKind::Task);
        assert_eq!(task.kind_raw, "");
        assert_eq!(task.priority, Priority::None);
        assert!(task.reporter.is_none());

        // Open card → no category; the opaque list id is the raw status.
        assert_eq!(task.status_category, None);
        assert_eq!(task.status_raw, "list456");

        // Board = project; multi-member = multi-assignee (name falls back to
        // username); empty label names dropped.
        assert_eq!(task.project_ids, vec!["trello:board123"]);
        assert_eq!(task.assignees.len(), 2);
        assert_eq!(task.assignees[0].display_name, "Developer");
        assert_eq!(task.assignees[1].display_name, "pair");
        assert_eq!(task.labels, vec!["backend", "cdm"]);

        assert_eq!(task.start_date.as_deref(), Some("2026-06-20T00:00:00.000Z"));
        assert_eq!(task.due_date.as_deref(), Some("2026-07-01T12:00:00.000Z"));
        assert_eq!(task.updated_at.as_deref(), Some("2026-06-28T12:00:00.000Z"));
        assert!(!task.is_terminal());
    }

    #[test]
    fn archived_card_is_done() {
        let mut raw = sample_card();
        raw["closed"] = json!(true);
        let task = TrelloAdapter.to_canonical(&raw).unwrap();
        assert_eq!(task.status_category, Some(StatusCategory::Done));
        assert!(task.is_terminal());
    }

    #[test]
    fn sparse_card_maps_with_gaps() {
        // The connector's actual fields filter can omit almost everything.
        let raw = json!({"id": "5f4e6a1b2c3d4e5f6a7b8c9d", "name": "T"});
        let task = TrelloAdapter.to_canonical(&raw).unwrap();
        assert_eq!(task.canonical_id, "trello:5f4e6a1b2c3d4e5f6a7b8c9d");
        assert_eq!(task.status_category, None);
        assert!(task.assignees.is_empty());
        assert!(task.project_ids.is_empty());
        assert!(task.labels.is_empty());
    }

    #[test]
    fn missing_id_is_an_error() {
        assert!(TrelloAdapter
            .to_canonical(&json!({"shortLink": "HSkL1pnj"}))
            .is_err());
    }
}
