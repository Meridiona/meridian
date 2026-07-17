//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Cursor backend — `cursor-agent -p`, on the user's own Cursor subscription.
//!
//! Like copilot, and unlike claude/codex: **no schema mechanism**, so the JSON contract
//! rides in the prompt and the answer is parsed tolerantly. The payload goes in argv
//! (stdin support is unprobed on this CLI; argv is known to work).
//!
//! `--trust` is required: cursor-agent refuses an untrusted workspace even in print mode
//! ("Workspace Trust Required", exit 1). Trusting is safe here — the prompt is our own
//! text and the agent is not being asked to execute anything.

use async_trait::async_trait;

use crate::coding_agent_session_ingest::summariser::prompts as sp;
use crate::coding_agent_session_ingest::summariser::{cap_transcript, run_capture};

use super::{prompts, LlmBackend, LlmConfig, LlmError, LlmOutput, LlmProvider, PromptRequest};

const ARG_CAP: usize = 180_000;

pub struct CursorBackend {
    pub cfg: LlmConfig,
}

#[async_trait]
impl LlmBackend for CursorBackend {
    fn provider(&self) -> LlmProvider {
        LlmProvider::Cursor
    }

    async fn complete(&self, req: &PromptRequest) -> Result<LlmOutput, LlmError> {
        let t0 = std::time::Instant::now();

        let mut prompt = format!("{}\n\n{}", req.system, cap_transcript(&req.user, ARG_CAP));
        if let Some(schema) = &req.schema {
            prompt.push_str(&prompts::schema_instruction(schema));
        }

        let mut args: Vec<String> = vec![
            "-p".into(),
            prompt,
            "--output-format".into(),
            "text".into(),
            "--trust".into(),
        ];
        if !self.cfg.model.is_empty() {
            args.push("--model".into());
            args.push(self.cfg.model.clone());
        }

        let cap = run_capture(
            "cursor-agent",
            &args,
            "", // payload is in the prompt
            &self.cfg.meridian_home,
            self.cfg.cli_timeout_s,
            // Cursor exposes no per-call telemetry flag — the no-training guarantee is its
            // account-level Privacy Mode (it sends a ZDR flag upstream). Set DO_NOT_TRACK as
            // the best-effort standard opt-out we can control; the UI points users at
            // Privacy Mode for the real training/retention guarantee.
            &[("MERIDIAN_SUMMARISER", "1"), super::DO_NOT_TRACK],
            &[],
        )
        .await
        .map_err(super::resolver::from_summariser_error)?;

        if !cap.success {
            let blob = format!("{}\n{}", cap.stderr, cap.stdout);
            if sp::looks_rate_limited(&blob) {
                let msg =
                    sp::rate_limited_line(&blob).unwrap_or_else(|| sp::first_line(&cap.stderr));
                return Err(LlmError::RateLimited(if msg.is_empty() {
                    "rate/usage limit".into()
                } else {
                    msg
                }));
            }
            return Err(LlmError::Failed(format!(
                "cursor-agent exited {:?}: {}",
                cap.code,
                sp::first_line(&cap.stderr)
            )));
        }

        let text = cap.stdout.trim().to_string();
        if text.is_empty() {
            return Err(LlmError::Failed(
                "cursor-agent returned an empty answer".into(),
            ));
        }

        Ok(LlmOutput {
            text,
            input_tokens: 0,
            output_tokens: 0,
            elapsed_s: t0.elapsed().as_secs_f64(),
        })
    }
}
