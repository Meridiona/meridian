//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Notices command — the ported `/api/notices/[id]` DELETE.
//!
//! Clears a fault banner from `system_notices` immediately (the daemon would
//! otherwise auto-clear it on the next healthy poll). The UI calls this the
//! moment a provider reconnects so the banner disappears at once.
//!
//! # Who calls this
//! Registered in `lib.rs`'s `invoke_handler!`; consumed by
//! `ui/components/views/TasksView.tsx` on a successful provider connect
//! (dual-path `invoke` / `/api` DELETE — a path-param route, no JSON body).
//!
//! # Related
//! - [`meridian_core::notices::delete_notice`] — the byte-for-byte port.

use tauri::State;

/// Clear one notice by `notice_id` (the ported /api/notices/[id] DELETE).
/// Idempotent — clearing an absent notice is a no-op.
#[tauri::command]
#[tracing::instrument(skip(pool))]
pub async fn delete_notice(
    pool: State<'_, Option<meridian_core::SqlitePool>>,
    notice_id: String,
) -> Result<(), String> {
    let Some(pool) = pool.inner() else {
        return Err("meridian.db is not open yet".to_string());
    };
    meridian_core::notices::delete_notice(pool, &notice_id)
        .await
        .map_err(|e| {
            tracing::warn!(error = %e, notice_id, "delete_notice failed");
            e.to_string()
        })
}
