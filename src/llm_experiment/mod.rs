//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Dev-only LLM comparison harness ("LLM Lab") — replay one prose stage across
//! several provider/model variants and persist every outcome side by side.
//!
//! One **experiment** = (process, input ref, N variants). The variant-independent
//! [`crate::llm::PromptRequest`] is rebuilt from stored pipeline inputs
//! ([`request`]), snapshotted onto the experiment row, then fanned **sequentially**
//! across the variants ([`runner`]) via [`crate::llm::resolver::backend_for`] — one
//! level *below* [`crate::llm::complete`], so a variant's rate-limit or failure is
//! recorded as that variant's outcome instead of being silently substituted by the
//! resolver's on-device fallback.
//!
//! Experiments NEVER write production tables: `pm_worklog_hours` / `day_tasks` /
//! `pm_tasks` are read as inputs; outputs land only in `llm_experiments` /
//! `llm_experiment_results` (migration 061). Rendering stops at "what the pipeline
//! would have made of the answer" — nothing is persisted back.
//!
//! Replay fidelity caveats, accepted for A/B comparison (all variants still get
//! identical input):
//! * The workstream fold replays with *today's* stored prior task state, not the
//!   state as it was at the original fold time (that state isn't archived).
//! * Only hours whose distilled `hour_text` is stored can be replayed — the local
//!   MLX distillation is provider-independent and is not re-run.
//!
//! # Who calls this
//! The `meridian llm-experiment` CLI ([`cli`], dispatched from `main.rs`) and,
//! through it, the tray's dev-only `run_llm_experiment` command (the "LLM Lab"
//! modal). Results are read back via `meridian_core::llm_experiments`.
//!
//! # Related
//! - [`crate::llm::resolver`] — `backend_for`, the one provider match.
//! - [`crate::worklog_pipeline::hour`] / [`crate::worklog_pipeline::workstream`] /
//!   [`crate::pm_worklog::generate`] — the extracted request builders replayed here.

pub mod cli;
pub mod day_state;
pub mod request;
pub mod runner;
pub mod store;

use anyhow::{bail, Context, Result};
use serde_json::Value;

use crate::llm::{prompts, LlmProvider};

/// Which prose stage an experiment replays. Wire forms match the `process` column
/// of `llm_experiments` and the UI's process picker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExperimentProcess {
    /// The hourly activity report ([`crate::worklog_pipeline::hour`]).
    HourReport,
    /// The workstream / day-task fold ([`crate::worklog_pipeline::workstream`]).
    WorkstreamFold,
    /// The day-task worklog draft ([`crate::pm_worklog::generate`]).
    WorklogGenerate,
    /// A whole day of folds, chained: every stored hour report of the day is
    /// folded in order, each variant evolving its OWN in-memory task state from
    /// empty — so the final day-task timelines are comparable model-for-model.
    /// One request per stored hour per variant (not a single snapshot request).
    DayFold,
}

impl ExperimentProcess {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::HourReport => "hour_report",
            Self::WorkstreamFold => "workstream_fold",
            Self::WorklogGenerate => "worklog_generate",
            Self::DayFold => "day_fold",
        }
    }

    /// Parse a wire form. Tolerates the CLI's hyphenated spelling
    /// (`hour-report` == `hour_report`). Unknown → `None`.
    pub fn from_wire(s: &str) -> Option<Self> {
        match s.replace('-', "_").as_str() {
            "hour_report" => Some(Self::HourReport),
            "workstream_fold" => Some(Self::WorkstreamFold),
            "worklog_generate" => Some(Self::WorklogGenerate),
            "day_fold" => Some(Self::DayFold),
            _ => None,
        }
    }

    /// The variant-independent request contract for this process: the system
    /// prompt, the output schema (if the stage uses one), and the token ceiling.
    /// These are re-derived from the process — never stored — so the snapshot on
    /// the experiment row stays small and a replay always uses the stage's real,
    /// current contract.
    pub fn contract(&self) -> (&'static str, Option<Value>, u32) {
        match self {
            Self::HourReport => (
                prompts::ACTIVITY_REPORT,
                // No schema: the report is plain text, parsed by `hour::parse_report`.
                None,
                crate::worklog_pipeline::hour::REPORT_MAX_TOKENS,
            ),
            // The day fold sends the same per-hour request the hour fold does —
            // one contract, N chained calls (see the runner's day-fold branch).
            Self::WorkstreamFold | Self::DayFold => (
                prompts::WORKSTREAM,
                Some(prompts::workstream_schema()),
                crate::worklog_pipeline::workstream::WORKSTREAM_MAX_TOKENS,
            ),
            Self::WorklogGenerate => (
                prompts::WORKLOG_GENERATE,
                Some(prompts::worklog_generate_schema()),
                crate::pm_worklog::generate::GENERATE_MAX_TOKENS,
            ),
        }
    }
}

/// One provider/model combination an experiment fans out to. `params` is the
/// future-proofing slot (prompt version, temperature, …) — persisted as
/// `params_json`, currently always empty.
#[derive(Debug, Clone)]
pub struct Variant {
    pub provider: LlmProvider,
    /// Model override within the provider (`--model`). `None` = provider default.
    pub model: Option<String>,
    pub params: Option<Value>,
}

impl Variant {
    /// Parse one CLI variant token: `"codex"` (provider default model) or
    /// `"codex:gpt-5.1-codex"` (explicit model). Unknown providers error with the
    /// valid id list.
    pub fn parse(s: &str) -> Result<Self> {
        let s = s.trim();
        if s.is_empty() {
            bail!("empty variant - want <provider> or <provider>:<model>");
        }
        let (provider_str, model) = match s.split_once(':') {
            Some((p, m)) if !m.trim().is_empty() => (p.trim(), Some(m.trim().to_string())),
            Some((p, _)) => (p.trim(), None),
            None => (s, None),
        };
        let provider = LlmProvider::from_wire(provider_str).with_context(|| {
            let valid: Vec<&str> = LlmProvider::all().iter().map(|p| p.as_str()).collect();
            format!(
                "unknown provider {provider_str:?} - want one of: {}",
                valid.join(", ")
            )
        })?;
        Ok(Self {
            provider,
            model,
            params: None,
        })
    }
}

/// Parse the CLI's `--variants` list: comma-separated [`Variant::parse`] tokens.
pub fn parse_variants(csv: &str) -> Result<Vec<Variant>> {
    let variants: Vec<Variant> = csv
        .split(',')
        .filter(|t| !t.trim().is_empty())
        .map(Variant::parse)
        .collect::<Result<_>>()?;
    if variants.is_empty() {
        bail!("no variants given - want e.g. --variants claude,codex:gpt-5.1,local");
    }
    Ok(variants)
}

/// The fixed past input an experiment replays.
#[derive(Debug, Clone)]
pub enum ExperimentInput {
    /// A local hour label `YYYY-MM-DDTHH` (hour report / workstream fold).
    Hour(String),
    /// A day-task card (worklog generate).
    DayTask { day: String, task_id: String },
    /// A whole local day `YYYY-MM-DD` (day fold).
    Day(String),
}

impl ExperimentInput {
    /// The human `input_ref` stored on the experiment row:
    /// `YYYY-MM-DDTHH`, `YYYY-MM-DD/<task_id>`, or `YYYY-MM-DD`.
    pub fn ref_str(&self) -> String {
        match self {
            Self::Hour(label) => label.clone(),
            Self::DayTask { day, task_id } => format!("{day}/{task_id}"),
            Self::Day(day) => day.clone(),
        }
    }
}

/// Everything needed to create + run one experiment.
#[derive(Debug, Clone)]
pub struct ExperimentSpec {
    pub process: ExperimentProcess,
    pub input: ExperimentInput,
    pub variants: Vec<Variant>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_wire_round_trips_and_tolerates_hyphens() {
        for p in [
            ExperimentProcess::HourReport,
            ExperimentProcess::WorkstreamFold,
            ExperimentProcess::WorklogGenerate,
            ExperimentProcess::DayFold,
        ] {
            assert_eq!(ExperimentProcess::from_wire(p.as_str()), Some(p));
        }
        assert_eq!(
            ExperimentProcess::from_wire("hour-report"),
            Some(ExperimentProcess::HourReport)
        );
        assert_eq!(ExperimentProcess::from_wire("distill"), None);
    }

    #[test]
    fn contract_matches_each_stage() {
        let (sys, schema, max) = ExperimentProcess::HourReport.contract();
        assert!(!sys.is_empty());
        assert!(schema.is_none(), "the hour report is plain text");
        assert_eq!(max, crate::worklog_pipeline::hour::REPORT_MAX_TOKENS);

        let (_, schema, _) = ExperimentProcess::WorkstreamFold.contract();
        assert!(schema.is_some());
        let (_, schema, _) = ExperimentProcess::WorklogGenerate.contract();
        assert!(schema.is_some());
        // The day fold rides the hour fold's exact per-call contract.
        let (sys, _, max) = ExperimentProcess::DayFold.contract();
        let (fold_sys, _, fold_max) = ExperimentProcess::WorkstreamFold.contract();
        assert_eq!(sys, fold_sys);
        assert_eq!(max, fold_max);
    }

    #[test]
    fn variant_parse_round_trips() {
        let v = Variant::parse("codex").unwrap();
        assert_eq!(v.provider, LlmProvider::Codex);
        assert_eq!(v.model, None);

        let v = Variant::parse("codex:gpt-5.1-codex").unwrap();
        assert_eq!(v.provider, LlmProvider::Codex);
        assert_eq!(v.model.as_deref(), Some("gpt-5.1-codex"));

        // A trailing colon means "default model", not an empty override.
        let v = Variant::parse("claude:").unwrap();
        assert_eq!(v.model, None);

        assert!(Variant::parse("").is_err());
        assert!(Variant::parse("gemini").is_err(), "unknown provider");
    }

    #[test]
    fn parse_variants_splits_and_rejects_empty() {
        let vs = parse_variants("claude, codex:gpt-5.1 ,local").unwrap();
        assert_eq!(vs.len(), 3);
        assert_eq!(vs[1].model.as_deref(), Some("gpt-5.1"));
        assert!(parse_variants("").is_err());
        assert!(parse_variants(" , ").is_err());
    }

    #[test]
    fn input_ref_forms() {
        assert_eq!(
            ExperimentInput::Hour("2026-07-15T14".into()).ref_str(),
            "2026-07-15T14"
        );
        assert_eq!(
            ExperimentInput::DayTask {
                day: "2026-07-15".into(),
                task_id: "T2".into()
            }
            .ref_str(),
            "2026-07-15/T2"
        );
    }
}
