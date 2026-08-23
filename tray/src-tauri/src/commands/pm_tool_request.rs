//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! "I don't see my tool" — the escape hatch on `ConnectTrackers` for a PM tool
//! outside the five Meridian supports today (Jira/Linear/GitHub/Trello/Azure
//! DevOps, see `ui/lib/integrations.ts::TRACKERS`).
//!
//! Two independent, best-effort effects from one submit:
//! - a local mirror in `settings.json`'s `RuntimeSettings::requested_pm_tool`
//!   (survives even with product analytics off, and is what this command's
//!   `Result` actually reports success/failure for);
//! - a `pm_tool_requested` PostHog event via [`crate::analytics::send_pm_tool_requested`]
//!   (aggregate cross-fleet demand — same consent gate as every other event in
//!   [`crate::analytics`], silently skipped when signed out or opted out).
//!
//! # Who calls this
//! `ui/components/IntegrationConnect.tsx`'s `ConnectTrackers`, via
//! `ui/lib/bridge.ts`'s `mutate`.
//!
//! # Related
//! - [`crate::analytics`] — the PostHog capture half, and its consent rules.
//! - `meridian_core::settings` — the read/write-through-JSON pattern this
//!   mirrors (see `commands::account::write_account_pseudonym` for the sibling
//!   "one field, best-effort" write).

/// Persist `tool_name` locally and, best-effort, report it to PostHog.
///
/// Only the settings write is fallible from the caller's point of view — the
/// analytics half can never fail this command, matching every other capture
/// in [`crate::analytics`] (see its module doc's "Nothing is lost on
/// failure", which does not apply here: this is a one-off ask, not a rollup
/// with a retry cursor, so a failed send is simply not reported — the local
/// settings mirror is what's guaranteed).
/// Longest tool name accepted, after trimming.
///
/// This is an unbounded string arriving over IPC that gets persisted to
/// `settings.json` and forwarded to analytics, so it is a system boundary and
/// gets validated like one. Generous for the thing being named (a PM tool) and
/// small enough that a pasted document cannot bloat settings or the telemetry
/// payload. `IntegrationConnect.tsx`'s input carries the same `maxLength`, so
/// the UI stops it before this has to.
pub(crate) const MAX_TOOL_NAME_LEN: usize = 100;

// `tool_name` is deliberately NOT a span field (hence `skip`, not just
// `skip(app)`): it is free text the user typed, and every `tracing` call is
// captured at full fidelity into the local OTLP spool. The ship leg only
// egresses WARN+, so it would not leave the machine — but a diagnostics export
// bundle is unredacted and carries INFO, so it would travel there. The repo's
// rule is that attributes naming the user's own data stay off the record; the
// consented PostHog payload and the settings mirror are where this value
// legitimately lives.
#[tauri::command]
#[tracing::instrument(skip(app, tool_name))]
pub async fn request_pm_tool(app: tauri::AppHandle, tool_name: String) -> Result<(), String> {
    let tool_name = tool_name.trim().to_string();
    if tool_name.is_empty() {
        return Err("request_pm_tool: tool name is required".into());
    }
    // Count CHARS, not bytes: the message quotes a limit the user can only
    // reconcile against what they typed, and a byte cap would reject a shorter
    // non-ASCII name.
    if tool_name.chars().count() > MAX_TOOL_NAME_LEN {
        return Err(format!(
            "That name is too long - please keep it under {MAX_TOOL_NAME_LEN} characters."
        ));
    }

    let name = tool_name.clone();
    tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
        let mut v = meridian_core::settings::read_settings_value();
        // A settings root that is not an object (a corrupt file holding `[]`,
        // `null`, a bare scalar) used to skip the insert silently and then
        // report success - the user is told we recorded their request and
        // nothing was stored. Recover the root instead: the write below is
        // about to persist it either way, so leaving a non-object in place
        // would keep breaking every other settings writer too.
        if !v.is_object() {
            tracing::warn!("request_pm_tool: settings root was not an object - recovering it");
            v = serde_json::Value::Object(serde_json::Map::new());
        }
        if let Some(obj) = v.as_object_mut() {
            obj.insert(
                "requested_pm_tool".to_string(),
                serde_json::Value::String(name),
            );
        }
        meridian_core::settings::write_settings_value(&v)
    })
    .await
    .map_err(|e| format!("join settings write task: {e}"))?
    .map_err(|e| crate::cmd_err!(e, "request_pm_tool: settings write failed"))?;

    crate::analytics::send_pm_tool_requested(&app, &tool_name).await;
    tracing::info!("request_pm_tool: recorded");
    Ok(())
}
