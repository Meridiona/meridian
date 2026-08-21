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
#[tauri::command]
#[tracing::instrument(skip(app))]
pub async fn request_pm_tool(app: tauri::AppHandle, tool_name: String) -> Result<(), String> {
    let tool_name = tool_name.trim().to_string();
    if tool_name.is_empty() {
        return Err("request_pm_tool: tool name is required".into());
    }

    let name = tool_name.clone();
    tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
        let mut v = meridian_core::settings::read_settings_value();
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
