//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Reads for the dev-only LLM-Lab comparison harness (`llm_experiments` /
//! `llm_experiment_results`, migration 064; no route — new work).
//!
//! # What this is
//! The read side of the LLM Lab: list past experiments (newest first, with a done
//! count for progress display) and fetch one experiment's full detail — the input
//! snapshot plus every variant's outcome — for the side-by-side comparison view.
//! Writes live in the daemon (`meridian::llm_experiment::store`); this module is
//! deliberately read-only, per the meridian-core DB-only convention.
//!
//! A pre-064 DB (the tray opens `meridian.db` without running migrations) degrades
//! to empty / `None` instead of erroring — the Lab simply shows no runs until the
//! daemon has applied the migration.
//!
//! # Who calls this
//! The tray's dev-only `get_llm_experiments` / `get_llm_experiment` commands
//! (`tray/src-tauri/src/commands/llm_lab.rs`) → the "LLM Lab" modal; and the
//! `meridian llm-experiment list|get` CLI.
//!
//! # Related
//! - [`crate::day_task_worklogs`] — the production ledger the worklog-generate
//!   process writes; experiments never touch it.

use crate::SqlitePool;
use anyhow::Context;
use serde::Serialize;
use sqlx::FromRow;
use tracing::Instrument;

/// One row of the past-runs list.
#[derive(Debug, Clone, Serialize, FromRow)]
pub struct LlmExperimentSummary {
    pub id: i64,
    /// `hour_report` | `workstream_fold` | `worklog_generate`.
    pub process: String,
    /// `YYYY-MM-DDTHH` or `YYYY-MM-DD/<task_id>`.
    pub input_ref: String,
    /// `running` | `done` | `failed`.
    pub status: String,
    pub n_variants: i64,
    /// Variants already terminal (`ok`/`failed`/`rate_limited`) — the progress numerator.
    pub n_done: i64,
    pub created_at: String,
    pub finished_at: Option<String>,
}

/// One variant's outcome inside [`LlmExperimentDetail`].
#[derive(Debug, Clone, Serialize, FromRow)]
pub struct LlmExperimentResult {
    pub variant_idx: i64,
    pub provider: String,
    /// Empty string = the provider's default model.
    pub model: String,
    pub params_json: String,
    /// `pending` | `running` | `ok` | `failed` | `rate_limited`.
    pub status: String,
    pub output_text: Option<String>,
    pub output_rendered: Option<String>,
    pub error: Option<String>,
    /// The CLI backends report no token counts — 0 there, real numbers from local.
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub elapsed_s: Option<f64>,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
}

/// One experiment with its input snapshot and every variant's outcome.
#[derive(Debug, Clone, Serialize)]
pub struct LlmExperimentDetail {
    pub id: i64,
    pub process: String,
    pub input_ref: String,
    /// `{"user": …, "label": …, "render_ctx": …}` — the exact variant-independent
    /// request, so the UI can show precisely what was sent.
    pub input_json: String,
    pub status: String,
    pub n_variants: i64,
    pub created_at: String,
    pub finished_at: Option<String>,
    pub results: Vec<LlmExperimentResult>,
}

/// Does `llm_experiments` exist yet? (The tray opens the DB without migrations.)
async fn tables_exist(pool: &SqlitePool) -> bool {
    sqlx::query_scalar::<_, i64>(
        "SELECT 1 FROM sqlite_master WHERE type='table' AND name='llm_experiments'",
    )
    .fetch_optional(pool)
    .await
    .ok()
    .flatten()
    .is_some()
}

/// The past-runs list, newest first. Empty on a pre-064 DB.
#[tracing::instrument(skip(pool))]
pub async fn list_experiments(
    pool: &SqlitePool,
    limit: i64,
) -> anyhow::Result<Vec<LlmExperimentSummary>> {
    if !tables_exist(pool).await {
        tracing::debug!("llm_experiments table missing (pre-064 DB) - returning empty");
        return Ok(Vec::new());
    }

    let rows: Vec<LlmExperimentSummary> = sqlx::query_as(
        "SELECT e.id, e.process, e.input_ref, e.status, e.n_variants, \
                (SELECT COUNT(*) FROM llm_experiment_results r \
                 WHERE r.experiment_id = e.id \
                   AND r.status IN ('ok','failed','rate_limited')) AS n_done, \
                e.created_at, e.finished_at \
         FROM llm_experiments e \
         ORDER BY e.created_at DESC, e.id DESC \
         LIMIT ?",
    )
    .bind(limit.max(1))
    .fetch_all(pool)
    .instrument(tracing::debug_span!("llm_experiments.read.list"))
    .await
    .context("listing llm_experiments")?;
    tracing::debug!(rows = rows.len(), "llm_experiments.read.list");

    Ok(rows)
}

/// One experiment's full detail, or `None` for an unknown id / a pre-064 DB.
#[tracing::instrument(skip(pool))]
pub async fn get_experiment(
    pool: &SqlitePool,
    id: i64,
) -> anyhow::Result<Option<LlmExperimentDetail>> {
    if !tables_exist(pool).await {
        tracing::debug!("llm_experiments table missing (pre-064 DB) - returning None");
        return Ok(None);
    }

    /// The experiment row without its results — filled in below.
    #[derive(FromRow)]
    struct ExperimentRow {
        id: i64,
        process: String,
        input_ref: String,
        input_json: String,
        status: String,
        n_variants: i64,
        created_at: String,
        finished_at: Option<String>,
    }

    let exp: Option<ExperimentRow> = sqlx::query_as(
        "SELECT id, process, input_ref, input_json, status, n_variants, \
                created_at, finished_at \
         FROM llm_experiments WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .instrument(tracing::debug_span!("llm_experiments.read.one"))
    .await
    .context("loading llm_experiments row")?;
    let Some(exp) = exp else {
        return Ok(None);
    };

    let results: Vec<LlmExperimentResult> = sqlx::query_as(
        "SELECT variant_idx, provider, model, params_json, status, \
                output_text, output_rendered, error, \
                input_tokens, output_tokens, elapsed_s, started_at, finished_at \
         FROM llm_experiment_results \
         WHERE experiment_id = ? \
         ORDER BY variant_idx",
    )
    .bind(id)
    .fetch_all(pool)
    .instrument(tracing::debug_span!("llm_experiments.read.results"))
    .await
    .context("loading llm_experiment_results rows")?;
    tracing::debug!(rows = results.len(), "llm_experiments.read.results");

    tracing::info!(experiment_id = exp.id, "llm_experiments: detail served");
    Ok(Some(LlmExperimentDetail {
        id: exp.id,
        process: exp.process,
        input_ref: exp.input_ref,
        input_json: exp.input_json,
        status: exp.status,
        n_variants: exp.n_variants,
        created_at: exp.created_at,
        finished_at: exp.finished_at,
        results,
    }))
}
