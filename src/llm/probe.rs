//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Measuring what a custom endpoint actually honours — the gate's evidence.
//!
//! # Why measure instead of tabulate
//! "OpenAI-compatible" is not one contract. Measured 2026-07-17 with real requests:
//! OpenAI rejects the workstream schema outright when `required` omits an optional key
//! (400 `invalid_json_schema`), while Gemini's compat endpoint accepts the very same
//! schema. A hardcoded per-vendor capability table would therefore have been wrong for
//! someone on day one, and could never answer for a hand-entered base URL at all. So an
//! endpoint is probed once, when it is added, and what it DID is recorded on its row.
//!
//! # Why every schema, not one
//! The pipeline sends four, and they do not demand the same things — `worklog_generate`
//! carries a `["object","null"]` union and deeper nesting than `workstream`. Support is
//! not guaranteed uniform across them, so each is probed separately and the gate reads
//! the weakest ([`meridian_core::CustomLlmProvider::effective_rung`]). Probing one and
//! generalising is the same trap as probing with a toy schema: measured, a trivial
//! `{"answer":"string"}` schema PASSES on an endpoint where the real workstream schema
//! 400s, because the violation sits in a nested `items` object.
//!
//! # This spends real money
//! Every attempt is one metered request against the user's own key: worst case
//! `schemas × rungs` (16). It must therefore only ever run on an explicit user action —
//! adding an endpoint, or pressing Test — never on a mount, a poll tick, or a rescan.
//! The prompts are deliberately minimal so the bill is a few hundred tokens.
//!
//! # Who calls this
//! The tray's `add_custom_llm_provider` / `probe_custom_llm_provider` commands — and only
//! those, on an explicit user action. The gate that reads the result is
//! [`meridian_core::CustomLlmProvider::effective_rung`], enforced on the settings write.

use std::collections::BTreeMap;

use meridian_core::{CustomLlmProvider, SchemaRung};
use serde_json::Value;

use super::openai_compat::{CustomEndpoint, OpenAiCompatBackend};
use super::{LlmBackend, LlmConfig, LlmError, PromptRequest};

/// A probe answer is one word of JSON, not an hour of work — but it must still be big
/// enough for the model to close the object it opened, or a truncated answer reads as a
/// failed rung when the endpoint was fine.
const PROBE_MAX_TOKENS: u32 = 512;

/// A probe must not inherit the pipeline's generous per-call budget: the user is watching
/// a spinner, and a rung that needs 5 minutes to answer is not a rung worth having.
const PROBE_TIMEOUT_S: u64 = 45;

/// Deliberately content-free. The probe asks "does this endpoint honour this SHAPE", not
/// "is this model any good" — that question is the LLM Lab's, and answering it here would
/// make every probe an expensive, slow, and differently-flaky thing.
const PROBE_SYSTEM: &str =
    "You answer only in JSON matching the required schema. Reply with the smallest valid \
     answer: empty arrays and empty strings wherever the schema allows them.";

const PROBE_USER: &str = "Return the minimal valid answer.";

/// The schema behind each key in [`meridian_core::settings::PIPELINE_SCHEMA_KEYS`].
///
/// That list is the gate's definition of a complete measurement; this is the other half of
/// the pact — the actual schema to send for each key. `None` for an unknown key would mean
/// the gate demands evidence this module cannot produce, so the two are kept in step by a
/// test rather than by hope.
fn schema_for(key: &str) -> Option<Value> {
    match key {
        "activity_report" => Some(super::prompts::activity_report_schema()),
        "workstream" => Some(super::prompts::workstream_schema()),
        "worklog_generate" => Some(super::prompts::worklog_generate_schema()),
        "plan_task_draft" => Some(super::prompts::plan_task_draft_schema()),
        _ => None,
    }
}

/// The rungs to try, strongest first. The first that answers usably wins — there is no
/// point paying for a weaker one once a stronger one works.
///
/// [`SchemaRung::None`] is absent by design: it is the RESULT of every rung failing, never
/// something to attempt.
const LADDER: [SchemaRung; 4] = [
    SchemaRung::Strict,
    SchemaRung::JsonSchema,
    SchemaRung::JsonObject,
    SchemaRung::Prompt,
];

/// What a probe cost and found.
#[derive(Debug, Clone)]
pub struct ProbeReport {
    /// The row's rungs, with everything this run measured merged in. Complete only when
    /// `incomplete` is `None`.
    pub rungs: BTreeMap<String, SchemaRung>,
    /// Real requests spent by THIS run. Surfaced so "why did adding a provider cost 16
    /// calls" is answerable.
    pub requests: u32,
    /// Why the probe stopped early, if it did — a rate limit, a dead key. The measurements
    /// above are still real and still worth keeping; the endpoint just isn't gate-eligible
    /// until a retry finishes the job.
    pub incomplete: Option<String>,
}

/// Probe every schema `row` has not already been measured against, and return its rungs with
/// the new results merged in.
///
/// # Why this resumes rather than restarts
/// Each attempt is a metered request. A free-tier key 429s partway through the very first
/// probe (observed: three schemas measured, then quota) — restarting from scratch would
/// re-buy answers already on the row, and on a per-minute quota it would fail in the same
/// place forever. So an already-measured schema is skipped, and a stopped probe is resumed
/// by simply calling this again.
///
/// # Why stopping early is not an error
/// A rate limit means "ask me later", not "this endpoint cannot hold a schema". Recording
/// [`SchemaRung::None`] for it would gate a perfectly good endpoint out of production on a
/// transient, and it would be indistinguishable from a real refusal forever after. So the
/// partial result is returned with `incomplete` set; the gate independently refuses to pass
/// an endpoint whose measurement has holes
/// ([`meridian_core::CustomLlmProvider::effective_rung`]), so a half-probed row is safe
/// without this function having to lie about it.
#[tracing::instrument(skip(row), fields(endpoint_id = %row.id, model = %row.model))]
pub async fn probe_endpoint(row: &CustomLlmProvider) -> ProbeReport {
    let mut rungs = row.rungs.clone();
    let mut requests = 0u32;
    let todo = row.unmeasured_schemas();

    tracing::info!(
        todo = ?todo,
        already_measured = rungs.len(),
        "probe: starting (each attempt is one metered request)"
    );

    for key in todo {
        let Some(schema) = schema_for(key) else {
            // The gate demands this key; nothing here can produce it. Loud, because it means
            // the two halves of the pact have drifted and NOTHING will ever pass the gate.
            tracing::error!(
                schema = key,
                "probe: no schema for a gated key - cannot measure"
            );
            return ProbeReport {
                rungs,
                requests,
                incomplete: Some(format!("{key}: no schema to probe with (internal)")),
            };
        };
        match probe_one_schema(row, &schema, key).await {
            Ok((rung, spent)) => {
                requests += spent;
                tracing::info!(
                    schema = key,
                    ?rung,
                    requests = spent,
                    "probe: schema measured"
                );
                rungs.insert(key.to_string(), rung);
            }
            Err((reason, spent)) => {
                requests += spent;
                tracing::warn!(schema = key, requests, reason = %reason, "probe: stopped early");
                return ProbeReport {
                    rungs,
                    requests,
                    incomplete: Some(reason),
                };
            }
        }
    }

    let report = ProbeReport {
        rungs,
        requests,
        incomplete: None,
    };
    tracing::info!(requests = report.requests, "probe: endpoint fully measured");
    report
}

/// Walk the ladder for one schema. `Ok` = the first rung that answered usably (or
/// [`SchemaRung::None`] if every rung refused — a real result). `Err` = stop the whole probe.
///
/// Both arms report requests spent, because the caller is accounting for real money either
/// way.
async fn probe_one_schema(
    row: &CustomLlmProvider,
    schema: &Value,
    name: &str,
) -> Result<(SchemaRung, u32), (String, u32)> {
    let mut spent = 0u32;
    for rung in LADDER {
        spent += 1;
        match attempt(row, schema, rung).await {
            Ok(()) => return Ok((rung, spent)),
            // "Ask me later" — never a statement about the schema. Stop, keep what we have.
            Err(e @ LlmError::RateLimited { .. }) => {
                return Err((
                    format!("rate-limited while probing {name} ({e}) - retry to resume"),
                    spent,
                ))
            }
            Err(LlmError::Failed(msg)) if is_fatal(&msg) => {
                return Err((format!("{name}: {msg}"), spent));
            }
            Err(e) => {
                // The expected case for a rung this endpoint doesn't support. Debug, not
                // warn: walking down the ladder IS the algorithm, not a fault.
                tracing::debug!(schema = name, ?rung, error = %e, "probe: rung refused");
            }
        }
    }
    Ok((SchemaRung::None, spent))
}

/// Is this failure about the ENDPOINT rather than the rung? A bad key answers nothing at
/// any rung, so walking the ladder would spend three more requests to learn the same thing
/// and then record a confident, wrong `None`.
///
/// This is a substring contract on the messages [`crate::llm::openai_compat`] produces —
/// "rejected the API key", "custom provider unreachable", "selected but not configured". It
/// matches those PRODUCER PHRASES, not bare fragments like "API key": a rung refusal is an
/// ordinary 400 whose raw vendor body can incidentally mention an API key, and treating that
/// as endpoint-fatal would abort the ladder and record a confident, wrong `None`. It is
/// deliberately prose-matching (not a structured `LlmError` variant) for now; the guard
/// against a producer reword silently breaking classification is two-sided coverage:
/// `openai_compat`'s own tests pin that it emits those substrings, and
/// [`tests::endpoint_level_failures_are_fatal_but_rung_refusals_are_not`] pins that this fn
/// treats them as fatal. Promoting `LlmError::Failed` to carry a reason enum would remove
/// the guessing entirely and is tracked as a follow-up.
fn is_fatal(msg: &str) -> bool {
    msg.contains("rejected the API key")
        || msg.contains("custom provider unreachable")
        || msg.contains("selected but not configured")
}

/// One real request at one rung. `Ok(())` means the endpoint accepted the mode AND returned
/// something the pipeline could actually read.
///
/// Parsing is part of the test on purpose: an endpoint that accepts `response_format` and
/// then answers prose has not honoured the rung, it has only agreed to be asked.
async fn attempt(
    row: &CustomLlmProvider,
    schema: &Value,
    rung: SchemaRung,
) -> Result<(), LlmError> {
    // The reason this module drove the pacing work. Unbounded (`None`) on purpose: unlike a
    // production call there is no one waiting on a specific answer, and a probe that takes a
    // minute and COMPLETES beats one that returns in two seconds having 429'd partway and
    // left the endpoint unmeasured — which the production gate then refuses to pass.
    // `probe_endpoint` goes through this for every attempt, so the whole 16-request worst
    // case is spread, not just the gaps between schemas.
    super::rate_limit::acquire(&super::rate_limit::custom_key(&row.id), row.rpm, None).await;

    let cfg = LlmConfig {
        model: String::new(),
        meridian_home: std::env::temp_dir(),
        cli_timeout_s: PROBE_TIMEOUT_S,
        custom: Some(CustomEndpoint {
            id: row.id.clone(),
            base_url: row.base_url.clone(),
            model: row.model.clone(),
            api_key: row.api_key.clone(),
            rpm: row.rpm,
            rung,
        }),
    };
    let req = PromptRequest {
        system: PROBE_SYSTEM,
        user: PROBE_USER.to_string(),
        schema: Some(schema.clone()),
        max_tokens: PROBE_MAX_TOKENS,
        label: format!("probe {rung:?}"),
    };
    let out = OpenAiCompatBackend { cfg }.complete(&req).await?;
    match super::parse_json_object(&out.text) {
        Some(_) => Ok(()),
        None => Err(LlmError::Failed(
            "answered, but not with JSON the pipeline can read".to_string(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two halves of the pact must line up: the gate demands a rung for every key, and
    /// this module must be able to produce one. If a key here had no schema, NOTHING could
    /// ever pass the gate — and it would look like every endpoint was simply bad.
    #[test]
    fn every_gated_key_has_a_schema_to_probe_with() {
        for key in meridian_core::settings::PIPELINE_SCHEMA_KEYS {
            let schema = schema_for(key).unwrap_or_else(|| panic!("no schema for gated key {key}"));
            assert!(
                schema.is_object(),
                "{key}: a probed schema must be a real schema"
            );
        }
    }

    /// The reverse drift: a schema this module knows but the gate never asks for would be
    /// measured and paid for, then ignored.
    #[test]
    fn the_probe_measures_nothing_the_gate_does_not_ask_for() {
        assert!(schema_for("workstream_v2").is_none());
        assert!(schema_for("").is_none());
    }

    /// Strongest first, and `None` is a result rather than an attempt.
    #[test]
    fn the_ladder_runs_strongest_first_and_never_attempts_none() {
        let mut sorted = LADDER;
        sorted.sort_by(|a, b| b.cmp(a));
        assert_eq!(LADDER, sorted, "the ladder must be strongest-first");
        assert!(!LADDER.contains(&SchemaRung::None));
    }

    /// A bad key must abort rather than spend three more requests proving the same thing
    /// and then recording a confident, wrong `None`.
    #[test]
    fn endpoint_level_failures_are_fatal_but_rung_refusals_are_not() {
        assert!(is_fatal(
            "custom provider rejected the API key (401) - check it in Settings"
        ));
        assert!(is_fatal("custom provider unreachable: dns error"));
        assert!(is_fatal("custom provider is selected but not configured"));
        // A schema/mode refusal is the ordinary case — it must keep walking.
        assert!(!is_fatal(
            "custom provider 400: Invalid schema for response_format: Missing 'id'."
        ));
        assert!(!is_fatal(
            "custom provider 400: response_format is not supported"
        ));
        // A 400 whose raw vendor body merely mentions an API key is still a rung
        // refusal, not an endpoint failure — matching bare "API key" would misfire.
        assert!(!is_fatal(
            "custom provider 400: this model requires an API key with the beta header for schemas"
        ));
    }

    /// The live probe against a real endpoint — the only test that proves the ladder
    /// measures anything. Spends real requests; see this module's docs.
    ///
    /// ```text
    /// MERIDIAN_TEST_LLM_BASE=https://generativelanguage.googleapis.com/v1beta/openai \
    /// MERIDIAN_TEST_LLM_MODEL=gemini-flash-latest \
    /// MERIDIAN_TEST_LLM_KEY=<key> \
    ///   cargo test --lib llm::probe -- --ignored --nocapture
    /// ```
    #[tokio::test]
    #[ignore = "spends up to 16 real metered requests; needs MERIDIAN_TEST_LLM_{BASE,MODEL,KEY}"]
    async fn live_probe_measures_every_schema() {
        let (Ok(base_url), Ok(model), Ok(api_key)) = (
            std::env::var("MERIDIAN_TEST_LLM_BASE"),
            std::env::var("MERIDIAN_TEST_LLM_MODEL"),
            std::env::var("MERIDIAN_TEST_LLM_KEY"),
        ) else {
            panic!("set MERIDIAN_TEST_LLM_{{BASE,MODEL,KEY}} — see this test's docs");
        };

        let mut row = CustomLlmProvider {
            id: "live".into(),
            vendor: "other".into(),
            name: "live".into(),
            base_url,
            model,
            api_key,
            // The live probe deliberately runs UNPACED (`0`): this test exists to prove the
            // resume path still works when a real free tier cuts us off partway. Pacing it
            // would make the very case it covers unreachable.
            rpm: 0,
            rpd: 0,
            rungs: BTreeMap::new(),
        };

        // Resume until complete. A free-tier key is rate-limited per MINUTE, so the first
        // pass routinely stops partway — which is exactly the case this loop exists to
        // prove: each pass keeps what it measured and buys only what is still missing.
        let mut total = 0u32;
        for pass in 1..=4 {
            let report = probe_endpoint(&row).await;
            total += report.requests;
            println!(
                "pass {pass}: +{} requests, measured {}/{}{}",
                report.requests,
                report.rungs.len(),
                meridian_core::settings::PIPELINE_SCHEMA_KEYS.len(),
                report
                    .incomplete
                    .as_ref()
                    .map(|r| format!(" — stopped: {r}"))
                    .unwrap_or_default()
            );
            row.rungs = report.rungs;
            if report.incomplete.is_none() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_secs(35)).await; // let a per-minute quota recover
        }

        for k in meridian_core::settings::PIPELINE_SCHEMA_KEYS {
            println!("  {k:>16}: {:?}", row.rungs.get(k));
        }
        println!(
            "total requests: {total} | effective: {:?} | production-eligible: {}",
            row.effective_rung(),
            row.is_production_eligible()
        );
        assert!(row.is_fully_probed(), "every schema must end up measured");
    }
}
