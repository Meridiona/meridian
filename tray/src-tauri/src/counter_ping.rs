//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Public "worklog/task update" counter — a single fire-and-forget ping to the
//! meridiona.com website's live counter, incremented once per confirmed
//! worklog post or personal-task create/update, powering the "N updates
//! logged and counting" stat on the landing page.
//!
//! # What this is
//! A single POST to `{COUNTER_HOST}/api/counter/increment`, no payload beyond
//! auth — the website only needs to know "one more happened", not who or
//! what. Gated to `build_channel() == "prod"` so dev/staging runs never
//! inflate the public number (mirrors how `analytics.rs` tags every event
//! with a `channel` property instead — but this endpoint has no room for a
//! property, so non-prod channels are filtered out entirely rather than sent
//! and later excluded by a dashboard filter).
//!
//! Two call shapes:
//! - **Personal task create/update** — a direct, one-shot [`ping_task_update`]
//!   call from the `#[tauri::command]` wrappers in `commands::plan_tasks`,
//!   right after `run_meridian_json` confirms success. A single command
//!   invocation IS the event; no dedup needed.
//! - **Worklog posted to a PM provider** — the state transition happens in
//!   the daemon process (`src/pm_worklog/post.rs`), a separate OS process the
//!   tray has no direct hook into. [`check_worklog_posts`] instead diffs
//!   today's worklogs (already read every poll tick by
//!   `poll::refresh::refresh_worklogs`) against a persisted set of
//!   already-pinged ids, so a newly `posted` row is caught within one ~60s
//!   tick without the daemon needing any analytics wiring of its own.
//!
//! **Best-effort, not exactly-once.** Unlike `analytics.rs`'s retry-until-success
//! bookkeeping, a failed [`ping_task_update`] is simply dropped. The
//! worklog-diff path is slightly sturdier: an id is only added to the pinged
//! set on a successful ping, so a network blip retries it next tick. Either
//! way, a public vanity counter occasionally missing an increment is an
//! accepted tradeoff — it doesn't justify `daily_usage`-grade retry machinery.
//!
//! # Who calls this
//! - `commands::plan_tasks::{create_plan_task, edit_plan_task, set_plan_task_done}`
//! - `poll::mod` (via [`check_worklog_posts`], right after `refresh_worklogs`)
//!
//! # Related
//! - [`crate::analytics`] — the sibling PostHog pipeline; same channel-gating
//!   idea, different endpoint and no per-event identity.
//! - `crate::commands::version::build_channel`

use meridian_core::SqlitePool;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// The live counter's public host — the meridiona.com marketing site.
const COUNTER_HOST: &str = "https://meridiona.com";

/// Compiled-in shared secret for the increment endpoint (mirrors
/// `analytics::DEFAULT_POSTHOG_API_KEY` — a build-time secret via
/// `MERIDIAN_COUNTER_API_KEY`, empty in a plain source build, which disables
/// the ping entirely rather than sending an unauthenticated request the
/// website would reject).
const DEFAULT_COUNTER_API_KEY: &str = match option_env!("MERIDIAN_COUNTER_API_KEY") {
    Some(k) => k,
    None => "",
};

/// Resolve the shared secret: the `COUNTER_API_KEY` runtime env override
/// (e.g. set in `~/.meridian/.env` for a source/dev build) if set and
/// non-blank, else the compiled-in [`DEFAULT_COUNTER_API_KEY`].
fn counter_api_key() -> String {
    std::env::var("COUNTER_API_KEY")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_COUNTER_API_KEY.to_string())
}

/// POST one increment to the public counter. Returns `true` (a no-op success,
/// so one-shot callers never treat it as a failure worth retrying) on any
/// non-`prod` channel or a missing key; a real network/non-2xx failure
/// returns `false`.
async fn ping_increment(reason: &str) -> bool {
    if crate::commands::version::build_channel() != "prod" {
        tracing::debug!(reason, "counter_ping: non-prod channel — skipping");
        return true;
    }
    let key = counter_api_key();
    if key.is_empty() {
        tracing::debug!(reason, "counter_ping: no counter key configured — skipping");
        return true;
    }
    let result = reqwest::Client::new()
        .post(format!("{COUNTER_HOST}/api/counter/increment"))
        .bearer_auth(key)
        .timeout(Duration::from_secs(5))
        .send()
        .await;
    match result {
        Ok(resp) if resp.status().is_success() => {
            tracing::debug!(reason, "counter_ping: incremented");
            true
        }
        Ok(resp) => {
            tracing::warn!(reason, status = %resp.status(), "counter_ping: rejected");
            false
        }
        Err(e) => {
            tracing::warn!(reason, error = %e, "counter_ping: request failed");
            false
        }
    }
}

/// One-shot ping for a personal-task create/update, called right after the
/// owning `#[tauri::command]` confirms success. Callers spawn this via
/// `tauri::async_runtime::spawn` rather than awaiting it inline, so a slow
/// counter response never delays the command's response to the UI.
pub(crate) async fn ping_task_update() {
    ping_increment("task_update").await;
}

/// Persisted dedup state for [`check_worklog_posts`] — a plain per-day set of
/// already-pinged `pm_worklogs.id` values, so a tray restart mid-day doesn't
/// re-ping rows it already reported. Reset whenever the local day rolls over,
/// since `refresh_worklogs` (and this check) only ever reads today's rows.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct CounterPingState {
    day: String,
    #[serde(default)]
    pinged_worklog_ids: HashSet<i64>,
}

fn counter_ping_state_path() -> Option<PathBuf> {
    meridian_core::paths::home_dir().map(|h| h.join(".meridian/counter_ping_state.json"))
}

/// Load the state file, starting a fresh (empty) set if absent, unparseable,
/// or stamped for a previous day.
fn load_state(path: &std::path::Path, today: &str) -> CounterPingState {
    if let Ok(s) = std::fs::read_to_string(path) {
        if let Ok(state) = serde_json::from_str::<CounterPingState>(&s) {
            if state.day == today {
                return state;
            }
        }
    }
    CounterPingState {
        day: today.to_string(),
        pinged_worklog_ids: HashSet::new(),
    }
}

fn save_state(path: &std::path::Path, state: &CounterPingState) {
    if let Err(e) = meridian_core::fs_utils::atomic_write_json(path, state) {
        tracing::warn!(error = %e, "counter_ping: could not persist state file");
    }
}

/// Single-flight guard for [`check_worklog_posts`]. Each poll tick *spawns* the
/// check without awaiting it (a slow counter response must not stall the loop),
/// so on a burst of newly-posted rows — each ping awaits up to 5s — one run can
/// still be in flight when the next `do_worklogs` tick fires. Two overlapping
/// runs would load the same on-disk dedup set, both see the same not-yet-pinged
/// ids, and double-increment the public counter before either writes the set
/// back. This flag makes the later run skip instead.
static WORKLOG_CHECK_RUNNING: AtomicBool = AtomicBool::new(false);

/// Clears [`WORKLOG_CHECK_RUNNING`] on drop, so the flag is released even if the
/// check returns early or panics.
struct RunningGuard;
impl Drop for RunningGuard {
    fn drop(&mut self) {
        WORKLOG_CHECK_RUNNING.store(false, Ordering::Release);
    }
}

/// Diff today's worklogs against the persisted pinged-id set and ping once
/// per newly `posted` row. Called every poll tick, right after
/// `poll::refresh::refresh_worklogs` reads the same list.
#[tracing::instrument(skip(pool))]
pub(crate) async fn check_worklog_posts(pool: &SqlitePool) {
    // Skip if a prior tick's check is still running (see WORKLOG_CHECK_RUNNING),
    // so overlapping runs can't double-count the same worklog ids.
    if WORKLOG_CHECK_RUNNING
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        tracing::debug!("counter_ping: worklog check already in flight — skipping this tick");
        return;
    }
    let _running = RunningGuard;

    let Some(path) = counter_ping_state_path() else {
        return;
    };
    let today = meridian_core::date::today_string();
    let mut state = load_state(&path, &today);

    let worklogs = match meridian_core::worklogs::get_worklogs(pool, &today).await {
        Ok(w) => w,
        Err(e) => {
            tracing::warn!(error = %e, "counter_ping: worklogs read failed");
            return;
        }
    };

    let newly_posted: Vec<i64> = worklogs
        .items
        .iter()
        .filter(|i| i.state == "posted" && !state.pinged_worklog_ids.contains(&i.id))
        .map(|i| i.id)
        .collect();

    let mut dirty = false;
    for id in newly_posted {
        if ping_increment("worklog_posted").await {
            state.pinged_worklog_ids.insert(id);
            dirty = true;
        }
    }

    if dirty {
        save_state(&path, &state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A state file written for a previous day must not leak its pinged ids
    /// into today — otherwise a worklog posted today that happens to reuse an
    /// id pinged on a prior day (or, more realistically, a stale file left
    /// behind by a bug) would be silently skipped.
    #[test]
    fn load_state_resets_on_day_change() {
        let path = std::env::temp_dir().join(format!(
            "meridian_counter_ping_test_{}.json",
            std::process::id()
        ));
        let stale = CounterPingState {
            day: "2026-01-01".to_string(),
            pinged_worklog_ids: HashSet::from([1, 2, 3]),
        };
        std::fs::write(&path, serde_json::to_string(&stale).unwrap()).unwrap();

        let state = load_state(&path, "2026-01-02");

        assert_eq!(state.day, "2026-01-02");
        assert!(state.pinged_worklog_ids.is_empty());

        let _ = std::fs::remove_file(&path);
    }

    /// Same-day reload must preserve the already-pinged ids — otherwise a
    /// tray restart mid-day would re-ping every worklog posted before it.
    #[test]
    fn load_state_preserves_same_day_ids() {
        let path = std::env::temp_dir().join(format!(
            "meridian_counter_ping_test_same_{}.json",
            std::process::id()
        ));
        let today = CounterPingState {
            day: "2026-01-02".to_string(),
            pinged_worklog_ids: HashSet::from([42]),
        };
        std::fs::write(&path, serde_json::to_string(&today).unwrap()).unwrap();

        let state = load_state(&path, "2026-01-02");

        assert_eq!(state.day, "2026-01-02");
        assert!(state.pinged_worklog_ids.contains(&42));

        let _ = std::fs::remove_file(&path);
    }
}
