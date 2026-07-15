//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! On-device backend — the local MLX server. Nothing leaves the machine.
//!
//! The default, and the fallback for every other provider. Two things make it different
//! from the CLI backends:
//!
//! * **It holds the LLM gate.** [`crate::llm_gate`] is a single global permit: only one
//!   call may occupy the Metal device at a time. A CLI backend deliberately does NOT take
//!   it — a `claude` call spends the user's cloud quota, not the GPU, so gating it would
//!   serialise the pipeline behind a resource it isn't using.
//! * **Its schema is enforced at the token level** (outlines FSM on the server), so a
//!   schema request here is a guarantee, not a suggestion.
//!
//! Thinking is OFF, deliberately. On this task the `<think>` block is actively harmful:
//! measured across real hours, the 2B burned its entire 2000-token thinking budget, then
//! emitted a report with no clock times at all, 3.5x slower than with thinking off.

use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};

use super::{LlmBackend, LlmConfig, LlmError, LlmOutput, LlmProvider, PromptRequest};

/// Free-prose temperature. 0.1, not the 0.4 the prose endpoints used to default to:
/// at 0.4 the activity count swung between 1 and 30 across runs on identical input.
const TEMP: f64 = 0.1;

pub struct LocalBackend {
    pub cfg: LlmConfig,
}

#[async_trait]
impl LlmBackend for LocalBackend {
    fn provider(&self) -> LlmProvider {
        LlmProvider::Local
    }

    async fn complete(&self, req: &PromptRequest) -> Result<LlmOutput, LlmError> {
        let t0 = std::time::Instant::now();

        // One model call in flight across the whole daemon. Held for this call only —
        // NOT for the surrounding pipeline stage, or a slow hour would starve the rest.
        let _permit = crate::llm_gate::acquire().await;

        let url = format!(
            "http://{}:{}/v1/chat/completions",
            self.cfg.mlx_host, self.cfg.mlx_port
        );

        let mut body = json!({
            "model": "default",
            "temperature": TEMP,
            "max_tokens": req.max_tokens,
            // Thinking OFF (see the module docs). Without this the free-form path leaks
            // the model's reasoning into the answer as plain untagged prose
            // ("Thinking Process: 1. Analyze the request…") — there is no </think> tag
            // for the server to strip, so it reaches the user's report verbatim.
            "enable_thinking": false,
            "messages": [
                {"role": "system", "content": req.system},
                {"role": "user",   "content": req.user},
            ],
        });
        // A json_schema response_format routes the server to its outlines FSM, which
        // constrains generation token by token — the model cannot produce invalid JSON.
        if let Some(schema) = &req.schema {
            body["response_format"] = json!({
                "type": "json_schema",
                "json_schema": {"name": "answer", "schema": schema},
            });
        }

        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(self.cfg.local_timeout_s))
            .build()
            .map_err(|e| LlmError::Failed(format!("MLX client: {e}")))?;

        let resp = client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| LlmError::Failed(format!("MLX server unreachable at {url}: {e}")))?;

        let status = resp.status();
        if !status.is_success() {
            let detail = resp.text().await.unwrap_or_default();
            let head: String = detail.chars().take(200).collect();
            return Err(LlmError::Failed(format!("MLX server {status}: {head}")));
        }

        let payload: Value = resp
            .json()
            .await
            .map_err(|e| LlmError::Failed(format!("MLX response not JSON: {e}")))?;

        let text = payload["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or_default()
            .trim()
            .to_string();
        if text.is_empty() {
            return Err(LlmError::Failed("MLX returned an empty answer".into()));
        }

        let usage = &payload["usage"];
        Ok(LlmOutput {
            text,
            input_tokens: usage["prompt_tokens"].as_u64().unwrap_or(0) as u32,
            output_tokens: usage["completion_tokens"].as_u64().unwrap_or(0) as u32,
            elapsed_s: t0.elapsed().as_secs_f64(),
        })
    }
}
