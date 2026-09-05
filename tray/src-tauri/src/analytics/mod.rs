//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Product analytics — PostHog Cloud event capture.
//!
//! # What this is
//! A deliberately tiny, best-effort telemetry client: three IDENTIFIED signals,
//! all capped at once-per-day or once-ever —
//! - `app_installed`, once per (signed-in email, device) pair;
//! - `app_active`, once per local calendar day the tray is actually running —
//!   the return-rate signal (see [`state::day_active_action`] for why `daily_usage`
//!   cannot serve as one), and the event that keeps the person properties
//!   fresh (see [`base_person_properties`]);
//! - `daily_usage`, once per COMPLETED local calendar day, carrying engagement
//!   hours, product-action counts ([`meridian_core::usage_rollup`]) and an
//!   install-health snapshot ([`health`]) — assembled in [`daily`].
//!
//! Nothing else: a single raw HTTP POST to PostHog's `/i/v0/e/`
//! capture endpoint, no `posthog-js`, so session replay / autocapture /
//! surveys / feature flags never activate — that's client-SDK behaviour this
//! code never touches. GeoIP enrichment is left on PostHog's default (no
//! `$geoip_disable`), so events resolve each user's location from their
//! request IP.
//!
//! **Consent.** Everything here is gated on
//! [`meridian_core::settings::RuntimeSettings::product_analytics_enabled`]
//! (opt-OUT, default true) and on the hard `MERIDIAN_TELEMETRY_DISABLED` kill
//! switch. That setting is deliberately SEPARATE from
//! `error_reporting_enabled`: this stream is identified by the user's account
//! email and describes product usage, whereas the error stream is pseudonymous
//! and error-only. Conflating them would mean turning off crash reports
//! silently killed usage reporting, and vice versa. The gate is re-read every
//! tick, so switching it off in Settings takes effect within ~60 s and needs
//! no relaunch.
//!
//! **The join key.** Every event carries a `support_id` property — the same
//! [`meridian::telemetry_spool::redact::local_host_pseudonym`] value that
//! identifies this install in OpenObserve and Sentry, and that Settings shows
//! the user as their Support ID. It is what makes "this user is erroring"
//! answerable at all: without it, PostHog knows emails and the error backends
//! know pseudonyms, with nothing connecting the two. It ships as an EVENT
//! property, not a person property, on purpose — the pseudonym legitimately
//! changes over an install's life (a second machine, a re-seed, the alpha
//! account-scoping window ending), and a person property would be overwritten
//! each time, silently orphaning every historical error row. As an event
//! property the full set is recoverable with a `DISTINCT` over the user's
//! events, across devices and across any re-seed.
//!
//! **Person properties.** Alongside the event properties, each event carries a
//! `$set` payload ([`base_person_properties`] +
//! [`health::HealthSnapshot::write_person_properties`]) holding the user's
//! slowly-changing CURRENT state: which AI provider and model, which trackers,
//! app version, channel, OS, and the email itself (so `person.email` is
//! filterable — it is already the `distinct_id`, so this adds no new data).
//! These exist because "which provider is each of my users on" is otherwise
//! answerable only by finding every person's latest `daily_usage` and reading
//! it, one query per person; as person properties it is a sortable column.
//! Momentary state (daemon up, notices, per-tracker sync status) stays on the
//! EVENT, where it is timestamped — a person property keeps only the newest
//! value and would read as current fact while answering "at the last send".
//!
//! **No anonymous events, ever.** The PostHog `distinct_id` for all events IS
//! the signed-in account email ([`crate::commands::account::read_account_email`])
//! — nothing is sent, for either event, until sign-in has happened. This is
//! deliberate: an anonymous per-machine UUID identity was tried first and
//! rejected, because it left permanently-orphaned UUID Persons in PostHog for
//! any event captured before sign-in completed (a `$set` on a later event
//! upgrades that Person's properties but never renames its `distinct_id`, so
//! the UUID keeps showing in the events explorer forever). Waiting for the
//! email and using it directly as `distinct_id` means every event lands on
//! one identified Person from the start — no UUID Persons, no merge step.
//!
//! **Install dedup is per (device, email), not per device.** A device may see
//! more than one signed-in email over its lifetime (sign-out, sign back in as
//! someone else), and the same email may install on more than one device.
//! `app_installed` should fire again in both of those cases, but never twice
//! for the same (device, email) pair (e.g. sign-out then sign back in as the
//! same person). So the per-device state file tracks a SET of emails this
//! device has already reported `app_installed` for
//! ([`state::AnalyticsState::install_events_sent`]), not a single boolean. A
//! per-device `device_id` (a random UUID, unchanged from the old anonymous
//! identity) still rides along as an event PROPERTY — never as `distinct_id`
//! — so cross-device patterns for one email stay analyzable in PostHog
//! without it ever becoming a Person of its own.
//!
//! Runs in every install shape — a source/dev checkout, a bare launch, and a
//! packaged DMG alike — so a local `cargo run`/`tauri dev` session shows up
//! too. Every event carries `channel` ([`crate::commands::version::build_channel`]:
//! `dev`/`staging`/`prod`) so dev-run noise stays filterable in PostHog from
//! real installs, rather than being excluded outright. The `device_id` plus
//! day/email bookkeeping live in `~/.meridian/analytics_state.json`, separate
//! from `settings.json` (never dashboard-editable, never displayed).
//!
//! Call volume is intentionally minimal: at most 1 `app_installed` per
//! (device, email) pair ever, + at most 1 `app_active` and 1 `daily_usage` per
//! calendar day, regardless of how many times the tray restarts or how often
//! the poll loop ticks. At the current fleet size that is ~2 events per user
//! per day — nowhere near PostHog's free-tier limits.
//!
//! **`daily_usage` is not backfilled**: only the single most-recently-closed
//! day is ever reported (see [`state::day_rollover_action`]). If the tray isn't running across a
//! local-day boundary for several days — including the entire time before the
//! very first sign-in — the intervening day(s) are skipped entirely, never
//! sent late. The first tick after sign-in only arms today's date; the
//! earliest day it can ever report is the day AFTER that.
//!
//! **Nothing is lost on failure.** [`capture`] reports success/failure via
//! its return value, and every call site ([`maybe_send_daily_tick`],
//! [`daily::send_daily_usage`]) only flips its persisted "sent" bookkeeping
//! (`install_events_sent` / `last_active_day_by_email` /
//! `last_sent_day_by_email`) on a confirmed success. A DB read
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
//! - [`crate::commands::account::read_account_email`] — the identity gate:
//!   nothing here sends anything until this returns `Some`.
//! - `meridian_core::today::get_today` / `meridian_core::worklogs::get_worklogs`
//!   — the same readers the dashboard uses, queried here for an already-closed
//!   past day (never "today", which is still accumulating).

/// Assembly of the `daily_usage` event — engagement hours + product actions +
/// health. Split out to keep this file under the repo's 500-line cap.
mod daily;
/// The point-in-time install-health snapshot folded into `daily_usage`.
mod health;
/// CPU/RSS sampling of Meridian's own processes, aggregated per day.
pub(crate) mod perf;
/// On-disk bookkeeping + the pure per-email cursor rules driving each tick.
mod state;

use meridian_core::SqlitePool;
use state::{
    analytics_state_path, day_active_action, day_rollover_action, load_or_init_state, save_state,
};
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

/// POST one event to PostHog's raw capture endpoint (no SDK — see module
/// docs). Never panics or propagates an error — analytics must never affect
/// the app's behaviour or startup — but DOES report success/failure via its
/// return value so callers with a "send at most once" bookkeeping flag (the
/// `install_event_sent`/`last_sent_day` fields in [`AnalyticsState`]) know
/// not to advance that bookkeeping on a failed send. Returns `true` only on
/// an HTTP 2xx from PostHog; `false` on a missing API key, a request/timeout
/// error, or a non-2xx response — in every `false` case the caller must
/// retry, never silently drop the event.
pub(crate) async fn capture(
    event: &str,
    distinct_id: &str,
    properties: serde_json::Map<String, serde_json::Value>,
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
/// from a real install rather than being excluded outright), plus the
/// per-device `device_id` ([`AnalyticsState::device_id`]) as a plain property
/// — never the PostHog `distinct_id`, which is always the signed-in email
/// (see the module doc). Callers only ever reach this after confirming an
/// email is present, so identity itself never needs to be threaded through
/// here.
pub(crate) fn base_properties(
    app: &tauri::AppHandle,
    device_id: &str,
) -> serde_json::Map<String, serde_json::Value> {
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
    m.insert(
        "device_id".to_string(),
        serde_json::Value::String(device_id.to_string()),
    );
    // The join key to OpenObserve / Sentry — see the module doc's "The join
    // key" section for why this is an event property rather than a person one.
    // Computed from the SAME function that produces the shipped `host.name`
    // and the Support ID shown in Settings, never recomputed separately, so
    // the three can never drift apart.
    m.insert(
        "support_id".to_string(),
        serde_json::Value::String(meridian::telemetry_spool::redact::local_host_pseudonym()),
    );
    m
}

/// PostHog **person** properties — the `$set` payload.
///
/// # Why these exist alongside the event properties
/// Everything else here is an event property, which is right for anything that
/// describes a moment. But "which AI provider is this user on" and "which
/// trackers do they have" are *current state* of a person, and as event
/// properties they are only answerable by finding that person's latest
/// `daily_usage` and reading it — per person, in HogQL. As person properties
/// they become a column on the Persons list, sortable and breakdown-able in one
/// click. Being overwritten on every send is exactly the wanted behaviour.
///
/// **`support_id` is deliberately NOT here.** It is the one value where being
/// overwritten is harmful rather than helpful: the pseudonym legitimately
/// changes over an install's life, and a person property would keep only the
/// newest, silently orphaning every error row recorded under the previous one.
/// It stays an event property so a `DISTINCT` recovers the full set — see the
/// module doc's "The join key".
///
/// `email` is included even though it is already the `distinct_id`, because
/// PostHog cannot filter on `person.email` unless the property exists. It adds
/// no new data: the address is the identity of every one of these events
/// already.
fn base_person_properties(
    app: &tauri::AppHandle,
    email: &str,
) -> serde_json::Map<String, serde_json::Value> {
    let mut m = serde_json::Map::new();
    m.insert(
        "email".to_string(),
        serde_json::Value::String(email.to_string()),
    );
    m.insert(
        "app_version".to_string(),
        serde_json::Value::String(app.package_info().version.to_string()),
    );
    m.insert(
        "channel".to_string(),
        serde_json::Value::String(crate::commands::version::build_channel().to_string()),
    );
    m.insert(
        "os".to_string(),
        serde_json::Value::String(std::env::consts::OS.to_string()),
    );
    m
}

/// Attach a `$set` (and, when `unset` is non-empty, `$unset`) person-property
/// payload to an event's properties.
///
/// PostHog reads both from inside `properties` on the capture endpoint, so
/// this is plain nested data rather than a separate request. `unset` clears a
/// PREVIOUSLY-set value that has gone away this send (e.g.
/// [`health::HealthSnapshot::write_person_properties`]'s `llm_vendor`/
/// `llm_model` when the user is no longer on a custom endpoint) — `$set`
/// alone cannot do this, since an omitted key is simply never mentioned
/// again, not cleared.
fn attach_person_properties(
    props: &mut serde_json::Map<String, serde_json::Value>,
    person: serde_json::Map<String, serde_json::Value>,
    unset: Vec<String>,
) {
    props.insert("$set".to_string(), serde_json::Value::Object(person));
    if !unset.is_empty() {
        props.insert("$unset".to_string(), serde_json::json!(unset));
    }
}

/// Whether product analytics may be sent at all: the user hasn't switched it
/// off, and the hard kill switch is clear.
///
/// Re-read every tick rather than cached at startup, so turning the switch off
/// in Settings stops the next tick's events without a relaunch. Deliberately
/// checks `product_analytics_enabled`, NOT `error_reporting_enabled` — see the
/// module doc's "Consent" section.
pub(crate) fn analytics_allowed() -> bool {
    let killed = std::env::var("MERIDIAN_TELEMETRY_DISABLED")
        .is_ok_and(|v| v == "1" || v.eq_ignore_ascii_case("true"));
    if killed {
        return false;
    }
    meridian_core::settings::load_runtime_settings().product_analytics_enabled
}

/// Called once per poll-loop health tick (~60 s — see [`crate::poll`]). A
/// cheap file read on every call; the two possible HTTP calls are
/// individually capped to once-per-(device, email) (`app_installed`) and
/// once-per-completed local day (`daily_usage`) — but only once each has
/// actually *succeeded*. Whenever `capture()` fails (no key, timeout, non-2xx
/// — see its doc), the corresponding bookkeeping field is left untouched, so
/// the very next tick (and every tick after that, ~60 s apart) retries the
/// exact same pending event — never a fabricated/skipped one — until it
/// lands. That bookkeeping is persisted to `analytics_state.json`, so the
/// retry survives a tray restart too: a `daily_usage` that failed to send
/// stays pending even if the tray is closed and reopened the "next day
/// morning", and is sent for its original (correct) day as soon as the app
/// is next open and reachable.
///
/// Both events are gated on [`crate::commands::account::read_account_email`]
/// returning `Some` — nothing is ever sent anonymously (see the module doc).
/// While no email is on file (sign-in never completed, or the user signed
/// out), this is a no-op every tick: no HTTP call, and day-rollover
/// bookkeeping (`last_sent_day_by_email`) stays un-armed for that email, so the day the user
/// eventually signs in on is never reported — only the first FULL day after
/// that is, matching the "never backfill" rule elsewhere in this module.
///
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

    if !analytics_allowed() {
        // Switched off in Settings, or the hard kill switch is set. Nothing is
        // sent AND no bookkeeping is advanced, so re-enabling later resumes
        // cleanly from wherever the cursors were left.
        return;
    }

    let Some(email) = crate::commands::account::read_account_email() else {
        // Not signed in — no identity to attach events to, so nothing to do.
        return;
    };

    let Some(path) = analytics_state_path() else {
        return;
    };
    let mut state = load_or_init_state(&path);
    let mut dirty = false;

    if !state.install_events_sent.contains(&email) {
        let mut props = base_properties(app, &state.device_id);
        // The health snapshot rides here too, not just on `app_active`/`daily_usage`
        // - `app_installed` fires exactly once per (device, account), so if it were
        // the ONLY event whose person properties never carried provider/tracker
        // state, a signed-in user whose tray never survives to the next event would
        // show up in PostHog with no AI provider or tracker set at all.
        let mut person = base_person_properties(app, &email);
        let mut unset = Vec::new();
        health::snapshot(pool)
            .await
            .write_person_properties(&mut person, &mut unset);
        attach_person_properties(&mut props, person, unset);
        if capture("app_installed", &email, props).await {
            state.install_events_sent.insert(email.clone());
            dirty = true;
            // INFO, not DEBUG: this fires once per (device, account) ever, so
            // it costs nothing, and without it a successful send leaves NO
            // trace in the logs at all — the on-disk cursor is the only
            // evidence. Failures already log at WARN, so "no WARN" was the
            // only available proof of success, which is an inference rather
            // than a record. Measured on a real staging install: 1500 log
            // records covering the exact minute of a confirmed send contained
            // zero analytics lines.
            tracing::info!(event = "app_installed", "analytics: event sent");
        }
    }

    let today = meridian_core::date::today_string();

    // The return-rate signal. Fires on the first tick of each local day the
    // tray is running, BEFORE the daily_usage rollover below, and about TODAY
    // rather than a closed day — see [`state::day_active_action`].
    if day_active_action(
        state
            .last_active_day_by_email
            .get(&email)
            .map(|s| s.as_str()),
        &today,
    ) {
        let mut props = base_properties(app, &state.device_id);
        props.insert("date".to_string(), serde_json::Value::String(today.clone()));

        // The health snapshot rides here as PERSON properties, not just on
        // `daily_usage`. That matters for exactly the question this data is
        // for: `daily_usage` needs a local-midnight rollover, so attaching the
        // provider and tracker set only there means a brand-new user's AI
        // provider and PM tool are unknown until their second calendar day —
        // and permanently unknown if their tray never survives a midnight.
        // `app_active` fires on day one, so these land immediately.
        let mut person = base_person_properties(app, &email);
        let mut unset = Vec::new();
        health::snapshot(pool)
            .await
            .write_person_properties(&mut person, &mut unset);
        attach_person_properties(&mut props, person, unset);

        if capture("app_active", &email, props).await {
            state
                .last_active_day_by_email
                .insert(email.clone(), today.clone());
            dirty = true;
            tracing::info!(event = "app_active", day = %today, "analytics: event sent");
        }
    }

    // Read/advance THIS email's own cursor — never a shared one — so a
    // still-pending day accrued under a previously signed-in account can never
    // be attributed to whoever is signed in now (see [`AnalyticsState::last_sent_day_by_email`]).
    let last_sent_day = state.last_sent_day_by_email.get(&email).cloned();
    match day_rollover_action(last_sent_day.as_deref(), &today) {
        Some(None) => {
            // First observation since sign-in (or ever) for THIS email —
            // nothing has closed yet, so there's nothing to report.
            state.last_sent_day_by_email.insert(email.clone(), today);
            dirty = true;
        }
        Some(Some(day)) => {
            // Only advance past `day` when it actually sent — a transient DB
            // read hiccup must retry next tick, not silently report (and
            // permanently skip) a fabricated zero-usage day.
            if daily::send_daily_usage(app, pool, &email, &state.device_id, &day).await {
                state.last_sent_day_by_email.insert(email.clone(), today);
                dirty = true;
            }
        }
        None => {}
    }

    if dirty {
        save_state(&path, &state);
    }
}

/// Fire a `pm_tool_requested` event for [`crate::commands::pm_tool_request::request_pm_tool`]
/// — a user typed a PM tool into `ConnectTrackers`' "I don't see my tool" row.
///
/// Best-effort and fire-and-forget by design, unlike every other event in
/// this module: there is no bookkeeping cursor to retry from (this is a
/// one-off ad hoc action, not a per-day/per-install rollup), so a failed send
/// is simply lost rather than retried next tick. That is an acceptable
/// tradeoff for a secondary demand signal — the caller ALSO persists the same
/// text into `settings.json` (`RuntimeSettings::requested_pm_tool`), which is
/// what survives regardless of whether this event lands.
pub(crate) async fn send_pm_tool_requested(app: &tauri::AppHandle, tool_name: &str) {
    if !analytics_allowed() {
        return;
    }
    let Some(email) = crate::commands::account::read_account_email() else {
        // Not signed in — no identity to attach the event to (see the module
        // doc's "No anonymous events, ever").
        return;
    };
    let Some(path) = analytics_state_path() else {
        return;
    };
    // Persist BEFORE sending. `load_or_init_state` mints a fresh `device_id`
    // in memory when the file is missing or unreadable and does not write it
    // back, so without this every event from such a machine would carry a
    // different device_id and read as a different install.
    let state = load_or_init_state(&path);
    save_state(&path, &state);
    let mut props = base_properties(app, &state.device_id);
    props.insert(
        "tool_name".to_string(),
        serde_json::Value::String(tool_name.to_string()),
    );
    // `tool_name` stays OUT of the log line - it is free text the user typed,
    // and `tracing` is captured at full fidelity locally (and travels in an
    // unredacted diagnostics bundle). The consented PostHog payload above is
    // where it belongs. See `commands::pm_tool_request` for the same rule on
    // the span field.
    if capture("pm_tool_requested", &email, props).await {
        tracing::info!("analytics: pm_tool_requested sent");
    }
}
