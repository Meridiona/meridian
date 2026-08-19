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
//! is the DB readable, is the chosen LLM provider usable, which internal
//! notices are latched. Never the user's data. Two rules that are easy to get
//! wrong and are enforced by review, not the compiler:
//!
//! - **Notices ship as `notice_id` only** (`db.corrupt`, `permissions.missing`
//!   — a fixed internal vocabulary), never `title`/`detail`/`remedy`, which are
//!   composed strings that can quote a file path or a provider's error body.
//! - **Integration failures ship as provider ids only** (`jira`, `linear`),
//!   never the sync error message, which routinely carries a ticket key.
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

use meridian_core::SqlitePool;
use serde_json::{Map, Value};

/// Install health at the moment the daily event is assembled.
///
/// Every field is `Option` where the underlying probe can be inconclusive —
/// a probe that failed to run must serialise as absent, never as `false`,
/// because "we could not tell" and "it is broken" would otherwise be the same
/// number on a dashboard.
#[derive(Debug, Clone, Default)]
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
    /// Human name of the in-use provider (`Codex`, `Claude`, …). Names one of
    /// OUR components from a fixed set, never user data.
    pub llm_provider_name: Option<String>,
    /// Ids of currently-latched system notices, e.g. `db.corrupt`. Ids only —
    /// see the module doc.
    pub active_notice_ids: Vec<String>,
    /// Provider ids whose last tracker sync failed. Ids only — never the
    /// error text.
    pub integrations_failing: Vec<String>,
    /// The user's notification master switch.
    pub notifications_enabled: bool,
    /// The user's error-reporting switch — shipped so a quiet install can be
    /// explained ("they turned it off") rather than guessed at.
    pub error_reporting_enabled: bool,
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

    // Only the KEYS of the error map — the values are provider error bodies.
    let integrations_failing = meridian_core::integrations::sync_errors(pool)
        .await
        .map(|m| m.into_keys().collect::<Vec<_>>())
        .unwrap_or_default();

    let snap = HealthSnapshot {
        daemon_running: h.daemon_running,
        database_ready: h.database_ready,
        llm_provider_ok: h.llm_provider_ok,
        llm_provider_rate_limited: h.llm_provider_rate_limited,
        llm_provider_name: h.llm_provider_name,
        active_notice_ids,
        integrations_failing,
        notifications_enabled: settings.notifications_enabled,
        error_reporting_enabled: settings.error_reporting_enabled,
    };

    tracing::info!(
        daemon_running = ?snap.daemon_running,
        database_ready = ?snap.database_ready,
        llm_provider_ok = ?snap.llm_provider_ok,
        notices = snap.active_notice_ids.len(),
        integrations_failing = snap.integrations_failing.len(),
        "analytics: health snapshot"
    );
    snap
}

impl HealthSnapshot {
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

        if let Some(name) = &self.llm_provider_name {
            props.insert(
                "health_llm_provider".to_string(),
                Value::String(name.clone()),
            );
        }
        props.insert(
            "health_active_notices".to_string(),
            serde_json::json!(self.active_notice_ids),
        );
        props.insert(
            "health_integrations_failing".to_string(),
            serde_json::json!(self.integrations_failing),
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
mod tests {
    use super::*;

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
            props.get("health_integrations_failing"),
            Some(&serde_json::json!([] as [String; 0]))
        );
    }
}
