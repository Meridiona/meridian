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
    ///
    /// Deliberately NOT renamed or repurposed - existing dashboards read it,
    /// and it answers a different question to [`HealthSnapshot::db_integrity`]:
    /// this is "could we read it just now", which a database that is quietly
    /// corrupting still answers `true` to.
    pub database_ready: Option<bool>,
    /// The last startup integrity verdict: `verified`, `damaged`,
    /// `unopenable`, `absent`, `inconclusive`, or `unknown`.
    ///
    /// Read from a sidecar file beside the database
    /// ([`crate::repair_boot::last_probe_verdict`]), never from a row inside
    /// it - the verdict that matters most describes a database that cannot be
    /// opened, so storing it in that database would make it unreachable
    /// exactly when it is worth having. That is the same trap that keeps the
    /// `db.corrupt` NOTICE invisible on the machines it best describes, and
    /// why `active_notice_ids` alone could not answer this.
    ///
    /// `unknown` is the honest and COMMON value: the tray only probes when no
    /// daemon answers (`quick_check` reads every page, so scanning on every
    /// healthy boot would tax the machines that need nothing). A working
    /// install is therefore read from `database_ready: true` plus the ABSENCE
    /// of `db.corrupt` in `active_notice_ids`, not from this field.
    pub db_integrity: &'static str,
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
    /// The tray is registered to start at login. `None` on a platform or an
    /// install shape where the check cannot run (an unbundled dev run).
    ///
    /// This is the field that separates **churn from a broken login item**.
    /// Capture runs in-process in the tray, so an install whose registration
    /// has gone (see [`crate::autostart`]) produces no data and stops sending
    /// `app_active` — which is byte-for-byte what a user who walked away looks
    /// like. Without this, the two populations are one number.
    pub autostart_registered: Option<bool>,
    /// macOS only: SMAppService's view of the LOGIN-item registration —
    /// `enabled`, `requires_approval`, `not_registered`, `not_found`, or
    /// `unavailable` below macOS 13. `None` off macOS.
    ///
    /// This is the field that answers "is Meridian actually in Login Items &
    /// Extensions", which was unanswerable from the fleet — and it is the real
    /// question behind every "why doesn't it start" report. `requires_approval`
    /// in particular is a state only the USER can clear, so seeing it here is
    /// the difference between shipping a fix and shipping a prompt.
    pub autostart_login_item: Option<&'static str>,
    /// The registration points at the executable that is actually running.
    /// `false` means the app was moved after being registered, so it will not
    /// come back at the next login even though something IS registered.
    pub autostart_path_ok: Option<bool>,
    /// This launch had to write a registration that should already have been
    /// there — i.e. autostart was broken until now. The fleet rate of this is
    /// how the fix is verified in production.
    pub autostart_repaired: bool,
    /// What [`crate::autostart::ensure_registered`] decided, as a stable wire
    /// name — `already_correct`, `repaired_path_drift`,
    /// `skipped_disabled_by_user`, … `None` before it has run.
    pub autostart_action: Option<&'static str>,
    /// This process was started by the login/morning job rather than by the
    /// user. Distinguishes "autostart is working" from "they opened it
    /// themselves", which is otherwise unknowable.
    pub launched_by_autostart: bool,
    /// Seconds this tray process has been up. Reading `launched_by_autostart`
    /// without it is ambiguous: a long-lived process started by the login job
    /// days ago says nothing about whether autostart worked *today*.
    pub tray_uptime_s: Option<i64>,
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
            db_integrity: crate::repair_boot::VERDICT_UNKNOWN,
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
            autostart_registered: None,
            autostart_login_item: None,
            autostart_path_ok: None,
            autostart_repaired: false,
            autostart_action: None,
            launched_by_autostart: false,
            tray_uptime_s: None,
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

/// Every vendor preset [`meridian_core::settings::CustomLlmProvider::vendor`] can
/// legitimately hold — see that field's doc. Unlike `llm_provider`, this field is a
/// plain `String` with no `from_wire`-style parser, because it is provenance/display
/// only; behaviour never branches on it. That also means nothing stops a hand-edited
/// settings file (or a future bug in the UI's preset picker) from putting arbitrary
/// text here, and this list is what stands between that text and an analytics event —
/// see the module doc's fourth rule ("the model id ships only for a known vendor
/// preset").
const KNOWN_CUSTOM_VENDORS: [&str; 5] = ["groq", "openai", "gemini", "openrouter", "other"];

/// The value shipped when a configured custom endpoint's `vendor` is not one of
/// [`KNOWN_CUSTOM_VENDORS`] — mirrors [`UNRECOGNISED_PROVIDER`]'s reasoning. Never the
/// raw string.
const UNRECOGNISED_VENDOR: &str = "unrecognised";

/// The `(llm_vendor, llm_model)` pair for the snapshot, given the active custom
/// endpoint (if any) and the built-in-provider model override that applies when
/// there isn't one. Pulled out of [`snapshot`] as a pure function purely so this
/// validation is unit-testable without a pool/settings file.
fn resolve_custom_vendor_and_model(
    custom: Option<&meridian_core::settings::CustomLlmProvider>,
    builtin_provider_model: Option<String>,
) -> (Option<String>, Option<String>) {
    let Some(c) = custom else {
        // No custom endpoint active: `builtin_provider_model` is the model override
        // for the built-in CLI provider `resolve_provider` resolved, not a
        // custom-vendor fallback — see `llm_provider_model`'s doc in
        // meridian-core/src/settings.rs.
        return (None, builtin_provider_model);
    };
    if !KNOWN_CUSTOM_VENDORS.contains(&c.vendor.as_str()) {
        // Unknown vendor text: the vendor itself downgrades (never the raw hand-
        // edited string) and the model ships even less - it's exactly as
        // untrustworthy as `other`'s, just not spelled that way.
        return (Some(UNRECOGNISED_VENDOR.to_string()), None);
    }
    let model = if c.vendor == "other" {
        // A hand-entered endpoint's model can name an internal deployment.
        None
    } else {
        Some(c.model.clone())
    };
    (Some(c.vendor.clone()), model)
}

/// Whether the user has ever actually CHOSEN an AI provider, as opposed to
/// silently inheriting the default.
///
/// # Why this can't be inferred from `llm_provider`
/// [`meridian_core::settings::RuntimeSettings::default`] sets `llm_provider` to
/// `claude`, and `load_runtime_settings` merges the file over those defaults.
/// So a user who has never opened the provider picker reports exactly the same
/// value as one who deliberately picked Claude — and on a fleet dashboard the
/// first group is large, newly-installed, and the population you most want to
/// find, because "provider = claude, provider_ok = false" is usually "never set
/// up" rather than "Claude is broken".
///
/// The only place that distinction survives is the raw settings file: the key
/// is present iff something wrote it. So this reads the file directly rather
/// than going through [`meridian_core::settings::read_settings_value`], which
/// has already merged the defaults in and cannot tell the two apart.
///
/// An unreadable or unparseable file reads as "not chosen" — the same answer a
/// fresh install gives, and the safer of the two when we cannot tell.
fn provider_explicitly_chosen() -> bool {
    let path = meridian_core::settings::settings_json_path();
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return false;
    };
    serde_json::from_str::<serde_json::Value>(&raw)
        .ok()
        .and_then(|v| v.get("llm_provider").cloned())
        .is_some_and(|v| v.is_string())
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
    let (llm_vendor, llm_model) =
        resolve_custom_vendor_and_model(custom, settings.llm_provider_model.clone());

    // Autostart is probed LIVE rather than cached from startup: the point of
    // shipping it is to catch a registration that has gone missing, and one that
    // went missing after this process started is exactly the case a cached value
    // could not see.
    let autostart = crate::autostart::status().await;
    let action = crate::autostart::last_action();

    let snap = HealthSnapshot {
        daemon_running: h.daemon_running,
        database_ready: h.database_ready,
        db_integrity: crate::repair_boot::last_probe_verdict(std::path::Path::new(
            &crate::install::meridian_db_path(),
        ))
        .unwrap_or(crate::repair_boot::VERDICT_UNKNOWN),
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
        autostart_registered: autostart.registered,
        autostart_login_item: autostart.login_item,
        autostart_path_ok: autostart.path_ok,
        autostart_repaired: action.is_some_and(|a| a.is_repair()),
        autostart_action: action.map(|a| a.as_str()),
        launched_by_autostart: crate::autostart::launched_by_autostart(),
        tray_uptime_s: crate::sys::uptime_secs(),
    };

    tracing::info!(
        daemon_running = ?snap.daemon_running,
        autostart_registered = ?snap.autostart_registered,
        autostart_login_item = ?snap.autostart_login_item,
        autostart_path_ok = ?snap.autostart_path_ok,
        autostart_action = ?snap.autostart_action,
        launched_by_autostart = snap.launched_by_autostart,
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

    /// The subset of this snapshot that belongs on the PERSON rather than the
    /// event: slowly-changing current state — which AI provider, which
    /// trackers — that you want as a sortable column on the Persons list
    /// rather than as something recoverable only by querying each person's
    /// latest event. See [`super::base_person_properties`] for the general
    /// rule, and for why `support_id` is excluded from this treatment.
    ///
    /// Deliberately NOT included: anything momentary (`daemon_running`,
    /// `database_ready`, rate-limit state, active notices, per-tracker sync
    /// status). Those change hour to hour, and a person property keeps only the
    /// newest value — so "is the daemon running" as a person property would
    /// answer "was it running at the last send", which reads as current fact
    /// and is not one. They stay event properties, where they are timestamped.
    ///
    /// `unset` collects the names of any optional field that is `None` on THIS
    /// snapshot, so the caller can send them as PostHog's `$unset` alongside
    /// `$set` — see `super::attach_person_properties`. Without it, a user who
    /// switches away from a custom endpoint (clearing `llm_vendor`/`llm_model`)
    /// or whose vendor stops resolving would keep their PREVIOUS value forever:
    /// `$set` only overwrites keys that are present, so an omitted key is not
    /// "cleared", it is simply never mentioned again.
    pub(crate) fn write_person_properties(
        &self,
        person: &mut Map<String, Value>,
        unset: &mut Vec<String>,
    ) {
        person.insert(
            "llm_provider".to_string(),
            Value::String(self.llm_provider.to_string()),
        );
        person.insert(
            "llm_provider_chosen".to_string(),
            Value::Bool(provider_explicitly_chosen()),
        );
        match &self.llm_vendor {
            Some(v) => {
                person.insert("llm_vendor".to_string(), Value::String(v.clone()));
            }
            None => unset.push("llm_vendor".to_string()),
        }
        match &self.llm_model {
            Some(m) => {
                person.insert("llm_model".to_string(), Value::String(m.clone()));
            }
            None => unset.push("llm_model".to_string()),
        }
        // The ONE health field that belongs on the person rather than only on
        // the event, and the reason is the rule this doc states above rather
        // than an exception to it: `daemon_running` is excluded because it
        // changes hour to hour, so "newest value only" would misread as
        // current fact. Database damage is the opposite - it LATCHES. It
        // cannot clear without an operator running a repair, so the newest
        // value IS the current fact, for weeks at a time.
        //
        // It has to be here to be useful at all. A damaged install often
        // cannot send `daily_usage` (that event needs a DB read, and
        // `analytics::daily` returns early when it fails), so the event-only
        // copy never arrives from exactly the machines worth finding.
        // `app_active` carries person properties and needs no DB read, so this
        // is the value that actually escapes a broken install.
        person.insert(
            "db_integrity".to_string(),
            Value::String(self.db_integrity.to_string()),
        );
        person.insert(
            "integrations_connected".to_string(),
            serde_json::json!(self.integrations_connected),
        );
        person.insert(
            "integrations_count".to_string(),
            serde_json::json!(self.integrations_connected.len()),
        );
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
        // Also a person property (see `write_person_properties`); kept here too
        // so a query can see WHEN the verdict changed, which the person copy
        // cannot show - it keeps only the newest value.
        props.insert(
            "health_db_integrity".to_string(),
            Value::String(self.db_integrity.to_string()),
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

        // Autostart. These are EVENT properties, never person properties: each
        // one describes one launch of one process, and a person property keeps
        // only the newest value, which would read as "this user's autostart is
        // fine" on the strength of whichever machine reported last.
        // Written long-hand rather than through `put_bool` above: that closure
        // holds a mutable borrow of `props`, and reviving it here would conflict
        // with the plain `insert`s in between.
        if let Some(b) = self.autostart_registered {
            props.insert("health_autostart_registered".to_string(), Value::Bool(b));
        }
        if let Some(b) = self.autostart_path_ok {
            props.insert("health_autostart_path_ok".to_string(), Value::Bool(b));
        }
        if let Some(s) = self.autostart_login_item {
            props.insert(
                "health_autostart_login_item".to_string(),
                Value::String(s.to_string()),
            );
        }
        props.insert(
            "health_autostart_repaired".to_string(),
            Value::Bool(self.autostart_repaired),
        );
        if let Some(a) = self.autostart_action {
            props.insert(
                "health_autostart_action".to_string(),
                Value::String(a.to_string()),
            );
        }
        props.insert(
            "health_launched_by_autostart".to_string(),
            Value::Bool(self.launched_by_autostart),
        );
        if let Some(u) = self.tray_uptime_s {
            props.insert("health_tray_uptime_s".to_string(), serde_json::json!(u));
        }
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

    fn custom_provider(vendor: &str, model: &str) -> meridian_core::settings::CustomLlmProvider {
        meridian_core::settings::CustomLlmProvider {
            id: "cust1".to_string(),
            vendor: vendor.to_string(),
            name: "My endpoint".to_string(),
            base_url: "https://example.com/v1".to_string(),
            model: model.to_string(),
            api_key: "sk-should-never-appear-in-this-test".to_string(),
            rpm: 0,
            rpd: 0,
            rungs: Default::default(),
        }
    }

    /// A known preset ships both its vendor and its model.
    #[test]
    fn a_known_vendor_ships_vendor_and_model() {
        let p = custom_provider("groq", "openai/gpt-oss-120b");
        assert_eq!(
            resolve_custom_vendor_and_model(Some(&p), None),
            (
                Some("groq".to_string()),
                Some("openai/gpt-oss-120b".to_string())
            )
        );
    }

    /// A hand-entered (`other`) endpoint's model can name an internal deployment,
    /// so only the vendor survives - the vendor id itself is still a closed-set
    /// preset value.
    #[test]
    fn a_hand_entered_vendor_ships_no_model() {
        let p = custom_provider("other", "acme-internal-gpt4");
        assert_eq!(
            resolve_custom_vendor_and_model(Some(&p), None),
            (Some("other".to_string()), None)
        );
    }

    /// The whole point of this fix: a vendor string that is not one of
    /// KNOWN_CUSTOM_VENDORS - a hand-edited settings file, or a future UI bug that
    /// lets free text through - must never reach the analytics event, neither as
    /// itself nor via the model it would otherwise unlock.
    #[test]
    fn an_unrecognised_vendor_ships_neither_vendor_nor_model() {
        let p = custom_provider("totally-made-up-vendor", "some-internal-model-name");
        assert_eq!(
            resolve_custom_vendor_and_model(Some(&p), None),
            (Some(UNRECOGNISED_VENDOR.to_string()), None),
            "an unrecognised vendor must downgrade, not pass its raw string or model through"
        );
    }

    /// No custom endpoint active: the built-in provider's own model override ships
    /// instead, and no vendor does (there is no custom vendor to report).
    #[test]
    fn no_custom_endpoint_falls_back_to_the_builtin_providers_model() {
        assert_eq!(
            resolve_custom_vendor_and_model(None, Some("claude-sonnet-5".to_string())),
            (None, Some("claude-sonnet-5".to_string()))
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

#[cfg(test)]
mod person_property_tests {
    //! What lands on the PostHog Person rather than the event. These pin the
    //! split, because getting it wrong is silent: a momentary value promoted to
    //! a person property reads as current fact forever, and a changing value
    //! promoted there loses its own history.
    use super::*;

    fn snap() -> HealthSnapshot {
        HealthSnapshot {
            llm_provider: "custom",
            llm_vendor: Some("groq".to_string()),
            llm_model: Some("openai/gpt-oss-120b".to_string()),
            integrations_connected: vec!["jira".into(), "github".into()],
            daemon_running: Some(false),
            database_ready: Some(true),
            ..Default::default()
        }
    }

    /// The provider and tracker set are the whole point: they must be person
    /// properties so PostHog shows them as a column, not values you have to dig
    /// out of each person's most recent event.
    #[test]
    fn provider_and_trackers_land_on_the_person() {
        let mut person = Map::new();
        snap().write_person_properties(&mut person, &mut Vec::new());

        assert_eq!(
            person.get("llm_provider"),
            Some(&Value::String("custom".to_string()))
        );
        assert_eq!(
            person.get("llm_vendor"),
            Some(&Value::String("groq".to_string()))
        );
        assert_eq!(
            person.get("integrations_connected"),
            Some(&serde_json::json!(["jira", "github"]))
        );
        assert_eq!(
            person.get("integrations_count"),
            Some(&serde_json::json!(2))
        );
    }

    /// Momentary state must NOT be promoted to the person. A person property
    /// keeps only the newest value, so `daemon_running` there would answer
    /// "was it up at the last send" while reading as "is it up" — the kind of
    /// wrong that looks right on a dashboard.
    #[test]
    fn momentary_state_stays_off_the_person() {
        let mut person = Map::new();
        snap().write_person_properties(&mut person, &mut Vec::new());

        for k in [
            "daemon_running",
            "database_ready",
            "llm_provider_rate_limited",
            "active_notices",
            "integrations_status",
            "health_observed_on",
        ] {
            assert!(
                !person.contains_key(k),
                "{k} is momentary and must stay an event property"
            );
        }

        // `db_integrity` IS on the person, and the distinction is the rule
        // above rather than an exception to it: database damage LATCHES - it
        // cannot clear without an operator running a repair - so "the newest
        // value" is the current fact for weeks, unlike `daemon_running`.
        // `database_ready` stays off precisely because it is the momentary
        // half of the same subject: it answers "could we read it just now".
        assert!(person.contains_key("db_integrity"));
        assert!(!person.contains_key("database_ready"));
    }

    /// The verdict must reach PostHog as a PERSON property, not only as an
    /// event property.
    ///
    /// A damaged install often cannot send `daily_usage` at all - that event
    /// needs a DB read and `analytics::daily` returns early when it fails - so
    /// the event-only copy never arrives from exactly the machines worth
    /// finding. `app_active` carries person properties and needs no DB read.
    /// If this ever moves to event-only, corrupt installs go silent again.
    #[test]
    fn db_integrity_reaches_posthog_as_a_person_property() {
        let snap = HealthSnapshot {
            db_integrity: crate::repair_boot::VERDICT_DAMAGED,
            ..Default::default()
        };
        let mut person = Map::new();
        let mut unset = Vec::new();
        snap.write_person_properties(&mut person, &mut unset);
        assert_eq!(
            person.get("db_integrity").and_then(Value::as_str),
            Some(crate::repair_boot::VERDICT_DAMAGED),
            "a damaged install must be findable on the PostHog Persons list"
        );

        // And on the event too, where it is timestamped by health_observed_on.
        let mut props = Map::new();
        snap.write_properties(&mut props);
        assert_eq!(
            props.get("health_db_integrity").and_then(Value::as_str),
            Some(crate::repair_boot::VERDICT_DAMAGED)
        );
    }

    /// An install nobody scanned must not read as verified-healthy.
    #[test]
    fn an_unprobed_install_reports_unknown_not_healthy() {
        let snap = HealthSnapshot::default();
        let mut person = Map::new();
        let mut unset = Vec::new();
        snap.write_person_properties(&mut person, &mut unset);
        assert_eq!(
            person.get("db_integrity").and_then(Value::as_str),
            Some(crate::repair_boot::VERDICT_UNKNOWN)
        );
        assert_ne!(
            person.get("db_integrity").and_then(Value::as_str),
            Some(crate::repair_boot::VERDICT_VERIFIED)
        );
    }

    /// `support_id` must never become a person property. It legitimately
    /// changes (a second machine, a re-seed, the alpha window ending), and a
    /// person property keeps only the newest — which would silently orphan
    /// every error row recorded under the previous one. As an event property a
    /// DISTINCT recovers the full set.
    #[test]
    fn support_id_is_never_a_person_property() {
        let mut person = Map::new();
        snap().write_person_properties(&mut person, &mut Vec::new());
        assert!(!person.contains_key("support_id"));
    }

    /// A snapshot WITH a vendor/model must not queue either for unset - only an
    /// absent value is stale, a present one is not.
    #[test]
    fn a_present_vendor_and_model_are_never_queued_for_unset() {
        let mut person = Map::new();
        let mut unset = Vec::new();
        snap().write_person_properties(&mut person, &mut unset);
        assert!(unset.is_empty());
    }

    /// The whole point of this fix: a user who moves off a custom endpoint (back
    /// to a built-in provider, or to one CodeRabbit's earlier fix downgrades to
    /// `unrecognised`) has `llm_vendor`/`llm_model` go from `Some` to `None`. A
    /// bare `$set` would leave the PostHog person carrying the stale value
    /// forever - `write_person_properties` must queue both names for `$unset`.
    #[test]
    fn a_cleared_vendor_and_model_are_queued_for_unset() {
        let snap = HealthSnapshot {
            llm_vendor: None,
            llm_model: None,
            ..snap()
        };
        let mut person = Map::new();
        let mut unset = Vec::new();
        snap.write_person_properties(&mut person, &mut unset);
        assert!(!person.contains_key("llm_vendor"));
        assert!(!person.contains_key("llm_model"));
        assert_eq!(
            unset,
            vec!["llm_vendor".to_string(), "llm_model".to_string()]
        );
    }
}
