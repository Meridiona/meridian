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

use tauri::{Emitter, State};

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
        .map_err(|e| crate::cmd_err!(e, notice_id, "delete_notice failed"))
}

/// Push a fresh `notices-update` NOW, unconditionally — the on-demand counterpart to
/// `crate::commands::health::push_health_update`, for the same reason: `poll::live::emit_notices`
/// only runs on the tray's own 30 s tick and only when its dedup snapshot changed, so a banner
/// this process just raised/cleared directly (see `update_settings`'s Groq sync) would
/// otherwise sit unrefreshed in the webview for up to another 30 s. Unlike `emit_notices` this
/// never checks a `last`-snapshot cache — it is called rarely (a settings write), never from
/// the hot poll loop, so an occasional duplicate identical push costs nothing.
///
/// Placed LAST in this file, not between two `#[tauri::command]`s: this has no `-> ReturnType`
/// (a plain `()` return), and `__tests__/mutate-body-contract.test.ts` parses this file's
/// commands with a regex that lazily scans for the next `) ->` — sitting between two commands
/// it would swallow the SECOND one's signature whole, exactly the failure mode
/// `push_health_update` (the same no-arrow shape) avoids by being last in `health.rs` too.
pub async fn push_notices_update(app: &tauri::AppHandle, pool: &meridian_core::SqlitePool) {
    let notices = meridian_core::notices::read_notices(pool).await;
    if let Err(e) = app.emit("notices-update", &notices) {
        tracing::warn!(error = %e, "notices update emit failed - banner stays stale until the next poll tick");
    }
}
