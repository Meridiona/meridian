//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Sequential executor: fan the experiment's stored request across its variants.
//!
//! Deliberately one level BELOW [`crate::llm::complete`]: each variant goes straight
//! to [`crate::llm::resolver::backend_for`], so there is **no retry, no rate-limit
//! backoff, and no silent on-device fallback** — a variant's `RateLimited`/`Failed`
//! is recorded as that variant's outcome, which is the whole point of a comparison.
//! (`backend_for` stays the codebase's only match on the provider.)
//!
//! Variants run strictly sequentially: the local backend self-gates on the single
//! GPU permit, and the CLI providers are rate-limited enough that parallel fan-out
//! would mostly measure queueing. A variant error never aborts the experiment —
//! the loop records it and moves on.
//!
//! # Who calls this
//! [`super::cli`] (`run` / `exec`), which the tray's dev-only `run_llm_experiment`
//! command spawns detached.

use anyhow::{Context, Result};
use serde_json::{json, Value};
use sqlx::SqlitePool;
use tracing::Instrument;

use meridian_core::settings::load_runtime_settings;

use crate::llm::{parse_json_object, resolver::backend_for, LlmBackend, LlmConfig, LlmProvider};
use crate::worklog_pipeline::hour::parse_report;
use crate::worklog_pipeline::hour_input::assemble_report;
use crate::worklog_pipeline::task_db::DayTaskRow;
use crate::worklog_pipeline::workstream::workstream_request;
use crate::worklog_pipeline::workstream_parse::parse_placements;
use crate::worklog_pipeline::workstream_state::build_state_json;

use super::{day_state, request, store, ExperimentProcess};

/// Run every not-yet-terminal variant of `experiment_id`, then close the ledger.
/// Resumable: a killed run's `running` rows are simply re-run.
#[tracing::instrument(skip(pool))]
pub async fn exec(pool: &SqlitePool, experiment_id: i64) -> Result<()> {
    let (exp, pending) = store::load_pending(pool, experiment_id).await?;
    let process = ExperimentProcess::from_wire(&exp.process).with_context(|| {
        format!(
            "experiment {experiment_id} has unknown process {:?}",
            exp.process
        )
    })?;
    // The day fold is a chain of per-hour requests (no single snapshot request);
    // everything else replays exactly one stored request.
    let day_fold = if process == ExperimentProcess::DayFold {
        Some(request::day_fold_from_snapshot(&exp.input_json)?)
    } else {
        None
    };
    let single = if day_fold.is_none() {
        Some(request::from_snapshot(process, &exp.input_json)?)
    } else {
        None
    };

    tracing::info!(
        experiment_id,
        process = process.as_str(),
        input_ref = %exp.input_ref,
        n_pending = pending.len(),
        "llm-lab: executing experiment"
    );

    for v in pending {
        let span = tracing::info_span!(
            "llm.experiment.variant",
            experiment_id,
            variant_idx = v.variant_idx,
            provider = %v.provider,
            model = %v.model,
            label = %exp.input_ref,
            status = tracing::field::Empty,
            elapsed_s = tracing::field::Empty,
        );
        async {
            let now = chrono::Utc::now().to_rfc3339();
            store::mark_running(pool, experiment_id, v.variant_idx, &now).await?;

            let outcome = match (&day_fold, &single) {
                (Some((day, hours)), _) => run_day_fold_variant(&v, day, hours).await,
                (None, Some((req, render_ctx))) => run_variant(&v, req, process, render_ctx).await,
                (None, None) => unreachable!("one of day_fold/single is always set"),
            };
            tracing::Span::current().record("status", outcome.status);
            tracing::Span::current().record("elapsed_s", outcome.elapsed_s);
            if let Some(e) = &outcome.error {
                tracing::warn!(
                    provider = %v.provider,
                    error = %e,
                    "llm-lab: variant did not answer"
                );
            }

            let now = chrono::Utc::now().to_rfc3339();
            store::finish_variant(pool, experiment_id, v.variant_idx, &outcome, &now).await
        }
        .instrument(span)
        .await?;
    }

    let now = chrono::Utc::now().to_rfc3339();
    store::finish_experiment(pool, experiment_id, &now).await
}

/// Resolve one variant's backend: the stored provider plus its model override on
/// top of the live settings-derived config.
fn variant_backend(v: &store::StoredVariant) -> Result<Box<dyn LlmBackend>, String> {
    let Some(provider) = LlmProvider::from_wire(&v.provider) else {
        return Err(format!("unknown provider {:?}", v.provider));
    };
    let settings = load_runtime_settings();
    let mut cfg = LlmConfig::from_settings(&settings);
    if !v.model.is_empty() {
        cfg.model = v.model.clone();
    }
    Ok(backend_for(provider, cfg))
}

/// One variant's whole-day chain: fold every stored hour report in order onto the
/// variant's OWN in-memory task state (starting empty, like a real day). A failed
/// hour ends the chain but records the partial day, so what DID fold is still
/// inspectable side by side.
async fn run_day_fold_variant(
    v: &store::StoredVariant,
    day: &str,
    hours: &[(String, String)],
) -> store::VariantOutcome {
    let backend = match variant_backend(v) {
        Ok(b) => b,
        Err(e) => return failure("failed", e),
    };

    let mut state: Vec<DayTaskRow> = Vec::new();
    let mut raw: Vec<Value> = Vec::new();
    let (mut in_tokens, mut out_tokens, mut elapsed) = (0u32, 0u32, 0f64);

    for (label, report) in hours {
        let req = workstream_request(&build_state_json(&state), label, report);
        match backend.complete(&req).await {
            Ok(out) => {
                in_tokens += out.input_tokens;
                out_tokens += out.output_tokens;
                elapsed += out.elapsed_s;
                let now = chrono::Utc::now().to_rfc3339();
                state = day_state::fold_answer(&state, &out.text, day, &now);
                raw.push(json!({ "hour": label, "answer": out.text }));
                tracing::debug!(hour = %label, n_tasks = state.len(), "llm-lab: day-fold hour folded");
            }
            Err(e) => {
                let mut rendered = day_state::day_tasks_json(day, &state);
                rendered["note"] =
                    json!(format!("stopped at {label} - later hours were not folded"));
                return store::VariantOutcome {
                    status: if e.is_rate_limited() {
                        "rate_limited"
                    } else {
                        "failed"
                    },
                    output_text: Some(Value::Array(raw).to_string()),
                    output_rendered: Some(rendered.to_string()),
                    error: Some(format!("{label}: {e}")),
                    input_tokens: in_tokens,
                    output_tokens: out_tokens,
                    elapsed_s: elapsed,
                };
            }
        }
    }

    store::VariantOutcome {
        status: "ok",
        output_rendered: Some(day_state::day_tasks_json(day, &state).to_string()),
        output_text: Some(Value::Array(raw).to_string()),
        error: None,
        input_tokens: in_tokens,
        output_tokens: out_tokens,
        elapsed_s: elapsed,
    }
}

/// One variant, one backend call, one terminal outcome. Never errors — every
/// failure mode becomes a recorded outcome.
async fn run_variant(
    v: &store::StoredVariant,
    req: &crate::llm::PromptRequest,
    process: ExperimentProcess,
    render_ctx: &Value,
) -> store::VariantOutcome {
    let backend = match variant_backend(v) {
        Ok(b) => b,
        Err(e) => return failure("failed", e),
    };

    match backend.complete(req).await {
        Ok(out) => {
            let rendered = render(process, &out.text, render_ctx);
            store::VariantOutcome {
                status: "ok",
                output_rendered: Some(rendered),
                output_text: Some(out.text),
                error: None,
                input_tokens: out.input_tokens,
                output_tokens: out.output_tokens,
                elapsed_s: out.elapsed_s,
            }
        }
        Err(e) => failure(
            if e.is_rate_limited() {
                "rate_limited"
            } else {
                "failed"
            },
            e.to_string(),
        ),
    }
}

fn failure(status: &'static str, error: String) -> store::VariantOutcome {
    store::VariantOutcome {
        status,
        output_text: None,
        output_rendered: None,
        error: Some(error),
        input_tokens: 0,
        output_tokens: 0,
        elapsed_s: 0.0,
    }
}

/// What the pipeline would have made of the raw answer — rendered for the UI,
/// never persisted to production tables.
fn render(process: ExperimentProcess, text: &str, render_ctx: &Value) -> String {
    match process {
        // The report path: parse the "<HH:MM-HH:MM>  N min  …" lines and run the
        // same clamp/fill the pipeline applies, against the hour's measured span.
        ExperimentProcess::HourReport => {
            let (activities, minutes, stamps) = parse_report(text);
            let span = render_ctx
                .get("span_min")
                .and_then(Value::as_i64)
                .unwrap_or(0);
            let assembled = assemble_report(&activities, &minutes, &stamps, span);
            if assembled.is_empty() {
                "(unparseable answer - no activity lines found)".to_string()
            } else {
                assembled
            }
        }
        // The hour fold: APPLY this variant's placements to the snapshotted prior
        // state and render the resulting day-task set in the dashboard's
        // `DayTasksResponse` shape, so the UI shows the day's timeline under this
        // model. An unusable answer keeps the prior state (the fold's own safety
        // rule) and says so via `note`.
        ExperimentProcess::WorkstreamFold => {
            let day = render_ctx.get("day").and_then(Value::as_str).unwrap_or("");
            let prior = day_state::rows_from_json(render_ctx.get("prior").unwrap_or(&Value::Null));
            let now = chrono::Utc::now().to_rfc3339();
            let folded = day_state::fold_answer(&prior, text, day, &now);
            let mut v = day_state::day_tasks_json(day, &folded);
            let no_placements = parse_placements(text).map(|p| p.is_empty()).unwrap_or(true);
            if no_placements {
                v["note"] = json!(
                    "no usable placements in the answer - the prior tasks are shown unchanged"
                );
            }
            v.to_string()
        }
        // Day fold renders inside its own chain (`run_day_fold_variant`); this arm
        // only fires if a stored day_fold row is somehow rendered singly.
        ExperimentProcess::DayFold => day_state::day_tasks_json("", &[]).to_string(),
        // The structured stage: show the JSON object the pipeline would parse.
        ExperimentProcess::WorklogGenerate => match parse_json_object(text) {
            Some(v) => serde_json::to_string_pretty(&v).unwrap_or_else(|_| v.to_string()),
            None => "(unparseable answer - no JSON object found)".to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn render_hour_report_assembles_and_clamps() {
        let text = "08:01-08:46  45 min  Built the thing\n08:50  90 min  Overclaimed";
        let out = render(
            ExperimentProcess::HourReport,
            text,
            &json!({"span_min": 60}),
        );
        assert!(out.contains("08:01-08:46  45 min  Built the thing"));
        // The 90-min claim is clamped to the measured 60-min span, like the pipeline.
        assert!(out.contains("60 min  Overclaimed"), "{out}");
    }

    #[test]
    fn render_structured_stage_pretty_prints_or_flags_garbage() {
        let out = render(
            ExperimentProcess::WorklogGenerate,
            r#"Sure! {"match":{"task_key":"KAN-1"}}"#,
            &json!({}),
        );
        assert!(out.contains("\"task_key\": \"KAN-1\""));
    }

    #[test]
    fn render_hour_fold_applies_placements_to_the_prior_state() {
        let render_ctx = json!({ "day": "2026-07-16", "prior": [] });
        let answer = r#"{"placements":[{"id":"","title":"New work",
            "summary":["started it"],"segments":[{"start":"10:00","end":"10:30"}]}]}"#;
        let out = render(ExperimentProcess::WorkstreamFold, answer, &render_ctx);
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["day"], "2026-07-16");
        assert_eq!(v["tasks"][0]["title"], "New work");
        assert_eq!(v["tasks"][0]["segments"][0]["start"], "10:00");
        assert!(v.get("note").is_none());

        // Garbage answer: prior state (empty) survives, and the note says why.
        let out = render(
            ExperimentProcess::WorkstreamFold,
            "no json here",
            &render_ctx,
        );
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["tasks"], json!([]));
        assert!(v["note"].as_str().unwrap().contains("no usable placements"));
    }
}
