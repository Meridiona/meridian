//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Live data → Tauri events: the push half of the ported SSE streams.
//!
//! The Next dashboard used Server-Sent Events for live notices and banner
//! notifications. The fold has no Node server, so the tray emits Tauri events
//! the webview listens to instead (`ui/lib/bridge.ts`'s `subscribe` listens in
//! the app, falls back to `EventSource` in a browser):
//!
//! - `notices-update`        ← [`emit_notices`]   (ported `/api/notices/stream`)
//! - `notifications-update`  ← [`emit_banners`]   (ported `/api/notifications/stream`)
//!
//! Both run on the poll-loop tick (30 s, matching the SSE's 30 s coalesced
//! poll) and emit **only when the set changes** — a JSON snapshot compare,
//! mirroring the SSE stores' change-only broadcast. `health-update` is NOT
//! here: it rides [`super::refresh::refresh_health`], which already owns the
//! health check.
//!
//! (There used to be a third stream here, `log-tail`, tailing a JSONL file for
//! a Logs UI — both the JSONL file and that UI are gone; see
//! `src/observability.rs`'s module doc and `meridian logs`/`telemetry_spool::render`
//! for the OTel-spool-backed replacement.)
//!
//! # Related
//! - [`crate::commands::notices`] / [`crate::commands::notifications`] — the
//!   matching `get_*` snapshot reads the webview primes with on first paint.

use tauri::Emitter;

/// Read the notice set and emit `notices-update` only if it changed since the
/// last tick. `last` holds the previous JSON snapshot (empty on first call).
pub(super) async fn emit_notices(
    app: &tauri::AppHandle,
    pool: &meridian_core::SqlitePool,
    last: &mut String,
) {
    let notices = meridian_core::notices::read_notices(pool).await;
    let snapshot = serde_json::to_string(&notices).unwrap_or_default();
    if snapshot == *last {
        return;
    }
    *last = snapshot;
    let _ = app.emit("notices-update", notices);
}

/// Read the active banner set and emit `notifications-update` only if it
/// changed. Resolves `now` + prefs here, matching the `get_banner_notifications`
/// command (the SSE compared snapshots the same way).
pub(super) async fn emit_banners(
    app: &tauri::AppHandle,
    pool: &meridian_core::SqlitePool,
    last: &mut String,
) {
    let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let settings = meridian_core::settings::load_runtime_settings();
    let banners = meridian_core::notifications::active_banners(pool, &now, &settings).await;
    let snapshot = serde_json::to_string(&banners).unwrap_or_default();
    if snapshot == *last {
        return;
    }
    *last = snapshot;
    let _ = app.emit("notifications-update", banners);
}
