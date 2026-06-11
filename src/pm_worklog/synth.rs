//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// The one LLM hop of Stage 4: POST the collected bundle to the MLX server's
// `/synthesise_worklog` endpoint (which runs the agno synth agent in-process)
// and get back a `JiraUpdate`. Wrapped in the single global LLM gate so it can
// never run concurrently with a classify or summarise call on the shared model.

use std::time::Duration;

use anyhow::{Context, Result};
use serde_json::json;

use super::config::PmWorklogConfig;
use super::models::{JiraUpdate, SessionBundle};

/// Synthesise one worklog from a session bundle. Holds the global LLM permit for
/// the whole request, then releases it.
pub async fn synthesise(bundle: &SessionBundle, cfg: &PmWorklogConfig) -> Result<JiraUpdate> {
    // Single global LLM gate — one model call in flight across all stages.
    let _llm_permit = crate::llm_gate::acquire().await;

    let url = format!(
        "http://{}:{}/synthesise_worklog",
        cfg.mlx_host, cfg.mlx_port
    );
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(cfg.synth_timeout_s))
        .build()
        .context("building synth http client")?;

    let resp = client
        .post(&url)
        .json(&json!({ "bundle": bundle }))
        .send()
        .await
        .with_context(|| {
            format!(
                "synth endpoint unreachable at {url} — is the MLX server running with --backend mlx?"
            )
        })?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("/synthesise_worklog returned {status}: {body}");
    }

    resp.json::<JiraUpdate>()
        .await
        .context("parsing JiraUpdate from synth response")
}
