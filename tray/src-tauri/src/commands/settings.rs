//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Settings commands — the ported `/api/settings` GET + PUT.
//!
//! Read the runtime settings (`settings.json`) for the dashboard, and persist
//! edits. Settings live in a FILE (not the DB), so these read/write through
//! [`meridian_core::settings`] (the shared schema + path the daemon also uses) —
//! the one exception to "meridian-core is DB-only", because the daemon must read
//! the same file each poll tick.
//!
//! `oo_password` never leaves the daemon side in cleartext: GET redacts it to a
//! sentinel, and PUT treats the sentinel (or empty) as "keep the stored value".
//!
//! # Who calls this
//! Registered in `lib.rs`'s `invoke_handler!`; consumed by
//! `ui/components/views/SettingsView.tsx` (read via `load`, save via `mutate`).
//!
//! # Related
//! - [`meridian_core::settings`] — the schema, path, and the value read/write.
//! - [`crate::commands::daemon`] — the daemon-reload step the Settings "Apply"
//!   flow chains after a save. `otlp_enabled`/`otlp_endpoint`/`oo_email`/
//!   `oo_password` remain valid fields (consumed by a Dev/Bare install's
//!   shipper — see `src/observability.rs`) but the shipped app no longer
//!   installs/manages a local OpenObserve service for them.
//!
//! **No UI writes these OTLP-shipping fields anymore** (the config panel was
//! replaced by the "Export Diagnostics" button, now in `AccountSection.tsx`
//! — see that component's history). This is intentional: OTLP shipping is a
//! Dev/Bare-only, engineer-facing debugging feature (a packaged/Canonical
//! install can never ship, regardless of these fields — see
//! `is_canonical_install()`), so an engineer who wants their dev daemon to
//! ship live to their own OpenObserve sets `otlp_enabled`/`otlp_endpoint`/
//! `oo_email`/`oo_password` by hand-editing `~/.meridian/settings.json` (this
//! command still validates and persists them if written that way, or via a
//! future re-added UI). Every end-user-facing workflow goes through Export
//! Diagnostics instead, which needs none of these fields.

use crate::capture_ignore::CaptureIgnore;
use crate::state::AppState;
use serde_json::Value;
use std::sync::{Arc, Mutex};
use tauri::State;

/// Returned to the UI when a password is stored — the real value never leaves the
/// daemon side. Matches the route's sentinel; PUT recognises it as "unchanged".
const PASSWORD_SENTINEL: &str = "••••••••";

/// Redact `oo_password` in a settings object: sentinel if a non-empty value is
/// stored, empty string otherwise (mirrors both routes' response shaping).
fn redact_password(v: &mut Value) {
    if let Some(obj) = v.as_object_mut() {
        let has_pw = obj
            .get("oo_password")
            .and_then(Value::as_str)
            .is_some_and(|p| !p.is_empty());
        obj.insert(
            "oo_password".into(),
            Value::String(if has_pw { PASSWORD_SENTINEL } else { "" }.into()),
        );
    }
}

/// Redact every custom endpoint's `api_key`, per row, to the same sentinel.
///
/// Same contract as [`redact_password`], but over a LIST, which is the harder round trip:
/// the UI gets N sentinels back and PUTs the whole array again after editing one field, so
/// the restore below must match each row BY ID. Getting that wrong overwrites the key of a
/// row the user never touched with the sentinel string itself — a silently broken endpoint
/// whose key is gone.
fn redact_custom_keys(v: &mut Value) {
    let Some(rows) = v
        .get_mut("custom_llm_providers")
        .and_then(Value::as_array_mut)
    else {
        return;
    };
    for row in rows {
        let Some(obj) = row.as_object_mut() else {
            continue;
        };
        let has_key = obj
            .get("api_key")
            .and_then(Value::as_str)
            .is_some_and(|k| !k.is_empty());
        obj.insert(
            "api_key".into(),
            Value::String(if has_key { PASSWORD_SENTINEL } else { "" }.into()),
        );
    }
}

/// Runtime settings for the dashboard (the ported /api/settings GET). Reads
/// `settings.json` via the shared meridian-core reader, coercing the nullable
/// string fields `null → ''` (TS consumers expect strings) and redacting the
/// stored password + every custom endpoint's API key to a sentinel. Read-only
/// counterpart to [`update_settings`].
#[tauri::command]
#[tracing::instrument]
pub async fn get_settings() -> Result<Value, String> {
    let mut v = meridian_core::settings::read_settings_value();
    redact_password(&mut v);
    redact_custom_keys(&mut v);
    Ok(v)
}

/// Persist a settings edit (the ported /api/settings PUT). Validates the OTLP
/// endpoint + credentials, merges the body over the current settings (body wins,
/// preserving any extra keys), keeps the stored password when the sentinel/empty
/// is sent, writes crash-safely, and returns the merged settings (password
/// redacted). `body` is one payload object so the Tauri + browser paths match.
#[tauri::command]
#[tracing::instrument(skip(pool, app_state, body))]
pub async fn update_settings(
    app: tauri::AppHandle,
    pool: State<'_, Option<meridian_core::SqlitePool>>,
    app_state: State<'_, Arc<Mutex<AppState>>>,
    body: Value,
) -> Result<Value, String> {
    let Some(body_obj) = body.as_object() else {
        return Err("settings body must be an object".to_string());
    };

    // Validate the OTLP endpoint — must be http/https when non-empty.
    if let Some(ep) = body_obj.get("otlp_endpoint").and_then(Value::as_str) {
        let ep = ep.trim();
        if !ep.is_empty() && !ep.starts_with("http://") && !ep.starts_with("https://") {
            return Err("otlp_endpoint must start with http:// or https://".to_string());
        }
    }
    // Validate the AI provider. This command is the ONLY writer of settings.json, so it
    // is the only place the check is needed — and it matters: an unrecognised value would
    // silently fall back to on-device at resolve time, so the user would pick "claude",
    // see it saved, and quietly keep getting the local model. Reject it at the door
    // instead. (The field stays a `String` on disk on purpose — see llm_provider.rs.)
    if let Some(p) = body_obj.get("llm_provider").and_then(Value::as_str) {
        if meridian_core::LlmProvider::from_wire(p).is_none() {
            let valid: Vec<&str> = meridian_core::LlmProvider::all()
                .iter()
                .map(|p| p.as_str())
                .collect();
            return Err(format!("llm_provider must be one of: {}", valid.join(", ")));
        }
    }
    // Reject newlines in credentials (HTTP header-injection vector).
    for field in ["oo_email", "oo_password"] {
        if let Some(v) = body_obj.get(field).and_then(Value::as_str) {
            if v.contains('\n') || v.contains('\r') {
                return Err(format!("{field} contains invalid characters"));
            }
        }
    }
    validate_worklog_auto_generate_time(body_obj)?;

    let current = meridian_core::settings::read_settings_value();
    let mut updated = current.clone();
    let obj = updated
        .as_object_mut()
        .ok_or("current settings are not an object")?;
    // { ...current, ...body } — body keys override.
    for (k, v) in body_obj {
        obj.insert(k.clone(), v.clone());
    }

    // Sentinel / empty / absent oo_password → keep the stored value.
    let sent = body_obj.get("oo_password").and_then(Value::as_str);
    if sent.is_none_or(|p| p.is_empty() || p == PASSWORD_SENTINEL) {
        let kept = current
            .get("oo_password")
            .cloned()
            .unwrap_or(Value::String(String::new()));
        obj.insert("oo_password".into(), kept);
    }

    // Same rule per custom endpoint, matched BY ID (see `redact_custom_keys`). The merge
    // above is shallow, so a body carrying the registry replaces it wholesale — and the UI's
    // copy has a sentinel in every row it read back. Without this, saving any unrelated
    // setting would overwrite every API key with "••••••••".
    restore_custom_keys(&current, obj)?;

    // The GATE. A custom endpoint may only run the pipeline on measured evidence
    // (`meridian_core::SchemaRung`), and it is enforced HERE rather than only in the UI:
    // this command is the sole writer of settings.json, so a hand-edited file or an older
    // frontend would otherwise route a user's whole day through an endpoint nobody has
    // shown can hold a schema. An unenforced fold doesn't fail loudly — it drops the hour.
    enforce_custom_provider_gate(&updated)?;

    meridian_core::settings::write_settings_value(&updated)
        .map_err(|e| crate::cmd_err!(e, "update_settings: write failed"))?;

    // Refresh the live capture ignore list so a Settings change takes effect on
    // the very next captured frame — no capture restart. The frame + UI-event
    // consumers read this same shared handle (see `crate::start_capture`).
    let apps = str_list(&updated, "ignored_apps");
    let urls = str_list(&updated, "ignored_urls");
    if let Ok(mut ig) = app_state.lock().unwrap().capture_ignore.lock() {
        *ig = CaptureIgnore::new(&apps, &urls);
    }
    tracing::info!(
        ignored_apps = apps.len(),
        ignored_urls = urls.len(),
        "update_settings: capture ignore list refreshed"
    );

    // Switching AI provider changes `llm_provider_ok` the moment this write lands - the new
    // provider may be signed out, or the old failure may no longer apply. The banner is
    // push-only, so without this it keeps naming the PREVIOUS provider until the poll loop's
    // next 60 s health tick. Only on the keys that can move it; every other setting leaves
    // provider health alone and shouldn't pay for a health check.
    if body_obj.contains_key("llm_provider") || body_obj.contains_key("llm_provider_custom_id") {
        crate::commands::health::push_health_update(&app).await;

        // Same fix, for the SAME reason, applied to `llm.groq_deprecated` - the daemon's poll
        // tick (`main.rs`) calls the identical `sync_groq_deprecated_notice`, so switching
        // away from Groq here and there converge on the same answer; this just stops the
        // banner from sitting stale for up to a minute after the switch already took effect.
        if let Some(p) = pool.as_ref() {
            let vendor = meridian_core::settings::load_runtime_settings()
                .active_custom_provider()
                .map(|c| c.vendor.clone());
            meridian::notices::sync_groq_deprecated_notice(p, vendor.as_deref()).await;
            crate::commands::notices::push_notices_update(&app, p).await;
        }
    }

    redact_password(&mut updated);
    redact_custom_keys(&mut updated);
    Ok(updated)
}

/// Put back each endpoint's stored key wherever the body sent the sentinel (or nothing),
/// matching on `id`.
///
/// A row whose id is NOT in the current settings is a genuinely new endpoint, and a new
/// endpoint arriving with a sentinel key is rejected rather than saved keyless: it would
/// look configured and fail on first use. (The supported way to add one is
/// `add_custom_llm_provider`, which also measures it — this path exists for edits.)
fn restore_custom_keys(
    current: &Value,
    obj: &mut serde_json::Map<String, Value>,
) -> Result<(), String> {
    let Some(rows) = obj
        .get_mut("custom_llm_providers")
        .and_then(Value::as_array_mut)
    else {
        return Ok(());
    };
    let stored = current
        .get("custom_llm_providers")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    for row in rows.iter_mut() {
        let Some(o) = row.as_object_mut() else {
            continue;
        };
        let id = o
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let sent = o.get("api_key").and_then(Value::as_str);
        if sent.is_some_and(|k| !k.is_empty() && k != PASSWORD_SENTINEL) {
            continue; // a real, newly-typed key — take it
        }
        let kept = stored
            .iter()
            .find(|s| s.get("id").and_then(Value::as_str) == Some(id.as_str()))
            .and_then(|s| s.get("api_key"))
            .cloned();
        match kept {
            Some(k) => {
                o.insert("api_key".into(), k);
            }
            None => {
                return Err(format!(
                    "custom endpoint \"{id}\" has no API key - add it with the endpoint form"
                ))
            }
        }
    }
    Ok(())
}

/// Validate a present-but-non-null `worklog_auto_generate_time` as a proper
/// "HH:MM" 24-hour local time. A malformed value would silently never match the
/// daemon's hourly clock check (`pm_worklog::auto_generate`) and the feature
/// would just never fire — reject it at the door instead of failing quietly
/// later. Absent or explicit `null` (turning the feature off) both pass.
fn validate_worklog_auto_generate_time(
    body_obj: &serde_json::Map<String, Value>,
) -> Result<(), String> {
    let Some(v) = body_obj.get("worklog_auto_generate_time") else {
        return Ok(());
    };
    if v.is_null() {
        return Ok(());
    }
    let s = v
        .as_str()
        .ok_or("worklog_auto_generate_time must be a string")?;
    let valid = s
        .split_once(':')
        .and_then(|(h, m)| Some((h.parse::<u32>().ok()?, m.parse::<u32>().ok()?)))
        .is_some_and(|(h, m)| h < 24 && m < 60 && s.len() == 5);
    if !valid {
        return Err("worklog_auto_generate_time must be \"HH:MM\" 24-hour local time".to_string());
    }
    Ok(())
}

/// Refuse to select a custom endpoint that isn't configured, isn't fully measured, or was
/// measured too weak for production. Any other provider passes untouched.
fn enforce_custom_provider_gate(updated: &Value) -> Result<(), String> {
    if updated.get("llm_provider").and_then(Value::as_str) != Some("custom") {
        return Ok(());
    }
    let settings: meridian_core::settings::RuntimeSettings =
        serde_json::from_value(updated.clone()).map_err(|e| format!("settings: {e}"))?;

    let Some(row) = settings.active_custom_provider() else {
        return Err(
            "select which custom endpoint to use (llm_provider_custom_id names no configured \
             endpoint)"
                .to_string(),
        );
    };
    if !row.is_fully_probed() {
        return Err(format!(
            "\"{}\" is not fully tested yet ({} still unmeasured) - press Test on its card, \
             then select it",
            row.name,
            row.unmeasured_schemas().join(", ")
        ));
    }
    if !row.is_production_eligible() {
        return Err(format!(
            "\"{}\" cannot hold the response schema reliably (measured: {:?}) - it can still be \
             compared in the LLM Lab, but it cannot run your worklogs",
            row.name,
            row.effective_rung()
        ));
    }
    tracing::info!(
        endpoint_id = %row.id,
        effective = ?row.effective_rung(),
        "settings: custom provider selected (gate passed)"
    );
    Ok(())
}

/// Pull a JSON string-array field out of a settings object as `Vec<String>`,
/// dropping non-string entries. An absent or non-array field yields an empty vec.
fn str_list(v: &Value, key: &str) -> Vec<String> {
    v.get(key)
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|e| e.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn row(id: &str, key: &str, rungs: Value) -> Value {
        json!({"id": id, "vendor": "gemini", "name": id, "base_url": "https://x.test/v1",
               "model": "m", "api_key": key, "rungs": rungs})
    }

    fn all_strict() -> Value {
        let mut m = serde_json::Map::new();
        for k in meridian_core::settings::PIPELINE_SCHEMA_KEYS {
            m.insert(k.to_string(), json!("strict"));
        }
        Value::Object(m)
    }

    /// The read side: a real key must never reach the UI, and an empty one must not look
    /// configured.
    #[test]
    fn redaction_hides_every_row_s_key() {
        let mut v = json!({"custom_llm_providers": [row("a", "sk-real", json!({})),
                                                    row("b", "", json!({}))]});
        redact_custom_keys(&mut v);
        let rows = v["custom_llm_providers"].as_array().unwrap();
        assert_eq!(rows[0]["api_key"], json!(PASSWORD_SENTINEL));
        assert_eq!(rows[1]["api_key"], json!(""));
        assert!(!serde_json::to_string(&v).unwrap().contains("sk-real"));
    }

    /// THE round-trip bug this guard exists for: the UI reads two redacted rows, edits one
    /// row's model, and PUTs the array back. Both rows carry a sentinel. Without a per-row
    /// restore, both keys become "••••••••" — and the user's endpoints are silently dead.
    #[test]
    fn a_redacted_round_trip_keeps_every_row_s_key() {
        let current = json!({"custom_llm_providers": [row("a", "sk-a", json!({})),
                                                      row("b", "sk-b", json!({}))]});
        let mut body = json!({"custom_llm_providers": [
            row("a", PASSWORD_SENTINEL, json!({})),
            row("b", PASSWORD_SENTINEL, json!({}))]});
        let obj = body.as_object_mut().unwrap();
        restore_custom_keys(&current, obj).expect("restore");

        let rows = obj["custom_llm_providers"].as_array().unwrap();
        assert_eq!(rows[0]["api_key"], json!("sk-a"));
        assert_eq!(
            rows[1]["api_key"],
            json!("sk-b"),
            "the untouched row keeps its own key"
        );
    }

    /// Rows must match by id, not by position — reordering or deleting one in the UI must
    /// not hand row A's key to row B.
    #[test]
    fn keys_are_restored_by_id_not_by_position() {
        let current = json!({"custom_llm_providers": [row("a", "sk-a", json!({})),
                                                      row("b", "sk-b", json!({}))]});
        // Same rows, reversed.
        let mut body = json!({"custom_llm_providers": [
            row("b", PASSWORD_SENTINEL, json!({})),
            row("a", PASSWORD_SENTINEL, json!({}))]});
        let obj = body.as_object_mut().unwrap();
        restore_custom_keys(&current, obj).expect("restore");

        let rows = obj["custom_llm_providers"].as_array().unwrap();
        assert_eq!(rows[0]["id"], json!("b"));
        assert_eq!(rows[0]["api_key"], json!("sk-b"));
        assert_eq!(rows[1]["api_key"], json!("sk-a"));
    }

    /// A newly-typed key must win over the stored one, or the key could never be changed.
    #[test]
    fn a_freshly_typed_key_replaces_the_stored_one() {
        let current = json!({"custom_llm_providers": [row("a", "sk-old", json!({}))]});
        let mut body = json!({"custom_llm_providers": [row("a", "sk-new", json!({}))]});
        let obj = body.as_object_mut().unwrap();
        restore_custom_keys(&current, obj).expect("restore");
        assert_eq!(obj["custom_llm_providers"][0]["api_key"], json!("sk-new"));
    }

    /// A row with no stored key and no typed one would save as configured-but-keyless and
    /// fail on first use. Reject it instead.
    #[test]
    fn a_new_row_without_a_key_is_rejected() {
        let current = json!({"custom_llm_providers": []});
        let mut body = json!({"custom_llm_providers": [row("new", PASSWORD_SENTINEL, json!({}))]});
        let obj = body.as_object_mut().unwrap();
        assert!(restore_custom_keys(&current, obj).is_err());
    }

    /// The gate: fully measured + strong enough passes.
    #[test]
    fn the_gate_passes_a_fully_measured_strong_endpoint() {
        let v = json!({"llm_provider": "custom", "llm_provider_custom_id": "a",
                       "custom_llm_providers": [row("a", "sk", all_strict())]});
        assert!(enforce_custom_provider_gate(&v).is_ok());
    }

    /// A partial probe must not be selectable — the very case a free-tier 429 produces.
    #[test]
    fn the_gate_refuses_a_partially_measured_endpoint() {
        let mut rungs = all_strict();
        rungs.as_object_mut().unwrap().remove("plan_task_draft");
        let v = json!({"llm_provider": "custom", "llm_provider_custom_id": "a",
                       "custom_llm_providers": [row("a", "sk", rungs)]});
        let err = enforce_custom_provider_gate(&v).expect_err("a half-probed endpoint");
        assert!(err.contains("not fully tested"), "{err}");
        assert!(
            err.contains("plan_task_draft"),
            "must name what is missing: {err}"
        );
    }

    /// Measured, complete, but too weak to guarantee shape → Lab-only.
    #[test]
    fn the_gate_refuses_an_endpoint_measured_too_weak() {
        let mut rungs = all_strict();
        rungs
            .as_object_mut()
            .unwrap()
            .insert("workstream".into(), json!("prompt"));
        let v = json!({"llm_provider": "custom", "llm_provider_custom_id": "a",
                       "custom_llm_providers": [row("a", "sk", rungs)]});
        let err = enforce_custom_provider_gate(&v).expect_err("a weak endpoint");
        assert!(err.contains("LLM Lab"), "must point at the Lab: {err}");
    }

    /// "custom" naming nothing configured, or nothing at all, is a mis-set provider.
    #[test]
    fn the_gate_refuses_custom_with_a_dangling_or_absent_id() {
        let v = json!({"llm_provider": "custom", "llm_provider_custom_id": "gone",
                       "custom_llm_providers": [row("a", "sk", all_strict())]});
        assert!(enforce_custom_provider_gate(&v).is_err());
        let v = json!({"llm_provider": "custom", "custom_llm_providers": []});
        assert!(enforce_custom_provider_gate(&v).is_err());
    }

    /// Every other provider is untouched by the gate — including with a weak endpoint
    /// merely configured alongside.
    #[test]
    fn the_gate_ignores_non_custom_providers() {
        for p in ["local", "claude", "codex"] {
            let v = json!({"llm_provider": p, "llm_provider_custom_id": "a",
                           "custom_llm_providers": [row("a", "sk", json!({}))]});
            assert!(enforce_custom_provider_gate(&v).is_ok(), "{p}");
        }
    }

    /// Absent, or explicitly `null` (turning auto-generate off), both pass untouched.
    #[test]
    fn worklog_time_validation_passes_when_absent_or_null() {
        let body = json!({}).as_object().unwrap().clone();
        assert!(validate_worklog_auto_generate_time(&body).is_ok());
        let body = json!({"worklog_auto_generate_time": null})
            .as_object()
            .unwrap()
            .clone();
        assert!(validate_worklog_auto_generate_time(&body).is_ok());
    }

    /// Every valid "HH:MM" in range passes, including the midnight/end-of-day edges.
    #[test]
    fn worklog_time_validation_accepts_valid_times() {
        for t in ["00:00", "09:05", "18:00", "23:59"] {
            let body = json!({"worklog_auto_generate_time": t})
                .as_object()
                .unwrap()
                .clone();
            assert!(
                validate_worklog_auto_generate_time(&body).is_ok(),
                "{t} should be valid"
            );
        }
    }

    /// Out-of-range hours/minutes, wrong shape, and non-string values are all rejected —
    /// a malformed value would otherwise silently never match the daemon's hourly check
    /// and the feature would just never fire, with no error anywhere to explain why.
    #[test]
    fn worklog_time_validation_rejects_malformed_values() {
        for t in [
            "24:00",
            "18:60",
            "9:05",
            "18:0",
            "18",
            "18:00:00",
            "",
            "not-a-time",
        ] {
            let body = json!({"worklog_auto_generate_time": t})
                .as_object()
                .unwrap()
                .clone();
            assert!(
                validate_worklog_auto_generate_time(&body).is_err(),
                "{t:?} should be rejected"
            );
        }
        let body = json!({"worklog_auto_generate_time": 1800})
            .as_object()
            .unwrap()
            .clone();
        assert!(validate_worklog_auto_generate_time(&body).is_err());
    }
}
