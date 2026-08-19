//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Point-in-time install-health snapshot, attached to the daily `daily_usage`
//! analytics event.
//!
//! # Why this exists
//! The central OpenObserve pipeline ships **WARN+ logs only** (see
//! `meridian::telemetry_spool::redact`). That makes silence ambiguous in the
//! worst possible way: an install with no error rows is indistinguishable from
//! one that is not running, one whose user switched error reporting off, and
//! one that is genuinely fine. There is no positive signal anywhere in the
//! stack — so "is everything working for my users?" cannot be answered from
//! error telemetry alone, no matter how good the queries get.
//!
//! This snapshot IS that positive signal. It rides on the once-a-day
//! `daily_usage` event, so a healthy install actively says so.
//!
//! # What may go in here
//! Operational state about **Meridian's own components** — is the daemon up,
//! is the DB readable, is the chosen AI provider usable, are the connected
//! trackers syncing, which internal notices are latched.
//!
//! One category here is NOT about our own components and is worth naming
//! honestly: **which tracker the user has connected** (`jira`, `linear`,
//! `github`, …) is a fact about their employer's toolchain, and a rough proxy
//! for their stack. It ships because it is the question "which PM tool are
//! they using, and is it working" — and because it is a **closed set of five
//! ids** already implied by `posts_by_provider` for anyone who posted. The
//! boundary is the id and nothing else.
//!
//! Four rules that are easy to get wrong and are enforced by review, not the
//! compiler:
//!
//! - **Notices ship as `notice_id` only** (`db.corrupt`, `permissions.missing`
//!   — a fixed internal vocabulary), never `title`/`detail`/`remedy`, which are
//!   composed strings that can quote a file path or a provider's error body.
//! - **Trackers ship as provider ids only** (`jira`, `linear`) plus a status
//!   from a fixed set — never the sync error message (routinely carries a
//!   ticket key), and never the instance URL, project keys, board ids, or
//!   workspace names, all of which name the user's employer directly.
//! - **The AI provider ships as its validated wire id**, resolved through
//!   [`meridian_core::LlmProvider::from_wire`] so the value can only ever come
//!   from a closed set. It deliberately does NOT ship the display name: for a
//!   custom endpoint that is `"your AI provider"` (useless), and the
//!   unrecognised-provider branch of `in_use_provider_health` passes the raw
//!   `settings.llm_provider` string through, which is hand-editable free text.
//! - **The model id ships only for a known vendor preset.** For a hand-entered
//!   (`other`) endpoint the model string can name an internal deployment —
//!   `acme-internal-gpt4` — so it is dropped and only the vendor survives. The
//!   endpoint's base URL and API key never leave the machine under any vendor.
//!
//! # Who calls this
//! [`super::daily::send_daily_usage`] — once per completed local day, folded
//! into the same event as [`meridian_core::usage_rollup`]'s counters.
//!
//! # Related
//! - [`crate::commands::health::check_health`] — the same probe the dashboard
//!   banner uses, reused here so the two can never disagree.
//! - [`meridian_core::usage_rollup`] — the sibling *per-day counters*; this
//!   module is strictly point-in-time state.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use meridian_core::integrations::SyncState;
use meridian_core::SqlitePool;
use serde_json::{Map, Value};

/// How long a connected tracker may go without a completed sync before it is
/// reported [`IntegrationStatus::Stale`].
///
/// Every provider refreshes on a 5-minute cache TTL when the pipeline asks, so
/// a full day without a completed sync is far outside normal operation and
/// means something is actually wrong — while still being loose enough that a
/// laptop closed over a weekend does not light up the whole fleet.
const SYNC_STALE_AFTER_HOURS: i64 = 24;

/// How a connected tracker is doing, from a closed set so it can be grouped
/// directly in a chart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IntegrationStatus {
    /// Synced within [`SYNC_STALE_AFTER_HOURS`], no error on record.
    Ok,
    /// Connected and previously syncing, but nothing completed recently.
    Stale,
    /// The last sync attempt failed. Only the FACT is reported, never the
    /// provider's error text.
    Error,
    /// Connected, and structurally unable to sync until the user picks a
    /// project — GitHub with no `GITHUB_PROJECT_IDS`. Split out from
    /// [`Self::NeverSynced`] because it is permanent and silent rather than
    /// transient: no sync, no error, and nothing on screen saying why. See
    /// [`crate::commands::integrations::providers_awaiting_selection`].
    AwaitingSelection,
    /// Connected, correctly configured, and the daemon has still never
    /// completed a sync. Expected for a few minutes after connecting; a
    /// standing value here is a real fault.
    NeverSynced,
}

impl IntegrationStatus {
    /// The wire form used as a PostHog property value.
    fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Stale => "stale",
            Self::Error => "error",
            Self::AwaitingSelection => "awaiting_selection",
            Self::NeverSynced => "never_synced",
        }
    }
}

/// Classify one connected tracker. Pure, so the ladder is testable without a
/// database, a filesystem, or a clock.
///
/// Order matters: a recorded error outranks everything (it is the most
/// actionable), then the permanent misconfiguration, then never-synced, then
/// staleness.
fn classify_integration(
    state: Option<&SyncState>,
    awaiting_selection: bool,
    now: DateTime<Utc>,
) -> IntegrationStatus {
    if state.is_some_and(|s| s.last_error.is_some()) {
        return IntegrationStatus::Error;
    }
    if awaiting_selection {
        return IntegrationStatus::AwaitingSelection;
    }
    let Some(last) = state.and_then(|s| s.last_synced_at.as_deref()) else {
        return IntegrationStatus::NeverSynced;
    };
    // An unparseable timestamp is treated as never-synced rather than OK: this
    // is a health signal, and guessing "fine" from a value we cannot read is
    // the one answer that could hide a fault.
    let Ok(ts) = DateTime::parse_from_rfc3339(last) else {
        return IntegrationStatus::NeverSynced;
    };
    if now
        .signed_duration_since(ts.with_timezone(&Utc))
        .num_hours()
        >= SYNC_STALE_AFTER_HOURS
    {
        IntegrationStatus::Stale
    } else {
        IntegrationStatus::Ok
    }
}

/// Install health at the moment the daily event is assembled.
///
/// Every field is `Option` where the underlying probe can be inconclusive —
/// a probe that failed to run must serialise as absent, never as `false`,
/// because "we could not tell" and "it is broken" would otherwise be the same
/// number on a dashboard.
#[derive(Debug, Clone)]
pub(crate) struct HealthSnapshot {
    /// The daemon process is alive.
    pub daemon_running: Option<bool>,
    /// `meridian.db` opened and answered a probe query.
    pub database_ready: Option<bool>,
    /// The user's chosen LLM provider looks usable (installed, last test OK).
    pub llm_provider_ok: Option<bool>,
    /// That provider is usable but currently rate-limited — a soft,
    /// self-clearing state, deliberately distinct from unusable.
    pub llm_provider_rate_limited: Option<bool>,
    /// The in-use AI provider's WIRE id (`claude`, `codex`, `cursor`,
    /// `copilot`, `custom`), or `unrecognised` when `settings.llm_provider`
    /// does not parse. Validated through a closed enum rather than passed
    /// through as a string — see the module doc's fourth rule.
    pub llm_provider: &'static str,
    /// For a `custom` endpoint, the preset it came from (`openai`, `gemini`,
    /// `openrouter`, `other`). `None` for the CLI providers.
    pub llm_vendor: Option<String>,
    /// The model id in use. Omitted for a hand-entered (`other`) endpoint,
    /// whose model string can name an internal deployment.
    pub llm_model: Option<String>,
    /// Ids of currently-latched system notices, e.g. `db.corrupt`. Ids only —
    /// see the module doc.
    pub active_notice_ids: Vec<String>,
    /// Every connected tracker id — the answer to "which PM tool are they
    /// using". Connectedness is filesystem state (`.env` keys + OAuth token
    /// files), so a tracker appears here even if it has never synced.
    pub integrations_connected: Vec<String>,
    /// Connected tracker id → how it is doing. Same key set as
    /// [`Self::integrations_connected`].
    pub integrations_status: BTreeMap<String, IntegrationStatus>,
    /// The user's notification master switch.
    pub notifications_enabled: bool,
    /// The user's error-reporting switch — shipped so a quiet install can be
    /// explained ("they turned it off") rather than guessed at.
    pub error_reporting_enabled: bool,
}

/// Hand-written rather than derived because `#[derive(Default)]` would give
/// [`HealthSnapshot::llm_provider`] the empty string — a value that means
/// nothing on a chart and is not one of the ids the field promises. The
/// default is [`UNRECOGNISED_PROVIDER`], which is a real, documented state.
impl Default for HealthSnapshot {
    fn default() -> Self {
        Self {
            daemon_running: None,
            database_ready: None,
            llm_provider_ok: None,
            llm_provider_rate_limited: None,
            llm_provider: UNRECOGNISED_PROVIDER,
            llm_vendor: None,
            llm_model: None,
            active_notice_ids: Vec::new(),
            integrations_connected: Vec::new(),
            integrations_status: BTreeMap::new(),
            notifications_enabled: false,
            error_reporting_enabled: false,
        }
    }
}

/// The value shipped when `settings.llm_provider` does not parse as a known
/// provider — a downgrade, or a hand-edited settings file. Never the raw
/// string, which is free text; see the module doc's fourth rule.
const UNRECOGNISED_PROVIDER: &str = "unrecognised";

/// The in-use provider's wire id, validated through the closed
/// [`meridian_core::LlmProvider`] enum.
fn resolve_provider(settings: &meridian_core::settings::RuntimeSettings) -> &'static str {
    meridian_core::LlmProvider::from_wire(&settings.llm_provider)
        .map_or(UNRECOGNISED_PROVIDER, |p| p.as_str())
}

/// Probe every health source and assemble the snapshot.
///
/// Never fails: each source already degrades to "unknown"/empty on error, and
/// a health snapshot must not be able to block the analytics event it decorates.
#[tracing::instrument(skip(pool))]
pub(crate) async fn snapshot(pool: &SqlitePool) -> HealthSnapshot {
    let h = crate::commands::health::check_health().await;
    let settings = meridian_core::settings::load_runtime_settings();

    let active_notice_ids = meridian_core::notices::read_notices(pool)
        .await
        .into_iter()
        .map(|n| n.notice_id)
        .collect::<Vec<_>>();

    // Which trackers are connected is filesystem state (`.env` + OAuth token
    // files); how they are DOING is database state. Both halves are needed:
    // the DB alone cannot see a tracker that has never synced, and the
    // filesystem alone cannot see one that is failing.
    let connected = crate::commands::integrations::connected_providers();
    let awaiting = crate::commands::integrations::providers_awaiting_selection();
    let sync = meridian_core::integrations::sync_state(pool).await;
    let now = Utc::now();

    let integrations_status: BTreeMap<String, IntegrationStatus> = connected
        .iter()
        .map(|p| {
            (
                (*p).to_string(),
                classify_integration(sync.get(*p), awaiting.contains(p), now),
            )
        })
        .collect();

    // A custom endpoint's vendor and model. The base URL and API key on this
    // same struct must never be touched here — see the module doc.
    let custom = settings.active_custom_provider();
    let llm_vendor = custom.map(|c| c.vendor.clone());
    let llm_model = match custom {
        // A hand-entered endpoint's model can name an internal deployment.
        Some(c) if c.vendor == "other" => None,
        Some(c) => Some(c.model.clone()),
        None => settings.llm_provider_model.clone(),
    };

    let snap = HealthSnapshot {
        daemon_running: h.daemon_running,
        database_ready: h.database_ready,
        llm_provider_ok: h.llm_provider_ok,
        llm_provider_rate_limited: h.llm_provider_rate_limited,
        llm_provider: resolve_provider(&settings),
        llm_vendor,
        llm_model,
        active_notice_ids,
        integrations_connected: connected.iter().map(|p| (*p).to_string()).collect(),
        integrations_status,
        notifications_enabled: settings.notifications_enabled,
        error_reporting_enabled: settings.error_reporting_enabled,
    };

    tracing::info!(
        daemon_running = ?snap.daemon_running,
        database_ready = ?snap.database_ready,
        llm_provider = snap.llm_provider,
        llm_provider_ok = ?snap.llm_provider_ok,
        notices = snap.active_notice_ids.len(),
        integrations = snap.integrations_connected.len(),
        integrations_failing = snap.integrations_failing_count(),
        "analytics: health snapshot"
    );
    snap
}

impl HealthSnapshot {
    /// Connected trackers that are not currently working — anything but
    /// [`IntegrationStatus::Ok`].
    ///
    /// [`IntegrationStatus::Stale`] counts: a tracker that has not completed a
    /// sync in a day is not doing its job, whatever the reason. A user whose
    /// laptop was shut for the weekend will show one here, which is the
    /// accepted false-positive rate for catching genuinely broken syncs.
    pub(crate) fn integrations_failing_count(&self) -> usize {
        self.integrations_status
            .values()
            .filter(|s| **s != IntegrationStatus::Ok)
            .count()
    }

    /// Fold the snapshot into an event's property map under a `health_` prefix,
    /// so PostHog's property list stays legible next to the usage counters.
    ///
    /// `Option::None` fields are OMITTED rather than written as `false` — see
    /// the struct doc on why "unknown" must not collapse into "broken".
    ///
    /// # These properties describe a DIFFERENT instant to the rest of the event
    /// `daily_usage` is sent on the first tick after midnight and every other
    /// property on it describes the day that just closed. This snapshot is
    /// taken at SEND time — so `health_daemon_running: false` on the event
    /// dated 2026-05-14 means the daemon was down early on the **15th**.
    ///
    /// Sitting next to `tickets_updated`, that asymmetry is invisible and will
    /// eventually produce a confident wrong answer in a support conversation.
    /// `health_observed_on` is therefore written alongside, carrying the day
    /// the snapshot actually describes, so the difference is visible at the
    /// query site rather than buried in this doc comment.
    pub(crate) fn write_properties(&self, props: &mut Map<String, Value>) {
        props.insert(
            "health_observed_on".to_string(),
            Value::String(meridian_core::date::today_string()),
        );
        let mut put_bool = |k: &str, v: Option<bool>| {
            if let Some(b) = v {
                props.insert(k.to_string(), Value::Bool(b));
            }
        };
        put_bool("health_daemon_running", self.daemon_running);
        put_bool("health_database_ready", self.database_ready);
        put_bool("health_llm_provider_ok", self.llm_provider_ok);
        put_bool(
            "health_llm_provider_rate_limited",
            self.llm_provider_rate_limited,
        );

        props.insert(
            "health_llm_provider".to_string(),
            Value::String(self.llm_provider.to_string()),
        );
        if let Some(v) = &self.llm_vendor {
            props.insert("health_llm_vendor".to_string(), Value::String(v.clone()));
        }
        if let Some(m) = &self.llm_model {
            props.insert("health_llm_model".to_string(), Value::String(m.clone()));
        }
        props.insert(
            "health_active_notices".to_string(),
            serde_json::json!(self.active_notice_ids),
        );

        // Which trackers, and how each is doing. The map answers a drill-down;
        // the two flat values answer "which PM tools are in the fleet" and
        // "how many users have a broken one" without digging into a nested
        // object in HogQL — the same reasoning as the notification totals.
        props.insert(
            "health_integrations_connected".to_string(),
            serde_json::json!(self.integrations_connected),
        );
        props.insert(
            "health_integrations_status".to_string(),
            serde_json::json!(self
                .integrations_status
                .iter()
                .map(|(k, v)| (k.clone(), v.as_str()))
                .collect::<BTreeMap<_, _>>()),
        );
        props.insert(
            "health_integrations_failing_count".to_string(),
            serde_json::json!(self.integrations_failing_count()),
        );
        props.insert(
            "setting_notifications_enabled".to_string(),
            Value::Bool(self.notifications_enabled),
        );
        props.insert(
            "setting_error_reporting_enabled".to_string(),
            Value::Bool(self.error_reporting_enabled),
        );
    }
}

#[cfg(test)]
mod integration_status_tests {
    //! The connected-tracker health ladder. Pure, so no DB/filesystem/clock.
    use super::*;

    fn at(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }
    fn synced(when: &str) -> SyncState {
        SyncState {
            last_synced_at: Some(when.to_string()),
            last_error: None,
        }
    }

    #[test]
    fn a_recent_sync_is_ok_and_a_day_old_one_is_stale() {
        let now = at("2026-05-14T12:00:00Z");
        assert_eq!(
            classify_integration(Some(&synced("2026-05-14T11:00:00Z")), false, now),
            IntegrationStatus::Ok
        );
        assert_eq!(
            classify_integration(Some(&synced("2026-05-13T11:00:00Z")), false, now),
            IntegrationStatus::Stale
        );
    }

    #[test]
    fn a_recorded_error_outranks_everything() {
        // Even a sync that completed seconds ago: if the LAST attempt failed,
        // that is the actionable fact.
        let now = at("2026-05-14T12:00:00Z");
        let state = SyncState {
            last_synced_at: Some("2026-05-14T11:59:00Z".to_string()),
            last_error: Some("401 Unauthorized".to_string()),
        };
        assert_eq!(
            classify_integration(Some(&state), true, now),
            IntegrationStatus::Error,
            "an error outranks even awaiting_selection"
        );
    }

    #[test]
    fn github_without_a_project_is_awaiting_selection_not_never_synced() {
        // The distinction that matters: GitHub with a token but no project
        // picked skips syncing entirely and reports nothing. Collapsing it
        // into `never_synced` would hide a permanent silent dead end among
        // installs that merely connected a minute ago.
        let now = at("2026-05-14T12:00:00Z");
        assert_eq!(
            classify_integration(None, true, now),
            IntegrationStatus::AwaitingSelection
        );
        assert_eq!(
            classify_integration(None, false, now),
            IntegrationStatus::NeverSynced
        );
    }

    #[test]
    fn an_unreadable_timestamp_never_reports_healthy() {
        // Guessing "fine" from a value we cannot parse is the one answer that
        // could hide a real fault.
        let now = at("2026-05-14T12:00:00Z");
        assert_eq!(
            classify_integration(Some(&synced("not-a-timestamp")), false, now),
            IntegrationStatus::NeverSynced
        );
    }

    #[test]
    fn failing_count_treats_every_non_ok_status_as_failing() {
        let mut snap = HealthSnapshot {
            integrations_connected: vec!["jira".into(), "github".into(), "linear".into()],
            ..Default::default()
        };
        snap.integrations_status
            .insert("jira".into(), IntegrationStatus::Ok);
        snap.integrations_status
            .insert("github".into(), IntegrationStatus::AwaitingSelection);
        snap.integrations_status
            .insert("linear".into(), IntegrationStatus::Stale);

        assert_eq!(snap.integrations_failing_count(), 2);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The AI provider must ship as a validated wire id from the closed enum.
    /// The display name it replaced was useless for a custom endpoint
    /// (`"your AI provider"`) and, on the unrecognised-provider branch,
    /// forwarded the hand-editable `settings.llm_provider` string verbatim.
    #[test]
    fn provider_ships_as_a_validated_wire_id() {
        let known = meridian_core::settings::RuntimeSettings {
            llm_provider: "codex".to_string(),
            ..Default::default()
        };
        assert_eq!(resolve_provider(&known), "codex");

        let bogus = meridian_core::settings::RuntimeSettings {
            llm_provider: "totally-made-up".to_string(),
            ..Default::default()
        };
        assert_eq!(
            resolve_provider(&bogus),
            UNRECOGNISED_PROVIDER,
            "an unparseable provider must never pass its raw string through"
        );
    }

    /// Tracker ids and statuses always ship, and the flat failing count rides
    /// alongside the map so "how many users have a broken tracker" is one line
    /// of HogQL rather than a dig into a nested object.
    #[test]
    fn integration_properties_ship_ids_statuses_and_a_flat_count() {
        let mut snap = HealthSnapshot {
            integrations_connected: vec!["jira".into(), "github".into()],
            ..Default::default()
        };
        snap.integrations_status
            .insert("jira".into(), IntegrationStatus::Ok);
        snap.integrations_status
            .insert("github".into(), IntegrationStatus::AwaitingSelection);

        let mut props = Map::new();
        snap.write_properties(&mut props);

        assert_eq!(
            props.get("health_integrations_connected"),
            Some(&serde_json::json!(["jira", "github"]))
        );
        assert_eq!(
            props.get("health_integrations_status"),
            Some(&serde_json::json!({"github": "awaiting_selection", "jira": "ok"}))
        );
        assert_eq!(
            props.get("health_integrations_failing_count"),
            Some(&serde_json::json!(1))
        );
    }

    /// An inconclusive probe must not serialise as `false`. Collapsing
    /// "couldn't tell" into "broken" would show healthy installs as failing on
    /// every dashboard built from this event.
    #[test]
    fn unknown_probes_are_omitted_not_false() {
        let snap = HealthSnapshot {
            daemon_running: None,
            database_ready: Some(true),
            ..Default::default()
        };
        let mut props = Map::new();
        snap.write_properties(&mut props);

        assert!(
            !props.contains_key("health_daemon_running"),
            "an unknown probe must be absent, never false"
        );
        assert_eq!(props.get("health_database_ready"), Some(&Value::Bool(true)));
    }

    /// The health group must always stamp the day it describes. Every OTHER
    /// property on `daily_usage` describes the day that closed; this group
    /// describes the morning after, and without the stamp that is invisible at
    /// the query site.
    #[test]
    fn health_carries_the_day_it_actually_describes() {
        let mut props = Map::new();
        HealthSnapshot::default().write_properties(&mut props);

        assert_eq!(
            props.get("health_observed_on"),
            Some(&Value::String(meridian_core::date::today_string())),
            "health is a point-in-time read, not a read of the reported day"
        );
    }

    /// The notice/integration lists always ship, even when empty — an absent
    /// key and an empty list would otherwise be indistinguishable, and "no
    /// notices" is exactly the healthy signal this module exists to send.
    #[test]
    fn empty_lists_still_ship() {
        let mut props = Map::new();
        HealthSnapshot::default().write_properties(&mut props);

        assert_eq!(
            props.get("health_active_notices"),
            Some(&serde_json::json!([] as [String; 0]))
        );
        assert_eq!(
            props.get("health_integrations_connected"),
            Some(&serde_json::json!([] as [String; 0])),
            "no trackers connected is a real, chartable answer - not a missing key"
        );
        assert_eq!(
            props.get("health_integrations_failing_count"),
            Some(&serde_json::json!(0))
        );
    }

    /// A hand-entered (`other`) endpoint's model can name an internal
    /// deployment, so only the vendor survives. Asserted on the writer because
    /// that is the boundary the value actually crosses.
    #[test]
    fn a_hand_entered_endpoints_model_never_ships() {
        let with_model = HealthSnapshot {
            llm_provider: "custom",
            llm_vendor: Some("other".to_string()),
            llm_model: None, // what `snapshot()` produces for vendor == "other"
            ..Default::default()
        };
        let mut props = Map::new();
        with_model.write_properties(&mut props);

        assert_eq!(
            props.get("health_llm_vendor"),
            Some(&Value::String("other".to_string())),
            "the vendor still ships - it is a closed-set preset id"
        );
        assert!(
            !props.contains_key("health_llm_model"),
            "but the model must not"
        );
    }
}
