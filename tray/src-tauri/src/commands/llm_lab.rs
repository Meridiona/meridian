//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! The dev-only "LLM Lab" commands - run and read multi-provider comparison
//! experiments (no route - new work).
//!
//! # What this is
//! Four commands behind the LLM Lab, a **development-only** full-screen surface for
//! comparing how the pipeline's prose stages come out across LLM providers:
//! - [`run_llm_experiment`] — start one: shells `meridian llm-experiment create`
//!   (fast, returns the id), then spawns `exec --id N` **detached** — an N-variant
//!   run can far outlive any reasonable invoke budget, so the UI polls instead.
//! - [`get_llm_experiments`] / [`get_llm_experiment`] — the past-runs list and the
//!   per-variant detail the UI polls while a run progresses.
//! - [`draft_lab_worklog`] — the sidebar's on-demand "draft this task with the
//!   shown variant": shells `meridian llm-experiment draft-task` and returns the
//!   model's answer. EPHEMERAL (writes nothing) but fires a REAL, metered
//!   completion, so the UI puts a free/local caution on the button.
//!
//! Every command is refused outside a dev build ([`dev_only`]): the Lab must not
//! exist for users, even against a hand-crafted `invoke`. The read/run
//! `llm-experiment` subcommands stay ungated for field debugging (they only write
//! local experiment tables), but `draft-task` is dev-gated in the CLI too, since it
//! alone calls a live provider and can incur cost. UI visibility rides
//! `get_app_info().channel === 'dev'`, the same `cfg!(debug_assertions)` signal, in
//! `MeridianTimelineShell`.
//!
//! # Who calls this
//! Registered in `lib.rs`'s `invoke_handler!`; consumed by
//! `ui/components/timeline/llmlab/` via `ui/lib/bridge.ts` — `load` for the two
//! reads (flat named args), `invoke('run_llm_experiment', {body})` for a run, and
//! `invoke('draft_lab_worklog', {body})` for the sidebar draft.
//!
//! # Related
//! - `src/llm_experiment/` — the daemon-side harness the run command spawns.
//! - [`meridian_core::llm_experiments`] — the reader behind the two read commands.
//! - [`crate::commands::cli_exec`] — the shared CLI spawn helpers.
//! - [`crate::commands::version`] — `build_channel()`, the UI-visibility half of
//!   the dev gate.

use serde::Deserialize;
use std::time::Duration;
use tauri::State;

use meridian_core::llm_experiments::{LlmExperimentDetail, LlmExperimentSummary};

use super::cli_exec::{parse_last_line, run_meridian, spawn_meridian_detached};

/// Refuse outside a dev build. `cfg!(debug_assertions)` is the same signal
/// `version::build_channel()` maps to `"dev"` — a release binary returns an error
/// even to a hand-crafted `invoke`, so the Lab cannot run in shipped builds.
fn dev_only() -> Result<(), String> {
    if cfg!(debug_assertions) {
        Ok(())
    } else {
        Err("LLM Lab is a dev-only surface".to_string())
    }
}

/// POST body for [`run_llm_experiment`] — `invoke` passes `{body}`. Mirrors the
/// CLI's `run`/`create` flags: `hour` for the two hour processes, `day` +
/// `task_id` for worklog-generate, `variants` as `provider[:model]` tokens.
#[derive(Debug, Deserialize)]
pub struct RunLlmExperimentBody {
    /// `hour_report` | `workstream_fold` | `worklog_generate`.
    pub process: String,
    pub hour: Option<String>,
    pub day: Option<String>,
    pub task_id: Option<String>,
    /// e.g. `["claude", "codex:gpt-5.1", "local"]`.
    pub variants: Vec<String>,
}

/// `create`'s one JSON line.
#[derive(Debug, Deserialize)]
struct CreateAck {
    experiment_id: i64,
}

/// Start an experiment: `create` synchronously (input assembly + pending rows,
/// 30 s), then spawn the execution detached and return the id immediately — the
/// modal polls [`get_llm_experiment`] for per-variant progress.
#[tauri::command]
#[tracing::instrument(skip(body), fields(process = %body.process, n_variants = body.variants.len()))]
pub async fn run_llm_experiment(body: RunLlmExperimentBody) -> Result<i64, String> {
    dev_only()?;
    if body.variants.is_empty() {
        return Err("pick at least one provider variant".to_string());
    }

    let mut args: Vec<String> = vec![
        "llm-experiment".into(),
        "create".into(),
        "--process".into(),
        body.process.clone(),
        "--variants".into(),
        body.variants.join(","),
    ];
    match (&body.hour, &body.day, &body.task_id) {
        (Some(hour), _, _) if !hour.is_empty() => {
            args.extend(["--hour".into(), hour.clone()]);
        }
        // A day, with a task for worklog-generate or bare for day-fold — the CLI
        // validates the process/input pairing.
        (_, Some(day), task_id) if !day.is_empty() => {
            args.extend(["--day".into(), day.clone()]);
            if let Some(t) = task_id {
                if !t.is_empty() {
                    args.extend(["--task-id".into(), t.clone()]);
                }
            }
        }
        _ => return Err("pick an hour, a day, or a day + task to replay".to_string()),
    }

    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let stdout = run_meridian(&arg_refs, Duration::from_secs(30), "llm-experiment-create").await?;
    let ack: CreateAck = parse_last_line(&stdout)?;

    spawn_meridian_detached(
        &[
            "llm-experiment".into(),
            "exec".into(),
            "--id".into(),
            ack.experiment_id.to_string(),
        ],
        "llm-experiment-exec",
    )?;
    tracing::info!(experiment_id = ack.experiment_id, "llm-lab: run started");
    Ok(ack.experiment_id)
}

/// The past-runs list (newest first). Empty on a pre-064 DB (the migration that adds
/// the llm_experiment* tables).
#[tauri::command]
#[tracing::instrument(skip(pool))]
pub async fn get_llm_experiments(
    pool: State<'_, Option<meridian_core::SqlitePool>>,
    limit: Option<i64>,
) -> Result<Vec<LlmExperimentSummary>, String> {
    dev_only()?;
    let Some(pool) = pool.inner() else {
        return Err("meridian.db is not open yet".to_string());
    };
    meridian_core::llm_experiments::list_experiments(pool, limit.unwrap_or(20))
        .await
        .map_err(|e| {
            tracing::warn!(error = %e, "get_llm_experiments failed");
            e.to_string()
        })
}

/// One experiment's detail (input snapshot + every variant outcome), or `None`.
/// The modal polls this while the experiment is `running`.
#[tauri::command]
#[tracing::instrument(skip(pool))]
pub async fn get_llm_experiment(
    pool: State<'_, Option<meridian_core::SqlitePool>>,
    id: i64,
) -> Result<Option<LlmExperimentDetail>, String> {
    dev_only()?;
    let Some(pool) = pool.inner() else {
        return Err("meridian.db is not open yet".to_string());
    };
    meridian_core::llm_experiments::get_experiment(pool, id)
        .await
        .map_err(|e| {
            tracing::warn!(error = %e, "get_llm_experiment failed");
            e.to_string()
        })
}

/// POST body for [`draft_lab_worklog`] - the selected fold task plus the variant
/// to draft it with. `task` carries the inline content the daemon drafts from
/// (`{title, summary, minutes}`); it is deliberately NOT looked up by id, because
/// a fold task id is a model's SIMULATED day, not a production `day_tasks` row.
#[derive(Debug, Deserialize)]
pub struct DraftLabWorklogBody {
    pub day: String,
    /// A variant token: `provider`, `provider:model`, or `custom:<endpoint-id>`.
    pub variant: String,
    /// `{title, summary, minutes}` - passed through to the CLI as `--task-json`.
    pub task: serde_json::Value,
}

/// `draft-task`'s one JSON line.
#[derive(Debug, Deserialize)]
struct DraftAck {
    draft: String,
}

/// Draft ONE fold task's worklog with ONE variant, on demand - shells
/// `meridian llm-experiment draft-task` (EPHEMERAL: no experiment row is written)
/// and returns the model's raw answer for the sidebar to render. This fires a
/// REAL, metered completion against the chosen variant, so the UI puts a
/// free/local caution on the button. Dev-only, like the rest of the Lab.
#[tauri::command]
#[tracing::instrument(skip(body), fields(variant = %body.variant, day = %body.day))]
pub async fn draft_lab_worklog(body: DraftLabWorklogBody) -> Result<String, String> {
    dev_only()?;
    let task_json = serde_json::to_string(&body.task).map_err(|e| e.to_string())?;
    let args: Vec<String> = vec![
        "llm-experiment".into(),
        "draft-task".into(),
        "--day".into(),
        body.day.clone(),
        "--variant".into(),
        body.variant.clone(),
        "--task-json".into(),
        task_json,
    ];
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    // The bottleneck is a full worklog-generate completion, which can be slow on a
    // large model - a generous ceiling, not the 30 s the create step uses.
    let stdout = run_meridian(
        &arg_refs,
        Duration::from_secs(180),
        "llm-experiment-draft-task",
    )
    .await?;
    let ack: DraftAck = parse_last_line(&stdout)?;
    tracing::info!("llm-lab: inline worklog draft complete");
    Ok(ack.draft)
}
