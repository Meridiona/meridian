//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Ledger writes for the LLM-Lab tables (`llm_experiments` / `llm_experiment_results`).
//!
//! Mirrors the `etl_runs` pattern: [`create`] inserts the experiment plus one
//! `pending` result row per variant and returns the id fast (the UI gets its handle
//! immediately); [`runner::exec`](super::runner::exec) then walks the pending rows,
//! flipping each `pending → running → ok|failed|rate_limited`, and
//! [`finish_experiment`] closes the ledger. A killed run is resumable — `exec`
//! re-runs anything not terminal.
//!
//! # Who calls this
//! [`super::cli`] (create) and [`super::runner`] (everything else). Reads for the
//! UI live in `meridian_core::llm_experiments`, not here.

use anyhow::{Context, Result};
use sqlx::SqlitePool;

use super::ExperimentSpec;

/// An experiment row as [`load_pending`] returns it.
#[derive(Debug)]
pub struct StoredExperiment {
    pub id: i64,
    pub process: String,
    pub input_ref: String,
    pub input_json: String,
    pub status: String,
}

/// A not-yet-terminal result row (`pending` or `running` — the latter from a
/// killed run being resumed).
#[derive(Debug)]
pub struct StoredVariant {
    pub variant_idx: i64,
    pub provider: String,
    pub model: String,
    pub params_json: String,
    pub status: String,
}

/// One variant's terminal outcome, written by [`finish_variant`].
#[derive(Debug)]
pub struct VariantOutcome {
    /// `ok` | `failed` | `rate_limited`.
    pub status: &'static str,
    /// The model's raw answer (success only).
    pub output_text: Option<String>,
    /// What the pipeline would have made of the answer (success only).
    pub output_rendered: Option<String>,
    pub error: Option<String>,
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub elapsed_s: f64,
}

/// Insert the experiment plus one `pending` result row per variant, atomically.
/// `input_json` is the variant-independent request snapshot
/// ([`super::request::snapshot`]). Returns the new experiment id.
#[tracing::instrument(skip(pool, spec, input_json))]
pub async fn create(
    pool: &SqlitePool,
    spec: &ExperimentSpec,
    input_json: &str,
    now: &str,
) -> Result<i64> {
    let mut tx = pool.begin().await.context("starting experiment insert")?;

    let id: i64 = sqlx::query_scalar(
        "INSERT INTO llm_experiments (process, input_ref, input_json, status, n_variants, created_at) \
         VALUES (?, ?, ?, 'running', ?, ?) RETURNING id",
    )
    .bind(spec.process.as_str())
    .bind(spec.input.ref_str())
    .bind(input_json)
    .bind(spec.variants.len() as i64)
    .bind(now)
    .fetch_one(&mut *tx)
    .await
    .context("inserting llm_experiments row")?;

    for (idx, v) in spec.variants.iter().enumerate() {
        let params = v
            .params
            .as_ref()
            .map(|p| p.to_string())
            .unwrap_or_else(|| "{}".to_string());
        sqlx::query(
            "INSERT INTO llm_experiment_results \
             (experiment_id, variant_idx, provider, model, params_json, status) \
             VALUES (?, ?, ?, ?, ?, 'pending')",
        )
        .bind(id)
        .bind(idx as i64)
        .bind(v.provider.as_str())
        .bind(v.model.as_deref().unwrap_or(""))
        .bind(params)
        .execute(&mut *tx)
        .await
        .context("inserting llm_experiment_results row")?;
    }

    tx.commit().await.context("committing experiment insert")?;
    tracing::info!(
        experiment_id = id,
        process = spec.process.as_str(),
        input_ref = %spec.input.ref_str(),
        n_variants = spec.variants.len(),
        "llm-lab: experiment created"
    );
    Ok(id)
}

/// Load an experiment and its not-yet-terminal variants (idx order) for [`super::runner::exec`].
pub async fn load_pending(
    pool: &SqlitePool,
    id: i64,
) -> Result<(StoredExperiment, Vec<StoredVariant>)> {
    let exp: (i64, String, String, String, String) = sqlx::query_as(
        "SELECT id, process, input_ref, input_json, status FROM llm_experiments WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .context("loading llm_experiments row")?
    .with_context(|| format!("no experiment with id {id}"))?;

    let rows: Vec<(i64, String, String, String, String)> = sqlx::query_as(
        "SELECT variant_idx, provider, model, params_json, status \
         FROM llm_experiment_results \
         WHERE experiment_id = ? AND status IN ('pending', 'running') \
         ORDER BY variant_idx",
    )
    .bind(id)
    .fetch_all(pool)
    .await
    .context("loading pending llm_experiment_results rows")?;

    Ok((
        StoredExperiment {
            id: exp.0,
            process: exp.1,
            input_ref: exp.2,
            input_json: exp.3,
            status: exp.4,
        },
        rows.into_iter()
            .map(|r| StoredVariant {
                variant_idx: r.0,
                provider: r.1,
                model: r.2,
                params_json: r.3,
                status: r.4,
            })
            .collect(),
    ))
}

/// Flip one variant to `running` and stamp `started_at`.
pub async fn mark_running(pool: &SqlitePool, id: i64, idx: i64, now: &str) -> Result<()> {
    sqlx::query(
        "UPDATE llm_experiment_results SET status = 'running', started_at = ? \
         WHERE experiment_id = ? AND variant_idx = ?",
    )
    .bind(now)
    .bind(id)
    .bind(idx)
    .execute(pool)
    .await
    .context("marking variant running")?;
    Ok(())
}

/// Write one variant's terminal outcome.
pub async fn finish_variant(
    pool: &SqlitePool,
    id: i64,
    idx: i64,
    out: &VariantOutcome,
    now: &str,
) -> Result<()> {
    sqlx::query(
        "UPDATE llm_experiment_results SET \
             status = ?, output_text = ?, output_rendered = ?, error = ?, \
             input_tokens = ?, output_tokens = ?, elapsed_s = ?, finished_at = ? \
         WHERE experiment_id = ? AND variant_idx = ?",
    )
    .bind(out.status)
    .bind(&out.output_text)
    .bind(&out.output_rendered)
    .bind(&out.error)
    .bind(out.input_tokens)
    .bind(out.output_tokens)
    .bind(out.elapsed_s)
    .bind(now)
    .bind(id)
    .bind(idx)
    .execute(pool)
    .await
    .context("writing variant outcome")?;
    Ok(())
}

/// Close the experiment: `done` when at least one variant answered, `failed` when
/// every variant errored (rate-limited counts as errored — there is nothing to look at).
pub async fn finish_experiment(pool: &SqlitePool, id: i64, now: &str) -> Result<()> {
    let ok_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM llm_experiment_results WHERE experiment_id = ? AND status = 'ok'",
    )
    .bind(id)
    .fetch_one(pool)
    .await
    .context("counting ok variants")?;
    let status = if ok_count > 0 { "done" } else { "failed" };

    sqlx::query("UPDATE llm_experiments SET status = ?, finished_at = ? WHERE id = ?")
        .bind(status)
        .bind(now)
        .bind(id)
        .execute(pool)
        .await
        .context("closing experiment")?;
    tracing::info!(experiment_id = id, status, "llm-lab: experiment finished");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::LlmProvider;
    use crate::llm_experiment::{ExperimentInput, ExperimentProcess, Variant};
    use sqlx::sqlite::SqliteConnectOptions;
    use std::str::FromStr;

    async fn fresh_db() -> SqlitePool {
        let opts = SqliteConnectOptions::from_str("sqlite::memory:")
            .unwrap()
            .create_if_missing(true);
        let pool = SqlitePool::connect_with(opts).await.unwrap();
        sqlx::migrate!("src/migrations").run(&pool).await.unwrap();
        pool
    }

    fn spec() -> ExperimentSpec {
        ExperimentSpec {
            process: ExperimentProcess::HourReport,
            input: ExperimentInput::Hour("2026-07-15T14".into()),
            variants: vec![
                Variant {
                    provider: LlmProvider::Claude,
                    model: None,
                    params: None,
                },
                Variant {
                    provider: LlmProvider::Local,
                    model: Some("qwen".into()),
                    params: None,
                },
            ],
        }
    }

    #[tokio::test]
    async fn create_inserts_experiment_and_pending_rows() {
        let pool = fresh_db().await;
        let id = create(
            &pool,
            &spec(),
            r#"{"user":"u","label":"l"}"#,
            "2026-07-15T15:00:00Z",
        )
        .await
        .unwrap();
        let (exp, pending) = load_pending(&pool, id).await.unwrap();
        assert_eq!(exp.process, "hour_report");
        assert_eq!(exp.input_ref, "2026-07-15T14");
        assert_eq!(exp.status, "running");
        assert_eq!(pending.len(), 2);
        assert_eq!(pending[0].provider, "claude");
        assert_eq!(pending[0].model, "");
        assert_eq!(pending[1].provider, "local");
        assert_eq!(pending[1].model, "qwen");
    }

    #[tokio::test]
    async fn variant_lifecycle_and_status_math() {
        let pool = fresh_db().await;
        let now = "2026-07-15T15:00:00Z";
        let id = create(&pool, &spec(), "{}", now).await.unwrap();

        mark_running(&pool, id, 0, now).await.unwrap();
        finish_variant(
            &pool,
            id,
            0,
            &VariantOutcome {
                status: "ok",
                output_text: Some("answer".into()),
                output_rendered: Some("rendered".into()),
                error: None,
                input_tokens: 10,
                output_tokens: 20,
                elapsed_s: 1.5,
            },
            now,
        )
        .await
        .unwrap();
        finish_variant(
            &pool,
            id,
            1,
            &VariantOutcome {
                status: "rate_limited",
                output_text: None,
                output_rendered: None,
                error: Some("quota".into()),
                input_tokens: 0,
                output_tokens: 0,
                elapsed_s: 0.0,
            },
            now,
        )
        .await
        .unwrap();

        // Both terminal → nothing pending; one ok → experiment is done.
        let (_, pending) = load_pending(&pool, id).await.unwrap();
        assert!(pending.is_empty());
        finish_experiment(&pool, id, now).await.unwrap();
        let status: String = sqlx::query_scalar("SELECT status FROM llm_experiments WHERE id = ?")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(status, "done");
    }

    #[tokio::test]
    async fn all_variants_failing_marks_the_experiment_failed() {
        let pool = fresh_db().await;
        let now = "2026-07-15T15:00:00Z";
        let id = create(&pool, &spec(), "{}", now).await.unwrap();
        for idx in 0..2 {
            finish_variant(
                &pool,
                id,
                idx,
                &VariantOutcome {
                    status: "failed",
                    output_text: None,
                    output_rendered: None,
                    error: Some("boom".into()),
                    input_tokens: 0,
                    output_tokens: 0,
                    elapsed_s: 0.0,
                },
                now,
            )
            .await
            .unwrap();
        }
        finish_experiment(&pool, id, now).await.unwrap();
        let status: String = sqlx::query_scalar("SELECT status FROM llm_experiments WHERE id = ?")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(status, "failed");
    }

    #[tokio::test]
    async fn a_running_row_is_resumable() {
        // A killed run leaves a variant 'running'; load_pending must return it again.
        let pool = fresh_db().await;
        let now = "2026-07-15T15:00:00Z";
        let id = create(&pool, &spec(), "{}", now).await.unwrap();
        mark_running(&pool, id, 0, now).await.unwrap();
        let (_, pending) = load_pending(&pool, id).await.unwrap();
        assert_eq!(pending.len(), 2, "a running row is still re-runnable");
        assert_eq!(pending[0].status, "running");
    }
}
