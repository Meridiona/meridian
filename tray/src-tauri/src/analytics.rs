//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Product analytics — PostHog Cloud event capture for the packaged DMG build.
//!
//! # What this is
//! A deliberately tiny, best-effort telemetry client: two anonymous signals —
//! `app_installed` once per machine, and one `daily_usage` event per completed
//! local calendar day (focus hours + worklog generated/approved/rejected
//! counts). Nothing else: a single raw HTTP POST to PostHog's `/i/v0/e/`
//! capture endpoint, no `posthog-js`, so session replay / autocapture /
//! surveys / feature flags never activate — that's client-SDK behaviour this
//! code never touches. `$geoip_disable` is set on every event since the
//! request originates from the user's own machine, not a backend relay (the
//! default GeoIP enrichment would otherwise resolve each user's real
//! location).
//!
//! Gated to [`crate::install::InstallMode::Canonical`] (the DMG install) only
//! — npm-bundle, source/dev, and bare runs never send anything. The anonymous
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
//! entirely — never sent late. A DB read failure on the day that WOULD be
//! reported is retried on the next tick instead of being sent as fabricated
//! zeros.
//!
//! # Who calls this
//! [`crate::poll::run_poll_loop`]'s health tick (`maybe_send_daily_tick`) — a
//! plain file read cheap enough to run every 60 s; the (at most) two HTTP
//! calls it can trigger are individually capped as described above.
//!
//! # Related
//! - [`crate::install::detect_install_mode`] — the Canonical/Dev/Bare gate.
//! - `meridian_core::today::get_today` / `meridian_core::worklogs::get_worklogs`
//!   — the same readers the dashboard uses, queried here for an already-closed
//!   past day (never "today", which is still accumulating).

use meridian_core::SqlitePool;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;
use tracing::Instrument;

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
    std::env::var("HOME")
        .ok()
        .map(|h| PathBuf::from(h).join(".meridian/analytics_state.json"))
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

/// Crash-safely persist the state file (temp + rename), matching the pattern
/// `meridian_core::settings::write_settings_value` uses.
fn save_state(path: &std::path::Path, state: &AnalyticsState) {
    let Some(dir) = path.parent() else { return };
    if let Err(e) = std::fs::create_dir_all(dir) {
        tracing::warn!(error = %e, "analytics: could not create state dir");
        return;
    }
    let json = match serde_json::to_string_pretty(state) {
        Ok(j) => j,
        Err(e) => {
            tracing::warn!(error = %e, "analytics: could not serialise state");
            return;
        }
    };
    let tmp = path.with_extension("json.tmp");
    if let Err(e) = std::fs::write(&tmp, json.as_bytes()) {
        tracing::warn!(error = %e, "analytics: could not write temp state file");
        return;
    }
    if let Err(e) = std::fs::rename(&tmp, path) {
        tracing::warn!(error = %e, "analytics: could not replace state file");
    }
}

/// POST one event to PostHog's raw capture endpoint (no SDK — see module
/// docs). Best-effort: logs and returns on any failure, never propagates —
/// analytics must never affect the app's behaviour or startup.
async fn capture(
    event: &str,
    distinct_id: &str,
    mut properties: serde_json::Map<String, serde_json::Value>,
) {
    let api_key = posthog_api_key();
    if api_key.is_empty() {
        // No compiled-in secret and no runtime override — a source build
        // without MERIDIAN_POSTHOG_API_KEY set. Skip silently rather than
        // POST an unauthenticated request PostHog will just reject.
        tracing::debug!(
            event,
            "analytics: no PostHog key configured — skipping capture"
        );
        return;
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
            tracing::debug!(event, "analytics: event captured")
        }
        Ok(resp) => tracing::warn!(event, status = %resp.status(), "analytics: capture rejected"),
        Err(e) => tracing::warn!(event, error = %e, "analytics: capture request failed"),
    }
}

/// Properties common to every event: app version + OS.
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
    m
}

/// Called once per poll-loop health tick (~60 s — see [`crate::poll`]). A
/// cheap file read on every call; the two possible HTTP calls are
/// individually capped to once-ever (`app_installed`) and once-per-completed
/// local day (`daily_usage`). No-ops entirely outside a Canonical (DMG)
/// install.
#[tracing::instrument(skip(app, pool))]
pub(crate) async fn maybe_send_daily_tick(app: &tauri::AppHandle, pool: &SqlitePool) {
    if !matches!(
        crate::install::detect_install_mode(),
        crate::install::InstallMode::Canonical(_)
    ) {
        return;
    }
    let Some(path) = analytics_state_path() else {
        return;
    };
    let mut state = load_or_init_state(&path);
    let mut dirty = false;

    if !state.install_event_sent {
        capture("app_installed", &state.distinct_id, base_properties(app)).await;
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

/// Build + send the `daily_usage` event for an already-completed local day —
/// focus hours plus worklog generated/approved/rejected counts, via the same
/// readers the dashboard uses ([`meridian_core::today::get_today`],
/// [`meridian_core::worklogs::get_worklogs`]). Queried at the day's own
/// closing instant (its local-day upper bound) so a past day's active-session
/// edge case can't leak into the total.
///
/// Returns `false` on a DB read failure WITHOUT sending anything — the caller
/// must not advance `last_sent_day` in that case, or the day is lost forever
/// instead of retried on the next tick. Never fabricates zeros for a real
/// error (a genuinely idle day's zeros come only from a successful read).
async fn send_daily_usage(
    app: &tauri::AppHandle,
    pool: &SqlitePool,
    distinct_id: &str,
    day: &str,
) -> bool {
    let (_, day_end) = meridian_core::date::local_day_bounds(day);
    let focus_s = match meridian_core::today::get_today(pool, day, &day_end).await {
        Ok(t) => t.focus_s,
        Err(e) => {
            tracing::warn!(error = %e, day, "analytics: today read failed for daily_usage — will retry");
            return false;
        }
    };

    let (generated, approved, rejected) = match meridian_core::worklogs::get_worklogs(pool, day)
        .await
    {
        Ok(w) => {
            let approved =
                *w.counts.get("approved").unwrap_or(&0) + *w.counts.get("posted").unwrap_or(&0);
            let rejected = *w.counts.get("skipped").unwrap_or(&0);
            let generated: i64 = w.counts.values().sum();
            (generated, approved, rejected)
        }
        Err(e) => {
            tracing::warn!(error = %e, day, "analytics: worklogs read failed for daily_usage — will retry");
            return false;
        }
    };

    let mut props = base_properties(app);
    props.insert(
        "date".to_string(),
        serde_json::Value::String(day.to_string()),
    );
    props.insert(
        "focus_hours".to_string(),
        serde_json::json!((focus_s.max(0) as f64 / 3600.0 * 100.0).round() / 100.0),
    );
    props.insert(
        "worklogs_generated".to_string(),
        serde_json::json!(generated),
    );
    props.insert("worklogs_approved".to_string(), serde_json::json!(approved));
    props.insert("worklogs_rejected".to_string(), serde_json::json!(rejected));

    tracing::info!(
        day,
        focus_s,
        generated,
        approved,
        rejected,
        "analytics: daily_usage sent"
    );
    capture("daily_usage", distinct_id, props).await;
    true
}
