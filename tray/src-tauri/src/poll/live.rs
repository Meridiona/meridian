//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Live data → Tauri events: the push half of the ported SSE streams.
//!
//! The Next dashboard used Server-Sent Events for live notices, banner
//! notifications, and the log tail. The fold has no Node server, so the tray
//! emits Tauri events the webview listens to instead (`ui/lib/bridge.ts`'s
//! `subscribe` listens in the app, falls back to `EventSource` in a browser):
//!
//! - `notices-update`        ← [`emit_notices`]   (ported `/api/notices/stream`)
//! - `notifications-update`  ← [`emit_banners`]   (ported `/api/notifications/stream`)
//! - `log-tail`              ← [`spawn_log_tailer`] (ported `/api/logs/stream`)
//!
//! The DB-backed pair run on the poll-loop tick (30 s, matching the SSE's 30 s
//! coalesced poll) and emit **only when the set changes** — a JSON snapshot
//! compare, mirroring the SSE stores' change-only broadcast. `health-update` is
//! NOT here: it rides [`super::refresh::refresh_health`], which already owns the
//! health check.
//!
//! # Related
//! - [`crate::commands::notices`] / [`crate::commands::notifications`] — the
//!   matching `get_*` snapshot reads the webview primes with on first paint.
//! - [`crate::commands::logs`] — the log path + line parser the tailer reuses.

use std::io::{Read, Seek, SeekFrom};
use std::time::Duration;
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

/// Spawn the background log tailer: every second, stream NEW lines of the
/// current day's JSONL as a `log-tail` event (the ported `/api/logs/stream`).
///
/// It starts at end-of-file (history is primed via the `get_logs` snapshot, so
/// we only push lines written after the dashboard opened), follows the day
/// rollover (a fresh `…jsonl.<date>` file → read from its start), and resets on
/// truncation/rotation (file shorter than the offset). Only complete lines are
/// consumed — a partial trailing line waits for the next tick. Emits nothing
/// when idle; with no listeners Tauri simply drops the event.
pub(crate) fn spawn_log_tailer(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut current_path: Option<String> = None;
        let mut offset: u64 = 0;
        let mut interval = tokio::time::interval(Duration::from_secs(1));
        loop {
            interval.tick().await;
            let path = crate::commands::logs::today_log_path();

            // First run or day rollover: follow the new file from its end so we
            // don't replay history the snapshot read already showed.
            if current_path.as_deref() != Some(path.as_str()) {
                offset = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                current_path = Some(path);
                continue;
            }

            let Ok(meta) = std::fs::metadata(&path) else {
                continue; // file gone (between rollovers) — wait
            };
            let len = meta.len();
            if len < offset {
                offset = 0; // truncated or rotated in place
            }
            if len <= offset {
                continue; // no new bytes
            }

            let entries = read_new_lines(&path, &mut offset);
            if !entries.is_empty() {
                let _ = app.emit("log-tail", entries);
            }
        }
    });
}

/// Read complete new lines from `path` starting at `*offset`, parse them, and
/// advance `*offset` past the last consumed newline (a partial trailing line is
/// left for the next read). Best-effort: any IO/UTF-8 error yields no entries
/// and leaves the offset untouched, so the next tick retries.
fn read_new_lines(path: &str, offset: &mut u64) -> Vec<crate::commands::logs::LogEntry> {
    let Ok(mut file) = std::fs::File::open(path) else {
        return Vec::new();
    };
    if file.seek(SeekFrom::Start(*offset)).is_err() {
        return Vec::new();
    }
    let mut buf = String::new();
    if file.read_to_string(&mut buf).is_err() {
        return Vec::new();
    }
    // Consume only up to the last complete line; keep any partial tail for later.
    let consume_to = match buf.rfind('\n') {
        Some(i) => i + 1,
        None => return Vec::new(),
    };
    *offset += consume_to as u64;
    buf[..consume_to]
        .lines()
        .filter(|l| !l.is_empty())
        .filter_map(crate::commands::logs::parse_line)
        .collect()
}
