//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Product analytics — PostHog Cloud event capture.
//!
//! # What this is
//! A deliberately tiny, best-effort telemetry client: two anonymous signals —
//! `app_installed` once per machine, and one `daily_usage` event per completed
//! local calendar day (focus/coding hours, logged hours, and drafts count —
//! the same four numbers as the dashboard's Today card). Nothing else: a
//! single raw HTTP POST to PostHog's `/i/v0/e/`
//! capture endpoint, no `posthog-js`, so session replay / autocapture /
//! surveys / feature flags never activate — that's client-SDK behaviour this
//! code never touches. `$geoip_disable` is set on every event since the
//! request originates from the user's own machine, not a backend relay (the
//! default GeoIP enrichment would otherwise resolve each user's real
//! location).
//!
//! Runs in every install shape — a source/dev checkout, a bare launch, and a
//! packaged DMG alike — so a local `cargo run`/`tauri dev` session shows up
//! too. Every event carries `channel` ([`crate::commands::version::build_channel`]:
//! `dev`/`staging`/`prod`) so dev-run noise stays filterable in PostHog from
//! real installs, rather than being excluded outright. The anonymous
//! `distinct_id` plus day bookkeeping live in `~/.meridian/analytics_state.json`,
//! separate from `settings.json` (never dashboard-editable, never displayed).
//!
//! Call volume is intentionally minimal: at most 1 event ever
//! (`app_installed`) + at most 1 event per calendar day (`daily_usage`),
//! regardless of how many times the tray restarts or how often the poll loop
//! ticks — nowhere near PostHog's free-tier event limits.
//!
//! **Not backfilled**: only the single most-recently-closed day is ever
//! reported (see [`day_rollover_action`]). If the tray isn't running across a
//! local-day boundary for several days, the intervening day(s) are skipped
//! entirely — never sent late.
//!
//! **Nothing is lost on failure.** [`capture`] reports success/failure via
//! its return value, and both call sites ([`maybe_send_daily_tick`],
//! [`send_daily_usage`]) only flip their persisted "sent" bookkeeping
//! (`install_event_sent` / `last_sent_day`) on a confirmed success. A DB read
//! failure, a network error, a timeout, or a non-2xx from PostHog all leave
//! that bookkeeping untouched, so the *exact same* pending event (the same
//! day, carrying that day's own `date` property — never today's) is retried
//! on every subsequent ~60 s tick until it lands, surviving a tray restart in
//! between since the bookkeeping is on disk. Never fabricates zeros for a
//! read that failed.
//!
//! # Who calls this
//! [`crate::poll::run_poll_loop`]'s health tick (`maybe_send_daily_tick`) — a
//! plain file read cheap enough to run every 60 s; the (at most) two HTTP
//! calls it can trigger are individually capped as described above.
//!
//! # Related
//! - [`crate::commands::version::build_channel`] — the `channel` property
//!   every event carries (dev/staging/prod).
//! - `meridian_core::today::get_today` / `meridian_core::worklogs::get_worklogs`
//!   — the same readers the dashboard uses, queried here for an already-closed
//!   past day (never "today", which is still accumulating).

use meridian_core::SqlitePool;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tracing::Instrument;

/// Single-flight guard for [`maybe_send_daily_tick`]. The poll loop spawns a
/// fresh call every ~60 s tick without waiting for the previous one to
/// finish (see `poll::run_poll_loop`'s doc comment on why it's spawned off);
/// under normal conditions each call completes in well under a second, but
/// nothing structurally prevents two calls overlapping (a slow DNS/TLS
/// handshake, GC pause, etc). Without this guard, two overlapping calls could
/// both read the same stale on-disk `last_sent_day`, both decide the same
/// day just closed, and both send a duplicate `daily_usage` for it — this
/// flag makes that impossible: only one call is ever actually running.
static TICK_IN_FLIGHT: AtomicBool = AtomicBool::new(false);

/// RAII guard that flips [`TICK_IN_FLIGHT`] back to `false` on drop, so every
/// return path out of `maybe_send_daily_tick` (including the early ones)
/// releases it — a plain `if`/`return` without this would leave the flag
/// stuck `true` forever after the first early exit.
struct InFlightGuard;
impl Drop for InFlightGuard {
    fn drop(&mut self) {
        TICK_IN_FLIGHT.store(false, Ordering::SeqCst);
    }
}

/// PostHog Cloud (US region) project token — a **public** write-only capture
/// key (safe to embed in a shipped client, unlike a personal/secret API key;
/// see the PR #427 discussion). **Baked in at compile time**, never committed
/// to source: the official release build injects it via the
/// `MERIDIAN_POSTHOG_API_KEY` build env (a GitHub Actions secret — see
/// `.github/workflows/release.yml` / `release-staging.yml`), mirroring
/// `meridian-oauth`'s `DEFAULT_CLIENT_SECRET` pattern. A plain source build
/// without that env compiles in an empty string, which disables analytics
/// entirely (see [`posthog_api_key`]) rather than shipping a placeholder.
const DEFAULT_POSTHOG_API_KEY: &str = match option_env!("MERIDIAN_POSTHOG_API_KEY") {
    Some(k) => k,
    None => "",
};
/// US Cloud ingestion host — matches the project's region.
const POSTHOG_HOST: &str = "https://us.i.posthog.com";

/// Resolve the capture key: the `POSTHOG_API_KEY` runtime env override (e.g.
/// set in `~/.meridian/.env` for a source/dev build — the daemon/tray already
/// load that file via `dotenvy`) if set and non-blank, else the compiled-in
/// [`DEFAULT_POSTHOG_API_KEY`].
fn posthog_api_key() -> String {
    std::env::var("POSTHOG_API_KEY")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_POSTHOG_API_KEY.to_string())
}

/// Persisted analytics bookkeeping. Deliberately its own file — never merged
/// into `settings.json` (which the dashboard reads/writes/displays).
#[derive(Debug, Clone, Serialize, Deserialize)]
struct AnalyticsState {
    distinct_id: String,
    install_event_sent: bool,
    /// The last LOCAL calendar day ("YYYY-MM-DD") a `daily_usage` event was
    /// sent for. `None` before the first day boundary has been observed.
    last_sent_day: Option<String>,
}

fn analytics_state_path() -> Option<PathBuf> {
    meridian_core::paths::home_dir().map(|h| h.join(".meridian/analytics_state.json"))
}

/// Load the state file, creating a fresh one (new random `distinct_id`) if
/// absent or unparseable. Never errors — a corrupt/missing file just starts a
/// new anonymous id, same as a genuine first run.
fn load_or_init_state(path: &std::path::Path) -> AnalyticsState {
    if let Ok(s) = std::fs::read_to_string(path) {
        if let Ok(state) = serde_json::from_str::<AnalyticsState>(&s) {
            return state;
        }
    }
    AnalyticsState {
        distinct_id: uuid::Uuid::new_v4().to_string(),
        install_event_sent: false,
        last_sent_day: None,
    }
}

/// Crash-safely persist the state file via the shared atomic-write helper
/// ([`meridian_core::fs_utils::atomic_write_json`]) — best-effort, logs on
/// failure like the rest of this module.
fn save_state(path: &std::path::Path, state: &AnalyticsState) {
    if let Err(e) = meridian_core::fs_utils::atomic_write_json(path, state) {
        tracing::warn!(error = %e, "analytics: could not persist state file");
    }
}

/// POST one event to PostHog's raw capture endpoint (no SDK — see module
/// docs). Never panics or propagates an error — analytics must never affect
/// the app's behaviour or startup — but DOES report success/failure via its
/// return value so callers with a "send at most once" bookkeeping flag (the
/// `install_event_sent`/`last_sent_day` fields in [`AnalyticsState`]) know
/// not to advance that bookkeeping on a failed send. Returns `true` only on
/// an HTTP 2xx from PostHog; `false` on a missing API key, a request/timeout
/// error, or a non-2xx response — in every `false` case the caller must
/// retry, never silently drop the event.
async fn capture(
    event: &str,
    distinct_id: &str,
    mut properties: serde_json::Map<String, serde_json::Value>,
) -> bool {
    let api_key = posthog_api_key();
    if api_key.is_empty() {
        // No compiled-in secret and no runtime override — a source build
        // without MERIDIAN_POSTHOG_API_KEY set. Skip silently rather than
        // POST an unauthenticated request PostHog will just reject. Treated
        // as a failure so the caller retries once a key becomes available
        // (e.g. after a rebuild) instead of losing the event forever.
        tracing::debug!(
            event,
            "analytics: no PostHog key configured — skipping capture"
        );
        return false;
    }
    properties.insert("$geoip_disable".to_string(), serde_json::Value::Bool(true));
    let body = serde_json::json!({
        "api_key": api_key,
        "event": event,
        "distinct_id": distinct_id,
        "properties": properties,
    });
    let result = reqwest::Client::new()
        .post(format!("{POSTHOG_HOST}/i/v0/e/"))
        .json(&body)
        .timeout(Duration::from_secs(5))
        .send()
        .instrument(tracing::debug_span!("analytics.capture", event))
        .await;
    match result {
        Ok(resp) if resp.status().is_success() => {
            tracing::debug!(event, "analytics: event captured");
            true
        }
        Ok(resp) => {
            tracing::warn!(event, status = %resp.status(), "analytics: capture rejected — will retry");
            false
        }
        Err(e) => {
            tracing::warn!(event, error = %e, "analytics: capture request failed — will retry");
            false
        }
    }
}

/// Properties common to every event: app version + OS + build channel
/// ([`crate::commands::version::build_channel`] — `dev`/`staging`/`prod`, so
/// a local `cargo run`/`tauri dev` session stays distinguishable in PostHog
/// from a real install rather than being excluded outright). When the setup
/// wizard's Clerk sign-in step has captured an email
/// (`crate::commands::account::read_account_email`), it rides along as a
/// PostHog `$set` so the person profile is upgraded from anonymous to
/// identified — same anonymous `distinct_id`, just no longer nameless.
fn base_properties(app: &tauri::AppHandle) -> serde_json::Map<String, serde_json::Value> {
    let mut m = serde_json::Map::new();
    m.insert(
        "app_version".to_string(),
        serde_json::Value::String(app.package_info().version.to_string()),
    );
    m.insert(
        "os".to_string(),
        serde_json::Value::String(std::env::consts::OS.to_string()),
    );
    m.insert(
        "channel".to_string(),
        serde_json::Value::String(crate::commands::version::build_channel().to_string()),
    );
    if let Some(email) = crate::commands::account::read_account_email() {
        let mut set = serde_json::Map::new();
        set.insert("email".to_string(), serde_json::Value::String(email));
        m.insert("$set".to_string(), serde_json::Value::Object(set));
    }
    m
}

/// Called once per poll-loop health tick (~60 s — see [`crate::poll`]). A
/// cheap file read on every call; the two possible HTTP calls are
/// individually capped to once-ever (`app_installed`) and once-per-completed
/// local day (`daily_usage`) — but only once each has actually *succeeded*.
/// Whenever `capture()` fails (no key, timeout, non-2xx — see its doc), the
/// corresponding bookkeeping field is left untouched, so the very next tick
/// (and every tick after that, ~60 s apart) retries the exact same pending
/// event — never a fabricated/skipped one — until it lands. That bookkeeping
/// is persisted to `analytics_state.json`, so the retry survives a tray
/// restart too: a `daily_usage` that failed to send stays pending even if
/// the tray is closed and reopened the "next day morning", and is sent for
/// its original (correct) day as soon as the app is next open and reachable.
/// Runs in every install shape (see the module doc) — filter by the
/// `channel` property in PostHog to separate dev runs from real installs.
#[tracing::instrument(skip(app, pool))]
pub(crate) async fn maybe_send_daily_tick(app: &tauri::AppHandle, pool: &SqlitePool) {
    if TICK_IN_FLIGHT.swap(true, Ordering::SeqCst) {
        // A previous tick's spawned call is still running — see
        // [`TICK_IN_FLIGHT`]'s doc for why this must never run concurrently.
        tracing::debug!("analytics: previous tick still in flight — skipping");
        return;
    }
    let _guard = InFlightGuard;

    let Some(path) = analytics_state_path() else {
        return;
    };
    let mut state = load_or_init_state(&path);
    let mut dirty = false;

    if !state.install_event_sent
        && capture("app_installed", &state.distinct_id, base_properties(app)).await
    {
        state.install_event_sent = true;
        dirty = true;
    }

    let today = meridian_core::date::today_string();
    match day_rollover_action(state.last_sent_day.as_deref(), &today) {
        Some(None) => {
            // First observation this install has made — nothing has closed
            // yet, so there's nothing to report.
            state.last_sent_day = Some(today);
            dirty = true;
        }
        Some(Some(day)) => {
            // Only advance past `day` when it actually sent — a transient DB
            // read hiccup must retry next tick, not silently report (and
            // permanently skip) a fabricated zero-usage day.
            if send_daily_usage(app, pool, &state.distinct_id, &day).await {
                state.last_sent_day = Some(today);
                dirty = true;
            }
        }
        None => {}
    }

    if dirty {
        save_state(&path, &state);
    }
}

/// Pure day-rollover decision, split out from [`maybe_send_daily_tick`] so it
/// can be unit-tested without a live DB/HTTP client (see tests below).
/// `None` → nothing to do this tick. `Some(None)` → arm the first-observed
/// day with no send (nothing has closed yet). `Some(Some(day))` → `day` just
/// closed; send its usage, then the caller advances past it on success.
///
/// NOTE: only the single most-recently-closed day is ever reported. If the
/// tray isn't running across more than one local-day boundary (closed, then
/// reopened days later), the intervening day(s) are skipped, not backfilled —
/// an accepted "today/yesterday-only, no backfill" simplification consistent
/// with the rest of the daemon's daily-cadence jobs.
fn day_rollover_action(last_sent_day: Option<&str>, today: &str) -> Option<Option<String>> {
    match last_sent_day {
        None => Some(None),
        Some(prev) if prev != today => Some(Some(prev.to_string())),
        _ => None,
    }
}

/// Build + send the `daily_usage` event for an already-completed local day —
/// the same four numbers as the dashboard's Today card (Focus/Coding/Logged/
/// Drafts, see `ui/components/timeline/OverviewPanel.tsx`), via the same
/// readers the dashboard uses ([`meridian_core::today::get_today`],
/// [`meridian_core::worklogs::get_worklogs`]). Queried at the day's own
/// closing instant (its local-day upper bound) so a past day's active-session
/// edge case can't leak into the total.
///
/// Returns `false` on a DB read failure (WITHOUT sending anything — never
/// fabricates zeros for a real error, a genuinely idle day's zeros come only
/// from a successful read) OR when the resulting `capture()` HTTP POST
/// itself fails/times out/is rejected. Either way the caller must not
/// advance `last_sent_day`, so the exact same day is retried next tick
/// instead of being lost.
async fn send_daily_usage(
    app: &tauri::AppHandle,
    pool: &SqlitePool,
    distinct_id: &str,
    day: &str,
) -> bool {
    let (_, day_end) = meridian_core::date::local_day_bounds(day);
    let today = match meridian_core::today::get_today(pool, day, &day_end).await {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!(error = %e, day, "analytics: today read failed for daily_usage — will retry");
            return false;
        }
    };
    // Mirrors OverviewPanel.tsx's "Focus" tile exactly (focus + autonomous
    // agent time), not the raw `focus_s` field.
    let focus_s = today.engaged_s;
    // Mirrors TimeByCategory.tsx's coding-category fold: session time
    // classified `cat == "coding"` plus coding-agent tool time (`agent_s`).
    let coding_s: i64 = today
        .sessions
        .iter()
        .filter(|s| s.cat == "coding")
        .map(|s| s.dur)
        .sum::<i64>()
        + today.agent_s;

    let worklogs = match meridian_core::worklogs::get_worklogs(pool, day).await {
        Ok(w) => w,
        Err(e) => {
            tracing::warn!(error = %e, day, "analytics: worklogs read failed for daily_usage — will retry");
            return false;
        }
    };
    // Mirrors OverviewPanel.tsx's "Logged" tile: real (non-proposed) worklogs
    // that are approved or posted, summed by their actual time spent.
    let logged_s: i64 = worklogs
        .items
        .iter()
        .filter(|i| !i.is_proposed && (i.state == "approved" || i.state == "posted"))
        .map(|i| i.time_spent_seconds)
        .sum();
    // Mirrors OverviewPanel.tsx's "Drafts" tile (`isPending`, types.ts):
    // proposed tickets still `proposed`, or real worklogs still `drafted`.
    let drafts_count = worklogs
        .items
        .iter()
        .filter(|i| {
            if i.is_proposed {
                i.state == "proposed"
            } else {
                i.state == "drafted"
            }
        })
        .count();

    let hours = |s: i64| (s.max(0) as f64 / 3600.0 * 100.0).round() / 100.0;
    let mut props = base_properties(app);
    props.insert(
        "date".to_string(),
        serde_json::Value::String(day.to_string()),
    );
    props.insert("focus_hours".to_string(), serde_json::json!(hours(focus_s)));
    props.insert(
        "coding_hours".to_string(),
        serde_json::json!(hours(coding_s)),
    );
    props.insert(
        "logged_hours".to_string(),
        serde_json::json!(hours(logged_s)),
    );
    props.insert("drafts_count".to_string(), serde_json::json!(drafts_count));

    tracing::info!(
        day,
        focus_s,
        coding_s,
        logged_s,
        drafts_count,
        "analytics: sending daily_usage"
    );
    capture("daily_usage", distinct_id, props).await
}

#[cfg(test)]
mod day_rollover_tests {
    use super::day_rollover_action;

    #[test]
    fn first_observation_arms_without_sending() {
        assert_eq!(day_rollover_action(None, "2026-07-09"), Some(None));
    }

    #[test]
    fn same_day_is_a_no_op() {
        assert_eq!(day_rollover_action(Some("2026-07-09"), "2026-07-09"), None);
    }

    #[test]
    fn day_boundary_reports_the_closed_day() {
        assert_eq!(
            day_rollover_action(Some("2026-07-08"), "2026-07-09"),
            Some(Some("2026-07-08".to_string()))
        );
    }

    #[test]
    fn multi_day_gap_reports_only_the_last_closed_day() {
        // A gap of several days (tray closed then reopened) still reports
        // only the most recent `last_sent_day` — intervening days are never
        // backfilled (see the doc comment on `day_rollover_action`).
        assert_eq!(
            day_rollover_action(Some("2026-07-01"), "2026-07-09"),
            Some(Some("2026-07-01".to_string()))
        );
    }
}
