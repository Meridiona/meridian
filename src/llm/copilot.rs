//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! GitHub Copilot backend — `copilot -p`, on the user's own Copilot subscription.
//!
//! Two constraints this backend has and the others don't:
//!
//! 1. **stdin is ignored.** `copilot -p` does not forward stdin to the model — feeding it
//!    there produced the model literally replying "Waiting for transcript input on
//!    stdin…". The whole payload therefore goes in argv, capped at [`ARG_CAP`] so we stay
//!    clear of the OS argument-length limit.
//! 2. **No schema flag exists.** Structured output cannot be enforced, so the JSON
//!    contract is appended to the prompt and the answer is parsed tolerantly
//!    ([`super::parse_json_object`]). A schema here is a request, not a guarantee — the
//!    caller must cope with an unparseable answer, and does.

use async_trait::async_trait;

use crate::coding_agent_session_ingest::summariser::prompts as sp;
use crate::coding_agent_session_ingest::summariser::{cap_transcript, run_capture};

use super::{prompts, LlmBackend, LlmConfig, LlmError, LlmOutput, LlmProvider, PromptRequest};

/// Payload cap for the argv-embedded prompt. The OS caps a single argument
/// (`MAX_ARG_STRLEN`); overshooting is an opaque spawn failure, so cap it ourselves and
/// keep the head and tail (`cap_transcript` elides the middle).
const ARG_CAP: usize = 180_000;

pub struct CopilotBackend {
    pub cfg: LlmConfig,
}

#[async_trait]
impl LlmBackend for CopilotBackend {
    fn provider(&self) -> LlmProvider {
        LlmProvider::Copilot
    }

    async fn complete(&self, req: &PromptRequest) -> Result<LlmOutput, LlmError> {
        let t0 = std::time::Instant::now();

        let mut prompt = format!("{}\n\n{}", req.system, cap_transcript(&req.user, ARG_CAP));
        if let Some(schema) = &req.schema {
            prompt.push_str(&prompts::schema_instruction(schema));
        }

        let args: Vec<String> = vec![
            "-p".into(),
            prompt,
            "-s".into(), // response only — no stats banner wrapped around the answer
            "--no-color".into(),
            "--log-level".into(),
            "none".into(),
            "--no-custom-instructions".into(),
        ];

        let cap = run_capture(
            "copilot",
            &args,
            "", // stdin is ignored by `copilot -p` — see the module docs
            &self.cfg.meridian_home,
            self.cfg.cli_timeout_s,
            // `DO_NOT_TRACK` is Copilot CLI's own documented telemetry opt-out. Training on
            // your snippets is the GitHub account's Copilot policy setting, not a CLI flag.
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
                return Err(LlmError::rate_limited(if msg.is_empty() {
                    "rate/usage limit".into()
                } else {
                    msg
                }));
            }
            return Err(LlmError::Failed(format!(
                "copilot exited {:?}: {}",
                cap.code,
                sp::first_line(&cap.stderr)
            )));
        }

        let text = cap.stdout.trim().to_string();
        if text.is_empty() {
            return Err(LlmError::Failed("copilot returned an empty answer".into()));
        }

        Ok(LlmOutput {
            text,
            input_tokens: 0,
            output_tokens: 0,
            elapsed_s: t0.elapsed().as_secs_f64(),
        })
    }
}
