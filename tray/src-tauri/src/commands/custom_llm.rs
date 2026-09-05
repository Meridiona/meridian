//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Managing the user's own cloud endpoints — add, probe, remove.
//!
//! # What this is
//! The write side of the [`meridian_core::CustomLlmProvider`] registry. Adding an endpoint
//! MEASURES it (`meridian::llm::probe`) and stores what it found, because the gate that
//! decides whether it may run the pipeline reads evidence, never a vendor name.
//!
//! # The API key never comes back out
//! A row is returned to the UI as a [`CustomProviderView`], which has no key field at all —
//! keyless by construction rather than by remembering to redact. The one other path that
//! serialises a row, `get_settings`, redacts per row on the way out. Every command here
//! `skip`s the key (and the `base_url`, which can carry a key in a query string) in its
//! span: spans reach the telemetry spool, which ships inside a diagnostics bundle.
//!
//! # Who calls this
//! Registered in `lib.rs`'s `invoke_handler!`; consumed by the Intelligence panel's custom
//! provider cards and the LLM Lab's variant composer.
//!
//! # Related
//! - [`crate::commands::settings`] — reads/redacts the same rows, and enforces the gate on
//!   the `llm_provider` write.
//! - `meridian::llm::probe` — the measurement; `meridian_core::SchemaRung` — what it means.

use meridian_core::llm_capacity::{self, CapacityAssessment};
use meridian_core::{settings, CustomLlmProvider, SchemaRung};
use serde::Serialize;
use serde_json::Value;

/// Serializes the whole registry read-modify-write across concurrent commands (two
/// "Test" clicks, an add racing a remove). Each command reads the rows, does its
/// work, and persists them, and without this those cycles interleave and
/// lost-update each other — the same bug class 2104c030 fixed for the provider-test
/// cache. It is a `tokio` mutex because the critical section spans a probe's
/// `.await`.
///
/// # These commands are NOT the only writers
/// This doc used to claim they were, and that claim was the bug:
/// `commands::settings::update_settings` also replaces `custom_llm_providers`
/// wholesale (the shallow body merge, plus `restore_custom_keys`), and it never
/// took this lock. So a settings save could land a registry edit in the middle
/// of a probe, and [`persist_rows`]'s re-read would then overwrite it with the
/// pre-probe rows - the exact lost update `persist_rows` was added to stop, via
/// the one writer it did not know about.
///
/// Every writer of `custom_llm_providers` must now hold this, which is what
/// [`registry_guard`] exists for. The daemon only READS the registry (via the
/// resolver), so a tray-process lock is still sufficient.
///
/// Lock ORDER is `REGISTRY_LOCK` then the settings lock inside
/// `meridian_core::settings::mutate_settings_value`, at every site. Never the
/// reverse.
static REGISTRY_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Acquire [`REGISTRY_LOCK`] from outside this module.
///
/// `update_settings` needs it whenever its body carries `custom_llm_providers`;
/// see [`REGISTRY_LOCK`] for why holding only the settings lock is not enough.
pub(crate) async fn registry_guard() -> tokio::sync::MutexGuard<'static, ()> {
    REGISTRY_LOCK.lock().await
}

/// A registry row as the UI sees it: everything except the key, plus the verdicts the UI
/// must not re-derive (the gate lives in one place — `meridian-core` — and this carries its
/// answer rather than inviting the frontend to compute a second, drifting one).
#[derive(Debug, Clone, Serialize)]
pub struct CustomProviderView {
    pub id: String,
    pub vendor: String,
    pub name: String,
    pub base_url: String,
    pub model: String,
    /// Requests-per-minute ceiling, `0` = unpaced. Safe to surface: unlike the key it is a
    /// user-entered plan limit, not a secret.
    pub rpm: u32,
    /// Requests-per-day ceiling, `0` = not known. Same safety note as `rpm`.
    pub rpd: u32,
    /// Whether these limits can actually run the app for a day, computed in
    /// `meridian-core` so the frontend renders a verdict rather than deriving a second,
    /// drifting one from the raw numbers.
    pub capacity: CapacityAssessment,
    /// Measured rung per schema key. Missing key = never measured.
    pub rungs: std::collections::BTreeMap<String, SchemaRung>,
    /// The weakest measured rung — what the endpoint can actually be trusted to hold.
    pub effective_rung: SchemaRung,
    /// Has every pipeline schema been measured? `false` after a probe stopped early.
    pub fully_probed: bool,
    /// May this be the production provider?
    pub production_eligible: bool,
    /// Is this the selected production provider right now?
    pub selected: bool,
}

impl CustomProviderView {
    fn of(row: &CustomLlmProvider, selected_id: Option<&str>) -> Self {
        Self {
            id: row.id.clone(),
            vendor: row.vendor.clone(),
            name: row.name.clone(),
            base_url: row.base_url.clone(),
            model: row.model.clone(),
            rpm: row.rpm,
            rpd: row.rpd,
            capacity: llm_capacity::assess(row.rpm, row.rpd),
            rungs: row.rungs.clone(),
            effective_rung: row.effective_rung(),
            fully_probed: row.is_fully_probed(),
            production_eligible: row.is_production_eligible(),
            selected: selected_id == Some(row.id.as_str()),
        }
    }
}

/// Judge a prospective endpoint's limits WITHOUT saving or contacting anything.
///
/// Exists so the add form can warn before the user commits. It could not just do this
/// arithmetic in TypeScript: adding an endpoint spends real requests from the very quota
/// being judged, so the warning has to be right the first time, and a second
/// implementation of [`llm_capacity::assess`] in the frontend would be free to drift
/// from the one the saved card then shows.
#[tauri::command]
#[tracing::instrument]
pub fn assess_llm_capacity(rpm: u32, rpd: u32) -> CapacityAssessment {
    llm_capacity::assess(rpm, rpd)
}

/// What `add`/`probe` report back: the row plus what the measurement cost and whether it
/// finished. The UI needs `incomplete` to offer "retry" rather than claiming the endpoint
/// is bad.
#[derive(Debug, Clone, Serialize)]
pub struct ProbeOutcome {
    pub provider: CustomProviderView,
    /// Real metered requests this run spent.
    pub requests: u32,
    /// Why it stopped early (a rate limit is the usual reason), or `None` if it completed.
    pub incomplete: Option<String>,
}

/// Read the registry out of a settings JSON value.
///
/// A malformed row is dropped rather than failing the whole read — one hand-edited entry
/// must not cost the user every other endpoint (the same rule `llm_provider` follows).
fn read_rows(v: &Value) -> Vec<CustomLlmProvider> {
    v.get("custom_llm_providers")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|r| match serde_json::from_value::<CustomLlmProvider>(r.clone()) {
                    Ok(row) => Some(row),
                    Err(e) => {
                        tracing::warn!(error = %e, "custom_llm: dropping an unreadable registry row");
                        None
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Write the registry back into a settings value, preserving every other key (the settings
/// write is a merge, not a replace — see `meridian_core::settings`).
/// Persist `rows` into the CURRENT settings document, under the shared settings
/// lock, and return the document as written.
///
/// # Why this re-reads instead of writing the caller's snapshot
/// Every command here reads settings, does its work, and writes back. For
/// `add_custom_llm_provider` and `replace_key` that work includes
/// `probe_endpoint(...).await` - a NETWORK round trip. Writing the pre-probe
/// snapshot means every non-registry settings change that completed during
/// those seconds is silently discarded: a Settings save, a `request_pm_tool`, a
/// sign-in's `write_account_pseudonym`.
///
/// [`REGISTRY_LOCK`] does not help. It serialises the custom-provider commands
/// against EACH OTHER, and knows nothing about
/// `meridian_core::settings::mutate_settings_value`, which is what every other
/// writer now uses. Two locks that do not see each other are one lock.
///
/// Re-reading under the shared lock is safe precisely because `REGISTRY_LOCK` is
/// still held: no other registry command can have changed `rows` in the
/// meantime, so the caller's `rows` remains authoritative for its own key while
/// every other key comes from the freshest document.
///
/// The shared lock is NOT held across the probe - only across this write, which
/// is a read, an insert and an atomic rename.
fn persist_rows(rows: &[CustomLlmProvider]) -> anyhow::Result<Value> {
    settings::mutate_settings_value(|v| {
        write_rows(v, rows).map_err(|e| anyhow::anyhow!(e))?;
        Ok(v.clone())
    })
}

fn write_rows(v: &mut Value, rows: &[CustomLlmProvider]) -> Result<(), String> {
    let obj = v.as_object_mut().ok_or("settings are not an object")?;
    obj.insert(
        "custom_llm_providers".into(),
        serde_json::to_value(rows).map_err(|e| format!("serialise providers: {e}"))?,
    );
    Ok(())
}

fn selected_custom_id(v: &Value) -> Option<String> {
    if v.get("llm_provider").and_then(Value::as_str) != Some("custom") {
        return None;
    }
    v.get("llm_provider_custom_id")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// Lay a probe's measurements over what was already known, keeping any schema the probe never
/// reached.
///
/// The whole point is that a probe which stops early (a 429) must not be able to LOWER what
/// the row claims — see `probe_custom_llm_provider`, which is where that would strand the
/// user's selected endpoint below the gate.
fn merge_rungs(
    stored: &mut std::collections::BTreeMap<String, SchemaRung>,
    fresh: std::collections::BTreeMap<String, SchemaRung>,
) {
    stored.extend(fresh);
}

/// An id derived from the name, unique within the registry. Stable and human-readable, so a
/// `custom:<id>` Lab variant and a settings file stay legible — the alternative (a uuid)
/// makes both unreadable for no gain at this scale.
fn make_id(name: &str, existing: &[CustomLlmProvider]) -> String {
    let base: String = name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .trim_matches('-')
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    let base = if base.is_empty() {
        "endpoint".to_string()
    } else {
        base
    };
    if !existing.iter().any(|r| r.id == base) {
        return base;
    }
    (2..)
        .map(|n| format!("{base}-{n}"))
        .find(|c| !existing.iter().any(|r| &r.id == c))
        .unwrap()
}

/// The checks every outbound request needs, whatever it is being sent for.
///
/// Shared by [`validate`] (adding an endpoint) and
/// [`list_custom_llm_provider_models`] (listing one that isn't saved yet) so the two cannot
/// drift — a rule enforced on the add path but not the listing path would be a rule with a
/// hole in it.
fn validate_transport_inputs(base_url: &str, api_key: &str) -> Result<(), String> {
    // NO EMPTY-KEY REJECTION, deliberately. A self-hosted OpenAI-compatible server (Ollama,
    // LM Studio, llama.cpp, vLLM) usually has no auth at all, and there is no key for the
    // user to invent - requiring one made every local endpoint unconfigurable, which is the
    // gap this allowance exists to close. A blank key sends no `Authorization` header (see
    // `openai_compat::with_auth`), so nothing is leaked by permitting it.
    //
    // A blank key against an endpoint that DOES require one is not silently accepted either:
    // it fails at `list_models` or at the probe, both of which run during setup with the user
    // watching, and both of which report a 401 in the vendor's own words. That is the right
    // place to find out - unlike a wrong MODEL, which would only surface hours later.
    //
    // The reachability rule that was considered and rejected: allowing a blank key only for
    // loopback/RFC1918 hosts. It would have refused Tailscale (100.64/10), Docker host
    // aliases and reverse-proxied local models, all of them legitimate, in exchange for
    // approximately no security - an EMPTY credential discloses nothing wherever it is sent.
    if !base_url.starts_with("http://") && !base_url.starts_with("https://") {
        return Err("base URL must start with http:// or https://".into());
    }
    // The key becomes an Authorization header and the URL becomes a request line: a newline
    // in either is header injection, exactly as `oo_email`/`oo_password` guard against.
    // Still checked for a BLANK key: blank is allowed, whitespace-with-a-newline is not.
    for (field, v) in [("API key", api_key), ("base URL", base_url)] {
        if v.contains('\n') || v.contains('\r') {
            return Err(format!("{field} contains invalid characters"));
        }
    }
    Ok(())
}

/// Reject what would break the request or the file. Kept strict at the door: the alternative
/// is discovering it hours later as a failed fold.
fn validate(name: &str, base_url: &str, model: &str, api_key: &str) -> Result<(), String> {
    if name.trim().is_empty() {
        return Err("name is required".into());
    }
    if model.trim().is_empty() {
        // Unlike a CLI provider there is no "the provider's default model" to fall back to.
        return Err("model is required - a custom endpoint has no default".into());
    }
    validate_transport_inputs(base_url, api_key)?;
    // The model rides in the request BODY rather than a header or the request line, so it is
    // checked here rather than in the shared transport helper.
    if model.contains('\n') || model.contains('\r') {
        return Err("model contains invalid characters".into());
    }
    Ok(())
}

/// Add an endpoint and MEASURE it, returning what the probe found.
///
/// The probe spends real metered requests (up to one per rung per schema), which is why this
/// only ever runs on an explicit add — never a mount or a poll tick.
///
/// The row is saved even when the probe stops early (a rate limit is routine on a free-tier
/// key): the measurements already bought are kept, the endpoint is usable in the LLM Lab,
/// and `probe_custom_llm_provider` resumes where this left off. It simply cannot be the
/// production provider until the measurement is complete and strong enough — the gate reads
/// the row, so a half-measured one is safe on disk.
#[tauri::command]
// `base_url` is skipped too: `#[instrument]` auto-captures every un-skipped param as a
// span field, and a base URL can carry a key in a query string (see `openai_compat`) -
// spans ship inside diagnostics bundles, so the URL never reaches one. `endpoint_id` +
// `vendor` in the info! line below give enough identity to debug without it.
#[tracing::instrument(skip(api_key, base_url), fields(has_key = !api_key.is_empty()))]
pub async fn add_custom_llm_provider(
    vendor: String,
    name: String,
    base_url: String,
    model: String,
    api_key: String,
    rpm: u32,
    rpd: u32,
) -> Result<ProbeOutcome, String> {
    validate(&name, &base_url, &model, &api_key)?;

    // Held across the whole read-probe-write so a concurrent add/probe/remove can't
    // lose this update. See [`REGISTRY_LOCK`].
    let _guard = REGISTRY_LOCK.lock().await;
    // READ-ONLY from here: this snapshot supplies the existing rows and the
    // validation below, and is deliberately never written back - see
    // `persist_rows` for why the write re-reads instead.
    let settings_v = settings::read_settings_value();
    let mut rows = read_rows(&settings_v);
    if rows
        .iter()
        .any(|r| r.name.eq_ignore_ascii_case(name.trim()))
    {
        return Err(format!(
            "an endpoint named \"{}\" already exists",
            name.trim()
        ));
    }

    let row = CustomLlmProvider {
        id: make_id(&name, &rows),
        vendor: vendor.trim().to_string(),
        name: name.trim().to_string(),
        base_url: base_url.trim().trim_end_matches('/').to_string(),
        model: model.trim().to_string(),
        api_key: api_key.trim().to_string(),
        rpm,
        rpd,
        rungs: Default::default(),
    };

    // `rpm` is set on the row BEFORE this call, not after the probe writes back: the probe is
    // the single biggest burst this endpoint will ever see (one metered request per schema ×
    // rung), and on a free tier it is the thing most likely to 429. An endpoint added with a
    // ceiling it does not yet carry would eat that burst exactly once - on the first run, the
    // only run where the measurement it produces is still missing.
    let report = meridian::llm::probe::probe_endpoint(&row).await;
    let row = CustomLlmProvider {
        rungs: report.rungs,
        ..row
    };

    tracing::info!(
        endpoint_id = %row.id,
        vendor = %row.vendor,
        requests = report.requests,
        effective = ?row.effective_rung(),
        eligible = row.is_production_eligible(),
        incomplete = ?report.incomplete,
        "custom_llm: endpoint added and measured"
    );

    rows.push(row.clone());
    let settings_v =
        persist_rows(&rows).map_err(|e| crate::cmd_err!(e, "custom_llm: write failed"))?;

    Ok(ProbeOutcome {
        provider: CustomProviderView::of(&row, selected_custom_id(&settings_v).as_deref()),
        requests: report.requests,
        incomplete: report.incomplete,
    })
}

/// Re-measure `id` — the card's "Test" button, and the way a probe stopped by a rate limit
/// is resumed.
///
/// `refresh` re-measures from scratch (the endpoint's support can change under it: a vendor
/// ships strict mode, a key is swapped for one on a different tier). Otherwise this only
/// buys the schemas still missing, so resuming after a 429 costs nothing already paid for.
///
/// # A refresh MERGES, it does not reset
///
/// The fresh measurements are laid over the stored ones rather than replacing them, and the
/// stored row is never cleared up front. This matters because a refresh can stop early — a
/// 429 is routine on the free-tier keys this feature is meant for — and a cleared row would
/// then be left half-measured, i.e. below the gate. The gate only runs on the settings WRITE,
/// so nothing would catch it: an endpoint the user has already selected would keep the
/// selection while silently dropping to a rung the pipeline can't rely on, and the next hour
/// would run degraded. Merging means an interrupted refresh costs requests and nothing else.
///
/// A refresh that DOES reach a schema still overwrites it, so a genuine downgrade (the vendor
/// pulled strict mode) is recorded. A schema it never reached keeps the measurement it
/// already had — which is exactly what it had a moment ago, so this can't make the row's
/// claims any staler than not pressing the button at all.
#[tauri::command]
#[tracing::instrument]
pub async fn probe_custom_llm_provider(id: String, refresh: bool) -> Result<ProbeOutcome, String> {
    // Held across the whole read-probe-write so a concurrent add/probe/remove can't
    // lose this update. See [`REGISTRY_LOCK`].
    let _guard = REGISTRY_LOCK.lock().await;
    // READ-ONLY from here: this snapshot supplies the existing rows and the
    // validation below, and is deliberately never written back - see
    // `persist_rows` for why the write re-reads instead.
    let settings_v = settings::read_settings_value();
    let mut rows = read_rows(&settings_v);
    let idx = rows
        .iter()
        .position(|r| r.id == id)
        .ok_or_else(|| format!("no custom endpoint with id {id}"))?;

    // A scratch row with no measurements makes `probe_endpoint` treat every schema as
    // unmeasured (it resumes from `unmeasured_schemas()`), without touching what is stored.
    let target = if refresh {
        CustomLlmProvider {
            rungs: Default::default(),
            ..rows[idx].clone()
        }
    } else {
        rows[idx].clone()
    };
    let report = meridian::llm::probe::probe_endpoint(&target).await;
    merge_rungs(&mut rows[idx].rungs, report.rungs);

    tracing::info!(
        endpoint_id = %id,
        requests = report.requests,
        effective = ?rows[idx].effective_rung(),
        eligible = rows[idx].is_production_eligible(),
        incomplete = ?report.incomplete,
        "custom_llm: endpoint re-measured"
    );

    let settings_v = persist_rows(&rows).map_err(|e| format!("{e:#}"))?;

    Ok(ProbeOutcome {
        provider: CustomProviderView::of(&rows[idx], selected_custom_id(&settings_v).as_deref()),
        requests: report.requests,
        incomplete: report.incomplete,
    })
}

/// Remove an endpoint.
///
/// Refused while it is the selected production provider. Silently unselecting it would swap
/// the user's model without telling them; leaving the selection dangling would fail every
/// call. Both are worse than making them choose.
#[tauri::command]
#[tracing::instrument]
pub async fn remove_custom_llm_provider(id: String) -> Result<Vec<CustomProviderView>, String> {
    // Held across the read-modify-write so a concurrent add/probe/remove can't lose
    // this update. See [`REGISTRY_LOCK`].
    let _guard = REGISTRY_LOCK.lock().await;
    // READ-ONLY from here: this snapshot supplies the existing rows and the
    // validation below, and is deliberately never written back - see
    // `persist_rows` for why the write re-reads instead.
    let settings_v = settings::read_settings_value();
    let mut rows = read_rows(&settings_v);

    if selected_custom_id(&settings_v).as_deref() == Some(id.as_str()) {
        return Err(
            "this endpoint is your current AI provider - switch to another provider first".into(),
        );
    }
    // Drop this endpoint's pacing reservation. Only clears THIS process's map (the tray, which
    // paces probes); the daemon keeps its own for production calls and lets it expire, which is
    // harmless — a reservation is at most one interval long and the id is never reused.
    meridian::llm::rate_limit::forget(&meridian::llm::rate_limit::custom_key(&id));

    let before = rows.len();
    rows.retain(|r| r.id != id);
    if rows.len() == before {
        return Err(format!("no custom endpoint with id {id}"));
    }

    let settings_v = persist_rows(&rows).map_err(|e| format!("{e:#}"))?;
    tracing::info!(endpoint_id = %id, remaining = rows.len(), "custom_llm: endpoint removed");

    let sel = selected_custom_id(&settings_v);
    Ok(rows
        .iter()
        .map(|r| CustomProviderView::of(r, sel.as_deref()))
        .collect())
}

/// Swap the API key on an existing endpoint, then re-measure it.
///
/// The one thing a user genuinely needs to do to a configured endpoint and previously could
/// not: a Groq key gets rotated, revoked, or pasted from the wrong account, and the only
/// route back was Remove-then-add — which [`remove_custom_llm_provider`] refuses while the
/// endpoint is the selected provider, i.e. exactly when the key matters. The alternative,
/// adding a second endpoint, is rejected too ("an endpoint named X already exists").
///
/// # Why the measurements are cleared
///
/// A different key can be a different account on a different tier, so what the endpoint
/// supports is no longer known — keeping the old rungs would let a key that has never
/// answered a schema inherit a passing grade and be selected as the production provider.
/// Clearing and re-probing is the same path [`probe_custom_llm_provider`] takes with
/// `refresh: true`, for the same reason, and it is a handful of metered requests on the NEW
/// key.
///
/// The row is written even when the probe stops early: the key is the user's, they pasted it
/// deliberately, and losing it because a free tier 429'd mid-measurement would be worse than
/// a row they can finish measuring with Test.
#[tauri::command]
#[tracing::instrument(skip(api_key), fields(has_key = !api_key.is_empty()))]
pub async fn replace_custom_llm_provider_key(
    id: String,
    api_key: String,
) -> Result<ProbeOutcome, String> {
    let key = api_key.trim().to_string();
    if key.is_empty() {
        return Err("paste a key first".into());
    }

    // Held across the whole read-probe-write so a concurrent add/probe/remove can't
    // lose this update. See [`REGISTRY_LOCK`].
    let _guard = REGISTRY_LOCK.lock().await;
    // READ-ONLY from here: this snapshot supplies the existing rows and the
    // validation below, and is deliberately never written back - see
    // `persist_rows` for why the write re-reads instead.
    let settings_v = settings::read_settings_value();
    let mut rows = read_rows(&settings_v);
    let idx = rows
        .iter()
        .position(|r| r.id == id)
        .ok_or_else(|| format!("no custom endpoint with id {id}"))?;

    rows[idx].api_key = key;
    rows[idx].rungs = Default::default();

    let report = meridian::llm::probe::probe_endpoint(&rows[idx]).await;
    rows[idx].rungs = report.rungs;

    tracing::info!(
        endpoint_id = %id,
        requests = report.requests,
        effective = ?rows[idx].effective_rung(),
        eligible = rows[idx].is_production_eligible(),
        incomplete = ?report.incomplete,
        "custom_llm: endpoint key replaced and re-measured"
    );

    let settings_v =
        persist_rows(&rows).map_err(|e| crate::cmd_err!(e, "custom_llm: write failed"))?;

    Ok(ProbeOutcome {
        provider: CustomProviderView::of(&rows[idx], selected_custom_id(&settings_v).as_deref()),
        requests: report.requests,
        incomplete: report.incomplete,
    })
}

/// Ask an endpoint which models it serves, for the model picker.
///
/// Two callers, two shapes:
///
/// * **A saved endpoint** — pass `id` alone. The base URL and key are read from the registry
///   here, because [`CustomProviderView`] deliberately omits `api_key` (there is a test
///   pinning that), so the frontend has no key to hand back and MUST go through the id.
/// * **The add-endpoint form** — pass `base_url` + `api_key`, which the user has just typed
///   and no row exists for yet.
///
/// # This is best-effort by design
///
/// Plenty of OpenAI-compatible servers never implement `/models`. Every caller must treat an
/// error as "offer free text" rather than a failure — the model field stays hand-typeable in
/// all cases, so a missing listing costs convenience and nothing else.
///
/// # Cost
///
/// One unmetered-in-practice `GET`, only ever on an explicit user action (opening or
/// refreshing the picker) — never on a mount or a poll tick. Unlike
/// [`probe_custom_llm_provider`] it spends no completion tokens, so it takes no rate-limit
/// reservation; a 429 is surfaced to the caller instead of retried.
///
/// # Related
///
/// [`meridian::llm::openai_compat::list_models`] does the request and response shaping.
#[tauri::command]
// `base_url` is skipped alongside the key, not just the key: a base URL can carry a
// credential in a query string, and this span goes to the telemetry spool that ships inside
// a diagnostics bundle. The endpoint id is the one safe thing to record - the same rule the
// backend follows (see the logging note in src/llm/openai_compat.rs).
#[tracing::instrument(skip(base_url, api_key))]
pub async fn list_custom_llm_provider_models(
    id: Option<String>,
    base_url: Option<String>,
    api_key: Option<String>,
) -> Result<Vec<String>, String> {
    // Resolve to (base_url, key, id-for-logging) from whichever shape the caller used.
    let (url, key, log_id) = match id {
        Some(id) => {
            let settings_v = settings::read_settings_value();
            let rows = read_rows(&settings_v);
            let row = rows
                .iter()
                .find(|r| r.id == id)
                .ok_or_else(|| format!("no custom endpoint with id {id}"))?;
            (row.base_url.clone(), row.api_key.clone(), id)
        }
        None => {
            let url = base_url.unwrap_or_default();
            let key = api_key.unwrap_or_default();
            // The same door the add path uses - see `validate_transport_inputs`.
            validate_transport_inputs(&url, &key)?;
            (url, key, "unsaved".to_string())
        }
    };

    meridian::llm::openai_compat::list_models(&url, &key, &log_id)
        .await
        .map_err(|e| crate::cmd_err!(e, endpoint_id = %log_id, "custom_llm: model listing failed"))
}

/// Every configured endpoint, keyless — what the picker and the Lab's variant list render.
#[tauri::command]
#[tracing::instrument]
pub async fn list_custom_llm_providers() -> Result<Vec<CustomProviderView>, String> {
    let v = settings::read_settings_value();
    let sel = selected_custom_id(&v);
    Ok(read_rows(&v)
        .iter()
        .map(|r| CustomProviderView::of(r, sel.as_deref()))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    /// `MERIDIAN_SETTINGS_PATH` is PROCESS-global and cargo runs tests in
    /// threads, so every test here that points settings resolution at a temp
    /// file must serialise on this - otherwise one test reads another's file and
    /// the failure looks like a bug in the code under test.
    /// A `tokio` mutex, not a `std` one: the tests below hold it across an
    /// `.await`, which clippy rightly refuses for a `std` guard - it blocks the
    /// executor thread and can deadlock a single-threaded runtime.
    static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    async fn env_lock() -> tokio::sync::MutexGuard<'static, ()> {
        ENV_LOCK.lock().await
    }

    /// For the one synchronous test here. Safe because it is not inside a
    /// runtime; `blocking_lock` would panic if it were.
    fn env_lock_blocking() -> tokio::sync::MutexGuard<'static, ()> {
        ENV_LOCK.blocking_lock()
    }

    /// A settings-side registry edit must WAIT for an in-flight custom-provider
    /// probe rather than landing in the middle of it.
    ///
    /// `persist_rows` re-reads the document but replaces `custom_llm_providers`
    /// with the caller's pre-probe rows. That is safe only if no other writer
    /// can change the registry during the probe - and `update_settings` could,
    /// because it replaces the registry wholesale while never taking
    /// `REGISTRY_LOCK`. A user saving a settings edit that touched a custom
    /// provider mid-probe had it silently discarded when the probe finished.
    ///
    /// # This drives the REAL path, and the first version did not
    /// The first attempt called `registry_guard()` directly in the spawned task.
    /// It never touched `update_settings`, never passed a registry-bearing body,
    /// and never exercised `body_touches_registry` - so it would have kept
    /// passing if someone deleted the guard from the settings path or broke the
    /// gating, restoring the very race it claims to prevent. Caught by review;
    /// the third green-but-vacuous test in this series.
    ///
    /// It now goes through `settings::mutate_settings_for_body`, which is the
    /// function `update_settings` itself calls, so the gating decision is under
    /// test too.
    #[tokio::test]
    async fn a_settings_side_registry_edit_waits_for_an_in_flight_probe() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc as StdArc;

        let _env = env_lock().await;
        let dir = std::env::temp_dir().join(format!(
            "meridian-registry-wait-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        std::env::set_var("MERIDIAN_SETTINGS_PATH", &path);
        let _ = std::fs::remove_file(&path);

        // A custom-provider command holding the lock across its probe.
        let probe_guard = registry_guard().await;

        // The REAL settings path, with a body that carries the registry.
        let body = serde_json::json!({ "custom_llm_providers": [row("acme", "Acme")] });
        let body_obj = body.as_object().unwrap().clone();
        let landed = StdArc::new(AtomicBool::new(false));
        let landed_in_task = landed.clone();
        let settings_write = tokio::spawn(async move {
            crate::commands::settings::mutate_settings_for_body(&body_obj, |v| {
                if let Some(obj) = v.as_object_mut() {
                    obj.insert(
                        "custom_llm_providers".into(),
                        body_obj["custom_llm_providers"].clone(),
                    );
                }
                Ok(v.clone())
            })
            .await
            .expect("settings write must succeed");
            landed_in_task.store(true, Ordering::SeqCst);
        });

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(
            !landed.load(Ordering::SeqCst),
            "a settings-side registry edit landed DURING the probe - persist_rows \
             would overwrite it with the pre-probe rows"
        );

        drop(probe_guard);
        settings_write.await.expect("task panicked");
        assert!(landed.load(Ordering::SeqCst));
        assert_eq!(
            read_rows(&settings::read_settings_value()).len(),
            1,
            "the settings-side registry edit must be persisted once it proceeds"
        );

        std::env::remove_var("MERIDIAN_SETTINGS_PATH");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The other half of the gate: a body that does NOT carry the registry must
    /// NOT queue behind a probe. Without this, "always take the lock" would pass
    /// the test above while making every unrelated Settings save wait seconds.
    #[tokio::test]
    async fn a_non_registry_settings_save_does_not_wait_for_a_probe() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc as StdArc;

        let _env = env_lock().await;
        let dir = std::env::temp_dir().join(format!(
            "meridian-registry-nowait-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        std::env::set_var("MERIDIAN_SETTINGS_PATH", &path);
        let _ = std::fs::remove_file(&path);

        let probe_guard = registry_guard().await;

        let body = serde_json::json!({ "llm_provider": "claude" });
        let body_obj = body.as_object().unwrap().clone();
        let landed = StdArc::new(AtomicBool::new(false));
        let landed_in_task = landed.clone();
        let settings_write = tokio::spawn(async move {
            crate::commands::settings::mutate_settings_for_body(&body_obj, |v| Ok(v.clone()))
                .await
                .expect("settings write must succeed");
            landed_in_task.store(true, Ordering::SeqCst);
        });

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(
            landed.load(Ordering::SeqCst),
            "an unrelated settings save must not serialise behind an endpoint probe"
        );

        drop(probe_guard);
        settings_write.await.expect("task panicked");

        std::env::remove_var("MERIDIAN_SETTINGS_PATH");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A registry write must not discard a non-registry settings change that
    /// landed while it was probing.
    ///
    /// This is the shape CodeRabbit flagged after #882: `add_custom_llm_provider`
    /// read settings, awaited `probe_endpoint` (a NETWORK round trip, seconds
    /// wide), then wrote its PRE-PROBE snapshot. Every `update_settings`,
    /// `request_pm_tool` or sign-in that completed during that window was
    /// silently reverted.
    ///
    /// `REGISTRY_LOCK` never covered this: it serialises the custom-provider
    /// commands against each other and knows nothing about
    /// `mutate_settings_value`, which every other writer uses. Two locks that
    /// cannot see each other are one lock.
    ///
    /// Driven through `persist_rows` rather than the `#[tauri::command]` itself
    /// because the command needs an AppHandle and a live endpoint to probe; the
    /// property under test is that the write re-reads, and this exercises it
    /// directly.
    #[test]
    fn a_registry_write_preserves_a_concurrent_settings_change() {
        let _env = env_lock_blocking();
        let dir = std::env::temp_dir().join(format!(
            "meridian-custom-llm-race-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        std::env::set_var("MERIDIAN_SETTINGS_PATH", &path);
        let _ = std::fs::remove_file(&path);

        // 1. The command reads its snapshot and starts probing.
        let pre_probe = settings::read_settings_value();
        let mut rows = read_rows(&pre_probe);
        rows.push(row("acme", "Acme"));

        // 2. An unrelated settings write completes DURING the probe window.
        meridian_core::settings::mutate_settings_value(|v| {
            if let Some(obj) = v.as_object_mut() {
                obj.insert("requested_pm_tool".into(), Value::String("shortcut".into()));
            }
            Ok(())
        })
        .expect("the concurrent write must succeed");

        // 3. The probe finishes and the registry write lands.
        persist_rows(&rows).expect("registry write must succeed");

        let final_doc = settings::read_settings_value();
        assert_eq!(
            final_doc.get("requested_pm_tool").and_then(Value::as_str),
            Some("shortcut"),
            "the registry write discarded a settings change that completed during the probe"
        );
        assert_eq!(
            read_rows(&final_doc).len(),
            1,
            "the registry write must still persist its own row"
        );

        std::env::remove_var("MERIDIAN_SETTINGS_PATH");
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn row(id: &str, name: &str) -> CustomLlmProvider {
        CustomLlmProvider {
            id: id.into(),
            vendor: "gemini".into(),
            name: name.into(),
            base_url: "https://x.test/v1".into(),
            model: "m".into(),
            api_key: "secret".into(),
            rpm: 0,
            rpd: 0,
            rungs: Default::default(),
        }
    }

    #[test]
    fn ids_are_readable_and_unique() {
        let mut rows = vec![];
        let a = make_id("Gemini Flash", &rows);
        assert_eq!(a, "gemini-flash");
        rows.push(row(&a, "Gemini Flash"));
        assert_eq!(make_id("Gemini  Flash!", &rows), "gemini-flash-2");
        // A name with nothing id-able still yields something addressable.
        assert_eq!(make_id("!!!", &rows), "endpoint");
    }

    /// The key is a header value and the URL a request line — a newline in either is header
    /// injection, the same vector `oo_password` already guards.
    #[test]
    fn validation_rejects_header_injection_and_missing_fields() {
        assert!(validate("n", "https://x.test/v1", "m", "k").is_ok());
        assert!(validate("", "https://x.test/v1", "m", "k").is_err());
        assert!(validate("n", "https://x.test/v1", "", "k").is_err());
        assert!(validate("n", "ftp://x.test", "m", "k").is_err());
        assert!(validate("n", "https://x.test/v1", "m", "k\nX-Evil: 1").is_err());
        assert!(validate("n", "https://x.test/v1\r\nHost: evil", "m", "k").is_err());
    }

    /// A KEYLESS endpoint is a supported configuration, not a half-filled form.
    ///
    /// This assertion used to read `is_err()` - "API key is required" - and that single line
    /// was the whole reason no self-hosted server could be configured: Ollama, LM Studio,
    /// llama.cpp and vLLM serve the OpenAI protocol with no auth, so there is no key for the
    /// user to type. Both doors are pinned here because they are separate call sites of the
    /// shared validator, and the listing one runs FIRST in the setup flow - leaving it strict
    /// would have failed the flow at "ask the endpoint what it serves", before the add.
    #[test]
    fn a_keyless_endpoint_is_allowed_at_both_doors() {
        assert!(validate("local", "http://localhost:11434/v1", "llama3", "").is_ok());
        assert!(validate_transport_inputs("http://localhost:1234/v1", "").is_ok());
        // Plain http is what a local server actually speaks - the scheme check must not have
        // quietly become https-only while the key rule was being relaxed.
        assert!(validate_transport_inputs("http://127.0.0.1:8000/v1", "").is_ok());
        // Blank is allowed; blank-with-a-newline is still header injection.
        assert!(validate_transport_inputs("http://localhost:11434/v1", " \n ").is_err());
        // Relaxing the key must not have relaxed anything else on the same path.
        assert!(validate("local", "localhost:11434", "llama3", "").is_err());
        assert!(validate("local", "http://localhost:11434/v1", "", "").is_err());
    }

    /// One unreadable row must not cost the user every other endpoint.
    #[test]
    fn an_unreadable_row_is_dropped_not_fatal() {
        let v = serde_json::json!({
            "custom_llm_providers": [
                {"id":"a","vendor":"v","name":"A","base_url":"https://x.test",
                 "model":"m","api_key":"k","rungs":{}},
                {"id":"broken"}
            ]
        });
        let rows = read_rows(&v);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "a");
    }

    /// Absent registry = no endpoints, not an error (every settings.json predating this).
    #[test]
    fn a_settings_file_without_the_registry_reads_as_empty() {
        assert!(read_rows(&serde_json::json!({"llm_provider": "local"})).is_empty());
    }

    /// The view is the wire form; it must not carry the key, however the row is built.
    #[test]
    fn the_view_cannot_carry_the_api_key() {
        let v = CustomProviderView::of(&row("a", "A"), Some("a"));
        let json = serde_json::to_string(&v).unwrap();
        assert!(
            !json.contains("secret"),
            "the key must never reach the UI: {json}"
        );
        assert!(
            !json.contains("api_key"),
            "the view must have no key field: {json}"
        );
        assert!(v.selected);
    }

    /// A row is only "selected" when `llm_provider` actually says custom AND names it.
    #[test]
    fn selection_requires_both_the_kind_and_the_id() {
        assert_eq!(
            selected_custom_id(
                &serde_json::json!({"llm_provider":"custom","llm_provider_custom_id":"a"})
            ),
            Some("a".to_string())
        );
        // Configured but not chosen — a stale id must not read as selected.
        assert_eq!(
            selected_custom_id(
                &serde_json::json!({"llm_provider":"local","llm_provider_custom_id":"a"})
            ),
            None
        );
        assert_eq!(
            selected_custom_id(&serde_json::json!({"llm_provider":"custom"})),
            None
        );
    }

    /// A refresh cut short by a rate limit must not be able to demote the row - that is what
    /// would strand a SELECTED endpoint below the gate with nothing to catch it (the gate
    /// only runs on the settings write).
    #[test]
    fn an_interrupted_refresh_cannot_lower_what_was_already_measured() {
        let mut stored = std::collections::BTreeMap::from([
            ("activity_report".to_string(), SchemaRung::Strict),
            ("workstream".to_string(), SchemaRung::Strict),
            ("worklog_generate".to_string(), SchemaRung::JsonSchema),
            ("plan_task_draft".to_string(), SchemaRung::Strict),
        ]);
        // A refresh that got one schema in before the 429 reports only that one.
        merge_rungs(
            &mut stored,
            std::collections::BTreeMap::from([("activity_report".to_string(), SchemaRung::Strict)]),
        );
        assert_eq!(
            stored.len(),
            4,
            "the untouched schemas keep their measurement"
        );
        assert_eq!(stored["worklog_generate"], SchemaRung::JsonSchema);
    }

    /// The other half of the same rule: a refresh that DOES reach a schema records what it
    /// found, downgrade included - otherwise re-testing could never discover bad news.
    #[test]
    fn a_completed_refresh_records_a_downgrade() {
        let mut stored =
            std::collections::BTreeMap::from([("workstream".to_string(), SchemaRung::Strict)]);
        merge_rungs(
            &mut stored,
            std::collections::BTreeMap::from([("workstream".to_string(), SchemaRung::JsonObject)]),
        );
        assert_eq!(stored["workstream"], SchemaRung::JsonObject);
    }

    /// An unmeasured endpoint is Lab-only, and the view says so rather than making the UI
    /// re-derive the rule.
    #[test]
    fn the_view_carries_the_gate_verdict() {
        let v = CustomProviderView::of(&row("a", "A"), None);
        assert!(!v.production_eligible);
        assert!(!v.fully_probed);
        assert_eq!(v.effective_rung, SchemaRung::None);
        assert!(!v.selected);
    }
}
