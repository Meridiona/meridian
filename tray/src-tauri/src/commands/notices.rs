//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Notices commands — the ported `/api/notices` surface.
//!
//! - [`get_notices`] — the snapshot read (ported `/api/notices/stream`'s query):
//!   the live fault-banner set, served on first paint and re-pushed by the poll
//!   loop's `notices-update` event.
//! - [`delete_notice`] — the DELETE: clear one banner immediately (the daemon
//!   would otherwise auto-clear it on the next healthy poll); the UI calls it the
//!   moment a provider reconnects.
//!
//! # Who calls this
//! Registered in `lib.rs`'s `invoke_handler!`; consumed by
//! `ui/components/NoticeBar.tsx` (`get_notices` via `bridge.subscribe`) and
//! `ui/components/views/TasksView.tsx` (`delete_notice`, a path-param route).
//!
//! # Related
//! - [`meridian_core::notices`] — the byte-for-byte ports
//!   ([`meridian_core::notices::read_notices`] / [`meridian_core::notices::delete_notice`]).
//! - [`crate::poll`] — emits the `notices-update` event off the same read.

use tauri::State;

/// The live notice set (the ported /api/notices/stream snapshot). No open DB →
/// empty (matches the route's `catch → []`), so the banner just shows nothing.
#[tauri::command]
#[tracing::instrument(skip(pool))]
pub async fn get_notices(
    pool: State<'_, Option<meridian_core::SqlitePool>>,
) -> Result<Vec<meridian_core::notices::Notice>, String> {
    let Some(pool) = pool.inner() else {
        return Ok(Vec::new());
    };
    let notices = meridian_core::notices::read_notices(pool).await;
    tracing::info!(count = notices.len(), "notices served");
    Ok(notices)
}

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
