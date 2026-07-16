//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Codex backend — `codex exec`, on the user's own ChatGPT subscription.
//!
//! Structured output is real: `--output-schema` is validated, and the final message is
//! written to a file (`-o`) rather than stdout, so we read it from there. Both the schema
//! and the output file live in a temp dir removed on drop, even on a panic or timeout.
//!
//! `-s read-only` + `--skip-git-repo-check` + `--ephemeral`: this is a summarisation call,
//! not an agent session. It must not touch the filesystem or leave a rollout behind.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;

use crate::coding_agent_session_ingest::summariser::prompts as sp;
use crate::coding_agent_session_ingest::summariser::run_capture;

use super::{LlmBackend, LlmConfig, LlmError, LlmOutput, LlmProvider, PromptRequest};

static SEQ: AtomicU64 = AtomicU64::new(0);

/// Removes the temp dir on drop — including when the call times out or panics.
struct TempDirGuard(PathBuf);
impl Drop for TempDirGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub struct CodexBackend {
    pub cfg: LlmConfig,
}

#[async_trait]
impl LlmBackend for CodexBackend {
    fn provider(&self) -> LlmProvider {
        LlmProvider::Codex
    }

    async fn complete(&self, req: &PromptRequest) -> Result<LlmOutput, LlmError> {
        let t0 = std::time::Instant::now();

        let td = std::env::temp_dir().join(format!(
            "meridian-llm-codex-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&td)
            .map_err(|e| LlmError::Failed(format!("codex: temp dir: {e}")))?;
        let _guard = TempDirGuard(td.clone());

        let out_path = td.join("last_message.txt");
        let mut args: Vec<String> = vec![
            "exec".into(),
            req.system.to_string(),
            "-s".into(),
            "read-only".into(),
            "--skip-git-repo-check".into(),
            "--ephemeral".into(),
            // Privacy: disable Codex's usage/analytics collection for this call. This is
            // telemetry only — opting your prompts out of model *training* is the ChatGPT
            // account's "Improve the model for everyone" setting (Data Controls), which a
            // subprocess cannot toggle. See super::DO_NOT_TRACK.
            "--config".into(),
            "analytics.enabled=false".into(),
            "-o".into(),
            out_path.display().to_string(),
            "-C".into(),
            self.cfg.meridian_home.display().to_string(),
        ];
        if let Some(schema) = &req.schema {
            let schema_path = td.join("schema.json");
            std::fs::write(&schema_path, schema.to_string())
                .map_err(|e| LlmError::Failed(format!("codex: write schema: {e}")))?;
            args.push("--output-schema".into());
            args.push(schema_path.display().to_string());
        }
        if !self.cfg.model.is_empty() {
            args.push("-m".into());
            args.push(self.cfg.model.clone());
        }

        let cap = run_capture(
            "codex",
            &args,
            &req.user, // codex exec reads the input from stdin
            &self.cfg.meridian_home,
            self.cfg.cli_timeout_s,
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
                "codex exited {:?}: {}",
                cap.code,
                sp::first_line(&cap.stderr)
            )));
        }

        let text = std::fs::read_to_string(&out_path)
            .map_err(|e| LlmError::Failed(format!("codex: no output file ({e})")))?;
        if text.trim().is_empty() {
            return Err(LlmError::Failed("codex returned an empty answer".into()));
        }

        Ok(LlmOutput {
            text: text.trim().to_string(),
            input_tokens: 0,
            output_tokens: 0,
            elapsed_s: t0.elapsed().as_secs_f64(),
        })
    }
}
