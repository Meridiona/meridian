//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Runtime settings (`~/.meridian/settings.json`) — the hot-reloadable subset
//! of config the dashboard writes and the daemon reads each poll tick.
//!
//! # What this is
//! The single source of truth for the settings schema + path. Lives here (not
//! the daemon) so all three consumers share ONE definition: the daemon
//! re-exports it (its `config::{RuntimeSettings, load_runtime_settings}` are
//! unchanged), and the Tauri app reads it directly — no reimplementation.
//!
//! # Who calls this
//! - The daemon: `src/config.rs` re-exports these; `src/observability.rs` reads
//!   them each poll tick to drive log level + OTLP export.
//! - The Tauri app: the tray `get_settings` command (the ported `/api/settings`
//!   GET) serialises this, redacting `oo_password`.
//!
//! The PUT (write) side is [`read_settings_value`] + [`write_settings_value`]:
//! these work on the raw JSON object (not the typed struct) so a write preserves
//! any keys not in [`RuntimeSettings`] — a faithful match of the route's
//! `{ ...current, ...body }` merge — and the write is crash-safe (temp + rename).
//!
//! # Related
//! - `ui/lib/settings.ts` is the TS mirror of this schema + defaults — keep the
//!   two in sync (the `SETTINGS_DEFAULTS` there must match [`RuntimeSettings::default`]).
//! - The tray `update_settings` command (ported `/api/settings` PUT) validates +
//!   merges the body on top of [`read_settings_value`] and persists via
//!   [`write_settings_value`].

use crate::paths::{expand_tilde, home_dir};
use anyhow::Context;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;

/// How well an endpoint honoured a structured-output request, measured — never assumed.
///
/// "OpenAI-compatible" endpoints genuinely disagree about the same schema: OpenAI rejects
/// a schema whose `required` omits an optional key (400 `invalid_json_schema`), while
/// Gemini's compat endpoint accepts it. Both facts were measured with real requests. So a
/// hardcoded per-vendor table would be wrong for someone on day one — and could never
/// answer for a hand-entered base URL. Instead each endpoint is probed once when it is
/// added, and what it actually did is recorded here.
///
/// Ordered worst-to-best so the weakest rung across the pipeline's schemas is `min()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SchemaRung {
    /// The endpoint refused every mode, or has never been probed. Nothing may rely on it.
    None,
    /// No enforcement: the schema rides in the prompt and the answer is parsed tolerantly.
    /// The shipped Cursor/Copilot contract — workable, but shape is not guaranteed.
    Prompt,
    /// `json_object` — valid JSON guaranteed, shape is not.
    JsonObject,
    /// `json_schema` accepted; shape enforced in the vendor's own dialect.
    JsonSchema,
    /// `json_schema` + `strict: true` — the strongest available.
    Strict,
}

impl SchemaRung {
    /// May the worklog pipeline run on an endpoint measured at this rung?
    ///
    /// Shape-enforcing rungs only. Below that the answer's *shape* is unguaranteed, and a
    /// malformed fold does not fail loudly — it silently drops an hour of the user's day.
    /// The LLM Lab is where an unenforced endpoint earns its promotion by being measured
    /// against the others; production is not the place to find out.
    pub fn is_production_eligible(self) -> bool {
        self >= SchemaRung::JsonSchema
    }
}

/// Every schema the worklog pipeline asks a provider to answer in — the key set a
/// measurement must cover before [`CustomLlmProvider::effective_rung`] will call an
/// endpoint measured at all.
///
/// It lives HERE, next to the gate that enforces it, rather than beside the schemas
/// themselves: the gate's whole job is to know what a COMPLETE measurement looks like, and
/// it must be able to say "you never tried that one" without depending on the daemon's LLM
/// code (which this crate cannot see). The daemon's `llm::probe` builds its work list from
/// this list, so adding a schema to the pipeline forces both to move together.
pub const PIPELINE_SCHEMA_KEYS: [&str; 4] = [
    "activity_report",
    "workstream",
    "worklog_generate",
    "plan_task_draft",
];

/// One user-configured cloud endpoint — **the storage form, and the only place the API key
/// lives**.
///
/// # This struct must never cross to the UI or telemetry
///
/// It carries `api_key` in cleartext, exactly as `oo_password` does, because the daemon has
/// to send it. The tray's `get_settings` redacts it per row on the way out; nothing else may
/// serialise this type outward. Anything the frontend or a span needs gets a keyless shape
/// built for that purpose — one type doing double duty is how a key leaks the first time
/// someone forgets to redact.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CustomLlmProvider {
    /// Stable id, generated on add. The registry key — `custom:<id>` addresses this row,
    /// and the redact/restore round-trip matches on it.
    pub id: String,
    /// Which preset this came from (`"openai"`, `"gemini"`, `"openrouter"`, … or `"other"`
    /// for a hand-entered base URL). Display + provenance only; behaviour comes from the
    /// measured rungs, never from this.
    pub vendor: String,
    /// What the user calls it, shown on the card.
    pub name: String,
    /// OpenAI-compatible base URL, e.g. `https://generativelanguage.googleapis.com/v1beta/openai`.
    /// `/chat/completions` is appended by the backend.
    pub base_url: String,
    /// The model id to send. Required — unlike a CLI provider there is no "default model".
    pub model: String,
    /// Sent as `Authorization: Bearer`. See the type docs: storage only.
    pub api_key: String,
    /// Requests-per-minute ceiling this endpoint's plan allows. `0` (the default) means
    /// "unmetered as far as we know" and disables pacing entirely.
    ///
    /// Free tiers are the reason this exists: a 15-RPM key cannot survive [`crate`]'s probe
    /// burst, which fires one metered request per schema × rung back-to-back. Pacing to this
    /// number turns "429s partway through, endpoint never becomes gate-eligible" into "takes
    /// a minute longer, completes".
    ///
    /// Deliberately requests-only for now. A token-per-minute ceiling is the other half of
    /// most free tiers and frequently binds first on long worklog prompts, but pacing it
    /// needs a token estimate before the call; until then TPM is handled REACTIVELY, from the
    /// `x-ratelimit-reset-tokens` header on a 429. See `src/llm/rate_limit.rs`.
    #[serde(default)]
    pub rpm: u32,
    /// Requests-per-DAY ceiling this endpoint's plan allows. `0` (the default) means
    /// "not known" — never "zero allowed" — so a user who leaves it blank is told we
    /// cannot judge, not that their key is bad.
    ///
    /// Advisory only: nothing paces against it. Unlike [`Self::rpm`], a daily quota has no
    /// remedy in software — pacing can spread requests across a minute, but it cannot
    /// conjure a 21st request out of a 20-a-day key. All this field buys is the ability to
    /// TELL the user before they rely on the endpoint, which is why it feeds
    /// [`crate::llm_capacity::assess`] and nothing else.
    ///
    /// User-entered because it is not discoverable: RPM occasionally arrives in an
    /// `x-ratelimit-limit-requests` header, but a daily ceiling essentially never does, and
    /// inferring one from a 429 would mean learning it only after burning the day's quota.
    #[serde(default)]
    pub rpd: u32,
    /// The rung measured for EACH pipeline schema, keyed by schema name — not one scalar.
    /// The schemas differ in what they demand (`worklog_generate` carries a union type and
    /// deeper nesting than `workstream`), and a vendor's support is not guaranteed uniform
    /// across them, so a single number would hide the weakest link. Empty = never probed.
    #[serde(default)]
    pub rungs: std::collections::BTreeMap<String, SchemaRung>,
}

impl CustomLlmProvider {
    /// The weakest rung across the pipeline's schemas — the honest answer to "how well does
    /// this endpoint hold a contract", because the pipeline sends all of them.
    ///
    /// [`SchemaRung::None`] unless EVERY key in [`PIPELINE_SCHEMA_KEYS`] has been measured.
    /// A `min()` over whatever happens to be present would let a partial probe pass: a rate
    /// limit halfway through leaves three `Strict` rungs and one schema never tried, and
    /// "the three I got round to are excellent" is not evidence about the fourth. Unmeasured
    /// is not innocent — including when it is unmeasured only because we ran out of quota.
    pub fn effective_rung(&self) -> SchemaRung {
        PIPELINE_SCHEMA_KEYS
            .iter()
            .map(|k| self.rungs.get(*k).copied().unwrap_or(SchemaRung::None))
            .min()
            .unwrap_or(SchemaRung::None)
    }

    /// Schemas still to measure — what a resumed probe should spend requests on.
    ///
    /// A probe attempt costs real money, so a retry after a rate limit must not re-buy an
    /// answer already on the row.
    pub fn unmeasured_schemas(&self) -> Vec<&'static str> {
        PIPELINE_SCHEMA_KEYS
            .iter()
            .copied()
            .filter(|k| !self.rungs.contains_key(*k))
            .collect()
    }

    /// Has every pipeline schema been measured? `false` = the probe never finished.
    pub fn is_fully_probed(&self) -> bool {
        self.unmeasured_schemas().is_empty()
    }

    /// May this endpoint be the production provider? See [`SchemaRung::is_production_eligible`].
    pub fn is_production_eligible(&self) -> bool {
        self.effective_rung().is_production_eligible()
    }
}

/// Settings changeable at runtime by editing `settings.json`. The daemon
/// re-reads on every poll tick (no restart). `Serialize` is added (the daemon
/// only needs `Deserialize`) so the dashboard command can return it as JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RuntimeSettings {
    // Appearance — the dashboard's selected surface palette (lilac/blush/ink).
    // `#[serde(default = "default_theme")]` so an existing settings.json without
    // the key upgrades cleanly (no migration). Mirrors `theme` in ui/lib/settings.ts.
    #[serde(default = "default_theme")]
    pub theme: String,
    pub log_level: String,
    pub agent_auto_floor: f64,
    pub agent_queue_floor: f64,
    // Which AI runs the user's prose LLM calls — the wire form of
    // `crate::llm_provider::LlmProvider` ("claude" | "codex" | "cursor" | "copilot" |
    // "custom"). Deliberately a `String`, NOT the enum: `load_runtime_settings` falls
    // back to `Default` on any deserialise error, so a variant written by a newer build
    // would make an older daemon lose EVERY setting (work hours, poll interval, …), not
    // just this one. Parsed with `LlmProvider::from_wire`, which degrades an unknown
    // value to the default and takes nothing else down with it.
    pub llm_provider: String,
    // Which custom endpoint is active, when `llm_provider == "custom"`. The enum names the
    // KIND; this names the instance (there can be several configured). Ignored for every
    // other provider. An id with no matching row degrades exactly like an unknown
    // `llm_provider` — the call fails loudly rather than silently running somewhere else.
    pub llm_provider_custom_id: Option<String>,
    // The user's configured cloud endpoints. Carries API KEYS in cleartext — see
    // `CustomLlmProvider`; the tray redacts per row on read, and nothing else may
    // serialise them outward.
    #[serde(default)]
    pub custom_llm_providers: Vec<CustomLlmProvider>,
    // Optional model override within the chosen provider (`--model` for claude/cursor,
    // `-m` for codex). None → the provider's own default. A custom endpoint ignores this:
    // its model is a required field on its own row, because there is no default to fall
    // back to.
    pub llm_provider_model: Option<String>,
    pub llm_budget_pct: f64,
    pub poll_interval_secs: u64,
    pub jira_update_enabled: bool,
    // OpenObserve OTLP export — all three must be non-empty for export to
    // activate. settings.json is the ONLY credential source (MERIDIAN_OO_AUTH is
    // deprecated/ignored); otlp_endpoint still falls back to MERIDIAN_OTLP_ENDPOINT.
    pub otlp_enabled: bool,
    pub otlp_endpoint: Option<String>,
    pub oo_email: Option<String>,
    pub oo_password: Option<String>,
    // Notification preferences — the master switch + per-type toggles + quiet
    // hours. Read by [`crate::notifications`] (the policy ported from
    // ui/lib/notifications.ts) to decide whether an event may surface.
    // `quiet_hours_*` are 'HH:MM' local time (start inclusive, end exclusive).
    pub notifications_enabled: bool,
    pub notify_plan_nudge: bool,
    // Daily planner auto-open — once per local day the tray opens the dashboard
    // on the Plan modal (first launch or first poll tick of a new day). A window
    // behaviour, not a toast, so it is NOT gated by `notifications_enabled`.
    pub auto_open_plan: bool,
    pub notify_worklog_ready: bool,
    pub notify_system_fault: bool,
    // Folded from direct tray-side toasts (pause/resume, daemon health,
    // updates) into the outbox — each now has its own toggle instead of
    // sharing `notify_system_fault` or being ungated entirely.
    pub notify_system_pause: bool,
    pub notify_system_health: bool,
    pub notify_system_update: bool,
    // Daily batched digests — see src/coding_agent_session_ingest and
    // src/intelligence::triage_after_sync.
    pub notify_summariser_digest: bool,
    pub notify_board_hygiene: bool,
    pub quiet_hours_enabled: bool,
    pub quiet_hours_start: String,
    pub quiet_hours_end: String,
    // Work hours — capture is paused automatically outside this window.
    pub work_hours_enabled: bool,
    pub work_hours_start: String, // "HH:MM" local time, inclusive
    pub work_hours_end: String,   // "HH:MM" local time, exclusive
    pub work_days: String,        // comma-separated 1–7 (Mon=1 … Sun=7), e.g. "1,2,3,4,5"
    // Pause capture when streaming video (Netflix, Disney+, etc.) is playing.
    pub pause_on_streaming_video: bool,
    // User capture ignore lists. `ignored_apps` matches an incoming frame's
    // `app_name` exactly (case-insensitive); `ignored_urls` matches the focused
    // tab's `browser_url` HOST at domain granularity (a domain also matches its
    // subdomains, e.g. "youtube.com" covers "www.youtube.com"). A match is
    // dropped before it is written to `capture_frames`/`capture_ui_events`, so
    // the app/site never appears on the timeline — GOING FORWARD ONLY; already
    // captured history is left untouched. Empty = nothing ignored. Enforced by
    // `tray::capture_ignore::CaptureIgnore` at the frame consumer. The
    // struct-level `#[serde(default)]` upgrades an existing settings.json that
    // predates these keys to empty lists. Mirrors ui/lib/settings.ts.
    pub ignored_apps: Vec<String>,
    pub ignored_urls: Vec<String>,
    // Worklog auto-generation — once a day, at this local "HH:MM" clock time,
    // Meridian scans today's day-tasks and auto-DRAFTS (never approves/posts) a
    // worklog for any with more than `pm_worklog::auto_generate`'s fixed 30-minute
    // qualifying threshold that doesn't have a draft yet. `None` means the user
    // hasn't chosen a time yet (never asked, or explicitly skipped) — in that
    // state nothing is ever auto-drafted, only the manual "Generate worklog"
    // button as before. Read by `pm_worklog::auto_generate` on the worklog
    // pipeline's clock tick. Mirrors `ui/lib/settings.ts`.
    pub worklog_auto_generate_time: Option<String>,
    // Has the user been asked about auto-generation at least once (the one-time
    // Timeline nudge, or a manual Settings visit)? Distinguishes "never asked"
    // from "asked and skipped" — both leave `worklog_auto_generate_time` at
    // `None`, but only the latter should never be asked again. `#[serde(default)]`
    // at the struct level makes this `false` for every settings.json written before
    // this field existed, which is exactly the "reprompt existing users once" case.
    pub worklog_auto_generate_prompted: bool,
}

/// Default surface palette when `settings.json` predates the `theme` key. Must
/// match `SETTINGS_DEFAULTS.theme` in ui/lib/settings.ts.
fn default_theme() -> String {
    "lilac".to_string()
}

impl Default for RuntimeSettings {
    fn default() -> Self {
        Self {
            theme: default_theme(),
            log_level: "INFO".to_string(),
            agent_auto_floor: 0.65,
            agent_queue_floor: 0.40,
            // The user's chosen AI CLI runs every prose LLM call (default: Claude).
            llm_provider: crate::llm_provider::LlmProvider::default()
                .as_str()
                .to_string(),
            llm_provider_custom_id: None,
            custom_llm_providers: Vec::new(),
            llm_provider_model: None,
            llm_budget_pct: 0.5,
            poll_interval_secs: 60,
            jira_update_enabled: true,
            // OpenObserve export is opt-in — off until enabled in Settings (the UI
            // default in ui/lib/settings.ts must match this).
            otlp_enabled: false,
            otlp_endpoint: None,
            oo_email: None,
            oo_password: None,
            // Notifications on by default; quiet hours off (22:00–08:00 when
            // enabled). Must match SETTINGS_DEFAULTS in ui/lib/settings.ts.
            notifications_enabled: true,
            notify_plan_nudge: true,
            auto_open_plan: true,
            notify_worklog_ready: true,
            notify_system_fault: true,
            notify_system_pause: true,
            notify_system_health: true,
            notify_system_update: true,
            notify_summariser_digest: true,
            notify_board_hygiene: true,
            quiet_hours_enabled: false,
            quiet_hours_start: "22:00".to_string(),
            quiet_hours_end: "08:00".to_string(),
            work_hours_enabled: false,
            work_hours_start: "09:00".to_string(),
            work_hours_end: "18:00".to_string(),
            work_days: "1,2,3,4,5".to_string(),
            // On by default — streaming video is never work signal, and DRM
            // services black out the screen for any recorder anyway. Must
            // match SETTINGS_DEFAULTS in ui/lib/settings.ts.
            pause_on_streaming_video: true,
            // Nothing ignored by default. Must match SETTINGS_DEFAULTS in
            // ui/lib/settings.ts.
            ignored_apps: Vec::new(),
            ignored_urls: Vec::new(),
            // Nobody is auto-drafted until they say so. Must match
            // SETTINGS_DEFAULTS in ui/lib/settings.ts.
            worklog_auto_generate_time: None,
            worklog_auto_generate_prompted: false,
        }
    }
}

/// Resolve `settings.json` — must agree with the dashboard (`ui/lib/settings.ts`)
/// or "Apply" never reaches the daemon. Resolution order:
///   1. `MERIDIAN_SETTINGS_PATH` override (tests / non-standard installs)
///   2. `~/.meridian/settings.json` — canonical home, next to `meridian.db`
///   3. `<cwd>/settings.json` — legacy fallback, only when the canonical is absent
///
/// cwd-relative resolution is avoided for the canonical path because the daemon's
/// working directory varies by install type (repo root vs `~/.meridian/app`).
pub fn settings_json_path() -> PathBuf {
    if let Ok(p) = std::env::var("MERIDIAN_SETTINGS_PATH") {
        if !p.is_empty() {
            let p = expand_tilde(&p);
            tracing::debug!(source = "env_override", path = %p.display(), "settings_json_path resolved");
            return p;
        }
    }

    let canonical = home_dir().map(|home| home.join(".meridian").join("settings.json"));

    if let Some(path) = &canonical {
        if path.exists() {
            tracing::debug!(source = "canonical", path = %path.display(), "settings_json_path resolved");
            return path.clone();
        }
    }

    let cwd_path = std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("settings.json");
    if cwd_path.exists() {
        tracing::debug!(source = "cwd_fallback", path = %cwd_path.display(), "settings_json_path resolved");
        return cwd_path;
    }

    // Neither exists yet — default to the canonical path so a first write lands
    // in the right, install-independent place.
    let p = canonical.unwrap_or(cwd_path);
    tracing::debug!(source = "default_not_yet_created", path = %p.display(), "settings_json_path resolved");
    p
}

impl RuntimeSettings {
    /// The active custom endpoint, when `llm_provider == "custom"` and its id resolves.
    ///
    /// `None` for any break in that chain — a different provider, no id, or an id naming a
    /// row that no longer exists (a deleted endpoint the setting still points at). The
    /// caller must treat `None` as "not configured" and fail; resolving to some *other*
    /// row, or quietly to the on-device model, would run the user's day through a provider
    /// they did not choose.
    pub fn active_custom_provider(&self) -> Option<&CustomLlmProvider> {
        if crate::llm_provider::LlmProvider::from_wire(&self.llm_provider)
            != Some(crate::llm_provider::LlmProvider::Custom)
        {
            return None;
        }
        self.custom_provider(self.llm_provider_custom_id.as_deref()?)
    }

    /// Look one endpoint up by id — the only supported way, so no caller hand-rolls the
    /// match and drifts.
    pub fn custom_provider(&self, id: &str) -> Option<&CustomLlmProvider> {
        self.custom_llm_providers.iter().find(|p| p.id == id)
    }
}

/// Load `settings.json`, falling back to defaults if absent or unparseable.
pub fn load_runtime_settings() -> RuntimeSettings {
    let path = settings_json_path();
    match std::fs::read_to_string(&path) {
        Ok(s) => serde_json::from_str(&s).unwrap_or_else(|e| {
            tracing::warn!(
                path = %path.display(),
                error = %e,
                "settings.json parse error — using defaults"
            );
            RuntimeSettings::default()
        }),
        Err(_) => RuntimeSettings::default(), // file doesn't exist yet — fine
    }
}

/// The 3 string-typed fields the daemon stores as `Option` (serialised to JSON
/// `null` when unset). The dashboard's `readSettings` coerces these `null → ''`
/// so TS consumers never hit a null on a string field — we mirror that here.
const NULLABLE_STRING_FIELDS: [&str; 3] = ["otlp_endpoint", "oo_email", "oo_password"];

/// Read the current settings as a raw JSON object — defaults overlaid with the
/// on-disk file (file wins), with the nullable string fields coerced `null → ''`.
/// A faithful mirror of `readSettings()`: working on the `Value` (not the typed
/// struct) preserves any extra keys the file carries, so a subsequent write can
/// round-trip them. Returns defaults alone when the file is absent/unparseable.
pub fn read_settings_value() -> Value {
    let mut v = serde_json::to_value(RuntimeSettings::default())
        .unwrap_or_else(|_| Value::Object(Default::default()));
    let path = settings_json_path();
    if let Ok(s) = std::fs::read_to_string(&path) {
        match serde_json::from_str::<Value>(&s) {
            Ok(Value::Object(file)) => {
                if let Some(obj) = v.as_object_mut() {
                    for (k, val) in file {
                        obj.insert(k, val); // file key overrides the default
                    }
                }
            }
            Ok(_) => tracing::warn!(path = %path.display(), "settings.json is not an object"),
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "settings.json parse error")
            }
        }
    }
    if let Some(obj) = v.as_object_mut() {
        for k in NULLABLE_STRING_FIELDS {
            if obj.get(k).map(Value::is_null).unwrap_or(false) {
                obj.insert(k.to_string(), Value::String(String::new()));
            }
        }
    }
    v
}

/// Persist a settings object to `settings.json`, crash-safely. Writes pretty
/// 2-space JSON (matching the dashboard's `JSON.stringify(x, null, 2)`) to a
/// temp file in the same dir, then atomically renames it over the target — so a
/// crash mid-write can never truncate the file holding the OO credentials (an
/// improvement over the route's plain `writeFileSync`, per the config-safety
/// practice). Creates the parent dir on first write.
pub fn write_settings_value(settings: &Value) -> anyhow::Result<()> {
    let path = settings_json_path();
    crate::fs_utils::atomic_write_json(&path, settings)
        .with_context(|| format!("writing settings {}", path.display()))?;
    tracing::info!(path = %path.display(), "settings.json written");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// `MERIDIAN_SETTINGS_PATH` is a PROCESS-global env var and cargo runs tests in
    /// parallel threads, so every test that points settings resolution at a temp file
    /// must hold this lock — otherwise one test's file is read by another and the
    /// failure looks like a bug in the code under test rather than in the harness.
    /// (Held for the whole test body; `set_var`/`remove_var` inside the guard.)
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Point settings resolution at a temp file for the duration of a test.
    fn with_settings_path(path: &std::path::Path) {
        std::env::set_var("MERIDIAN_SETTINGS_PATH", path);
    }

    fn custom_row(id: &str, rungs: &[(&str, SchemaRung)]) -> CustomLlmProvider {
        CustomLlmProvider {
            id: id.to_string(),
            vendor: "gemini".to_string(),
            name: "Gemini".to_string(),
            base_url: "https://example.test/v1beta/openai".to_string(),
            model: "gemini-flash-latest".to_string(),
            api_key: "secret".to_string(),
            rpm: 0,
            rpd: 0,
            rungs: rungs.iter().map(|(k, v)| (k.to_string(), *v)).collect(),
        }
    }

    /// Every schema at `rung` — a fully-probed endpoint.
    fn fully_probed(id: &str, rung: SchemaRung) -> CustomLlmProvider {
        let all: Vec<(&str, SchemaRung)> =
            PIPELINE_SCHEMA_KEYS.iter().map(|k| (*k, rung)).collect();
        custom_row(id, &all)
    }

    /// The gate is the WEAKEST schema, not the best or the average: production sends all
    /// four, so one schema the endpoint can't hold is enough to disqualify it.
    #[test]
    fn effective_rung_is_the_weakest_schema() {
        let mut p = fully_probed("g1", SchemaRung::Strict);
        // The one that would silently drop work if it were trusted.
        p.rungs
            .insert("worklog_generate".to_string(), SchemaRung::Prompt);
        assert_eq!(p.effective_rung(), SchemaRung::Prompt);
        assert!(!p.is_production_eligible());
    }

    /// A probe that died partway (a rate limit is the ordinary way) leaves excellent rungs
    /// for the schemas it reached. `min()` over only those would read as "excellent" and let
    /// the endpoint into production with a schema NEVER TRIED — observed live: a free-tier
    /// key 429s on the fourth schema after three clean `Strict` results.
    #[test]
    fn a_partial_probe_never_passes_the_gate() {
        let mut p = fully_probed("g1", SchemaRung::Strict);
        p.rungs.remove("plan_task_draft");

        assert_eq!(p.effective_rung(), SchemaRung::None);
        assert!(!p.is_production_eligible());
        assert!(!p.is_fully_probed());
        assert_eq!(p.unmeasured_schemas(), vec!["plan_task_draft"]);
    }

    /// A resumed probe must only pay for what is missing.
    #[test]
    fn unmeasured_schemas_lists_only_what_is_missing() {
        let p = custom_row("g1", &[("workstream", SchemaRung::Strict)]);
        assert_eq!(
            p.unmeasured_schemas(),
            vec!["activity_report", "worklog_generate", "plan_task_draft"]
        );
        assert!(fully_probed("g2", SchemaRung::Strict).is_fully_probed());
    }

    /// A rung recorded under an unknown key (a renamed schema, a hand-edited file) must not
    /// count towards the gate — it is evidence about nothing the pipeline sends.
    #[test]
    fn an_unknown_schema_key_does_not_satisfy_the_gate() {
        let mut p = fully_probed("g1", SchemaRung::Strict);
        p.rungs.remove("workstream");
        p.rungs
            .insert("workstream_v2".to_string(), SchemaRung::Strict);
        assert_eq!(p.effective_rung(), SchemaRung::None);
        assert!(!p.is_production_eligible());
    }

    /// Never probed ≠ fine. An unmeasured endpoint is Lab-only until it earns otherwise.
    #[test]
    fn an_unprobed_endpoint_is_not_production_eligible() {
        let p = custom_row("g1", &[]);
        assert_eq!(p.effective_rung(), SchemaRung::None);
        assert!(!p.is_production_eligible());
    }

    /// Shape-enforcing rungs pass the gate; the two that guarantee no shape do not.
    #[test]
    fn only_shape_enforcing_rungs_are_production_eligible() {
        assert!(SchemaRung::Strict.is_production_eligible());
        assert!(SchemaRung::JsonSchema.is_production_eligible());
        assert!(!SchemaRung::JsonObject.is_production_eligible());
        assert!(!SchemaRung::Prompt.is_production_eligible());
        assert!(!SchemaRung::None.is_production_eligible());
    }

    /// A setting pointing at a deleted row must resolve to nothing — never to another row,
    /// and never quietly to a different provider.
    #[test]
    fn a_dangling_custom_id_resolves_to_none() {
        let mut s = RuntimeSettings {
            llm_provider: "custom".to_string(),
            llm_provider_custom_id: Some("deleted".to_string()),
            custom_llm_providers: vec![custom_row("g1", &[("workstream", SchemaRung::Strict)])],
            ..Default::default()
        };
        assert!(s.active_custom_provider().is_none());

        s.llm_provider_custom_id = Some("g1".to_string());
        assert_eq!(
            s.active_custom_provider().map(|p| p.id.as_str()),
            Some("g1")
        );
    }

    /// The registry is inert unless `llm_provider` actually selects `custom` — a configured
    /// endpoint must not hijack a user still on claude.
    #[test]
    fn a_configured_endpoint_is_inert_until_selected() {
        let s = RuntimeSettings {
            llm_provider: "claude".to_string(),
            llm_provider_custom_id: Some("g1".to_string()),
            custom_llm_providers: vec![custom_row("g1", &[("workstream", SchemaRung::Strict)])],
            ..Default::default()
        };
        assert!(s.active_custom_provider().is_none());
    }

    /// Rungs must survive the settings round-trip; a probe result that doesn't persist
    /// would silently re-gate the endpoint to Lab-only on next load.
    #[test]
    fn rungs_round_trip_through_json() {
        let s = RuntimeSettings {
            custom_llm_providers: vec![custom_row("g1", &[("workstream", SchemaRung::Strict)])],
            ..Default::default()
        };
        let back: RuntimeSettings =
            serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(back.custom_llm_providers, s.custom_llm_providers);
        assert_eq!(
            back.custom_llm_providers[0].rungs["workstream"],
            SchemaRung::Strict
        );
    }

    /// A settings.json written before custom providers existed must load unchanged —
    /// the whole reason `llm_provider` is a String (see llm_provider.rs).
    #[test]
    fn settings_predating_the_registry_still_load() {
        let s: RuntimeSettings = serde_json::from_str(r#"{"llm_provider":"claude"}"#).unwrap();
        assert!(s.custom_llm_providers.is_empty());
        assert_eq!(s.llm_provider_custom_id, None);
        assert!(s.active_custom_provider().is_none());
    }

    #[test]
    fn read_coerces_nulls_and_write_round_trips_extra_keys() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir =
            std::env::temp_dir().join(format!("meridian-settings-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        with_settings_path(&path);

        // Seed a file with an extra (unknown) key + an explicit null string field.
        std::fs::write(
            &path,
            r#"{"log_level":"DEBUG","oo_email":null,"_extra":"keep me"}"#,
        )
        .unwrap();

        let v = read_settings_value();
        assert_eq!(v["log_level"], json!("DEBUG"), "file overrides default");
        assert_eq!(v["oo_email"], json!(""), "null string coerced to ''");
        assert_eq!(v["_extra"], json!("keep me"), "unknown key preserved");
        // a default-only field still present
        assert_eq!(v["poll_interval_secs"], json!(60));

        // Write it back and confirm the extra key survives a round-trip.
        write_settings_value(&v).unwrap();
        let again = read_settings_value();
        assert_eq!(again["_extra"], json!("keep me"));

        std::env::remove_var("MERIDIAN_SETTINGS_PATH");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An existing settings.json predates `llm_provider` entirely — it must upgrade to
    /// the default provider without a migration. `#[serde(default)]` buys this, and this
    /// test is what stops someone removing it.
    #[test]
    fn settings_without_llm_provider_default_to_claude() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!(
            "meridian-settings-upgrade-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        with_settings_path(&path);

        std::fs::write(&path, r#"{"log_level":"DEBUG","poll_interval_secs":30}"#).unwrap();

        let s = load_runtime_settings();
        assert_eq!(s.llm_provider, "claude");
        assert_eq!(s.llm_provider_model, None);
        assert_eq!(s.log_level, "DEBUG", "the pre-existing settings survive");

        std::env::remove_var("MERIDIAN_SETTINGS_PATH");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The landmine this whole design works around: `load_runtime_settings` falls back to
    /// `Default` on ANY deserialise error, so a bad field would take every OTHER setting
    /// with it. Typing `llm_provider` as a `String` means an unrecognised provider (say,
    /// one written by a newer build) costs the user their provider and nothing else — the
    /// work hours, poll interval and log level all survive. If someone ever retypes this
    /// field as the enum, this test fails and tells them why.
    #[test]
    fn an_unknown_llm_provider_does_not_reset_every_other_setting() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!(
            "meridian-settings-badprovider-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        with_settings_path(&path);

        std::fs::write(
            &path,
            r#"{"llm_provider":"gemini","poll_interval_secs":30,"work_hours_start":"07:30"}"#,
        )
        .unwrap();

        let s = load_runtime_settings();
        assert_eq!(
            s.poll_interval_secs, 30,
            "an unknown provider must not reset this"
        );
        assert_eq!(s.work_hours_start, "07:30", "…nor this");
        // The raw string survives the load; it is the *resolver* that rejects it.
        assert_eq!(s.llm_provider, "gemini");
        assert_eq!(
            crate::llm_provider::LlmProvider::from_wire(&s.llm_provider),
            None,
            "and the resolver degrades it to the default"
        );

        std::env::remove_var("MERIDIAN_SETTINGS_PATH");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
