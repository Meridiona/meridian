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
/// "Test" clicks, an add racing a remove). Each command does read_settings_value →
/// mutate → write_settings_value, and without this those cycles interleave and
/// lost-update each other — the same bug class 2104c030 fixed for the provider-test
/// cache. The daemon only READS `custom_llm_providers` (via the resolver), so these
/// tray commands are the only writers and a tray-process lock is sufficient. It is a
/// `tokio` mutex because the critical section spans a probe's `.await`.
static REGISTRY_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

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
    if api_key.trim().is_empty() {
        return Err("API key is required".into());
    }
    if !base_url.starts_with("http://") && !base_url.starts_with("https://") {
        return Err("base URL must start with http:// or https://".into());
    }
    // The key becomes an Authorization header and the URL becomes a request line: a newline
    // in either is header injection, exactly as `oo_email`/`oo_password` guard against.
    for (field, v) in [
        ("API key", api_key),
        ("base URL", base_url),
        ("model", model),
    ] {
        if v.contains('\n') || v.contains('\r') {
            return Err(format!("{field} contains invalid characters"));
        }
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
    let mut settings_v = settings::read_settings_value();
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
    write_rows(&mut settings_v, &rows)?;
    settings::write_settings_value(&settings_v).map_err(|e| {
        tracing::warn!(error = %e, "custom_llm: write failed");
        e.to_string()
    })?;

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
    let mut settings_v = settings::read_settings_value();
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

    write_rows(&mut settings_v, &rows)?;
    settings::write_settings_value(&settings_v).map_err(|e| e.to_string())?;

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
    let mut settings_v = settings::read_settings_value();
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

    write_rows(&mut settings_v, &rows)?;
    settings::write_settings_value(&settings_v).map_err(|e| e.to_string())?;
    tracing::info!(endpoint_id = %id, remaining = rows.len(), "custom_llm: endpoint removed");

    let sel = selected_custom_id(&settings_v);
    Ok(rows
        .iter()
        .map(|r| CustomProviderView::of(r, sel.as_deref()))
        .collect())
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
        assert!(validate("n", "https://x.test/v1", "m", "").is_err());
        assert!(validate("n", "ftp://x.test", "m", "k").is_err());
        assert!(validate("n", "https://x.test/v1", "m", "k\nX-Evil: 1").is_err());
        assert!(validate("n", "https://x.test/v1\r\nHost: evil", "m", "k").is_err());
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
