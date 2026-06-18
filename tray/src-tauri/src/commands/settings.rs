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
//! - [`crate::commands::openobserve`] / [`crate::commands::daemon`] — the
//!   service-start + daemon-reload steps the Settings "Apply" flow chains after a save.

use serde_json::Value;
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
#[tracing::instrument(skip(pool, body))]
pub async fn update_settings(
    pool: State<'_, Option<meridian_core::SqlitePool>>,
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

    redact_password(&mut updated);
    Ok(updated)
}
