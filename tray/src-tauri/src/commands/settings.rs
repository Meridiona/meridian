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
//! replaced by the "Export Diagnostics" button in `AdvancedSection.tsx` — see
//! that component's history). This is intentional: OTLP shipping is a
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

/// Runtime settings for the dashboard (the ported /api/settings GET). Reads
/// `settings.json` via the shared meridian-core reader, coercing the nullable
/// string fields `null → ''` (TS consumers expect strings) and redacting the
/// stored password to a sentinel. Read-only counterpart to [`update_settings`].
#[tauri::command]
#[tracing::instrument]
pub async fn get_settings() -> Result<Value, String> {
    let mut v = meridian_core::settings::read_settings_value();
    redact_password(&mut v);
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
    pool: State<'_, Option<meridian_core::SqlitePool>>,
    app_state: State<'_, Arc<Mutex<AppState>>>,
    body: Value,
) -> Result<Value, String> {
    // `pool` is unused (settings live in a file, not the DB) but kept in the
    // signature for a uniform command shape; touch it so it isn't a dead param.
    let _ = pool;
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

    meridian_core::settings::write_settings_value(&updated).map_err(|e| {
        tracing::warn!(error = %e, "update_settings: write failed");
        e.to_string()
    })?;

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

    redact_password(&mut updated);
    Ok(updated)
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
