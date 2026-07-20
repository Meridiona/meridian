//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Cursor backend - `cursor-agent -p`, on the user's own Cursor subscription.
//!
//! This is the LLM-provider entry point: when the user selects Cursor, EVERY Meridian AI process
//! (coding-agent summaries, hourly distillation, worklog generation, ticket proposals, task
//! classification, and the Settings connection probe) resolves here.
//!
//! The safety flags, environment and sandbox are NOT defined here - they live in
//! [`crate::llm::cursor_cli`] so the summariser's call site cannot drift out of sync. What is
//! local to this module is the JSON envelope and its error classification.
//!
//! Unlike claude/codex there is **no schema mechanism**, so the JSON contract rides in the prompt
//! and the answer is parsed tolerantly. The payload goes in argv (stdin is unprobed on this CLI).
//!
//! # Related
//! - [`crate::llm::cursor_cli`] - shared flags/env/sandbox (read this first)
//! - [`crate::llm::claude`] - the equivalent backend whose hardening this mirrors

use async_trait::async_trait;
use serde_json::Value;

use crate::coding_agent_session_ingest::summariser::prompts as sp;
use crate::coding_agent_session_ingest::summariser::{cap_transcript, run_capture};

use super::cursor_cli;
use super::{prompts, LlmBackend, LlmConfig, LlmError, LlmOutput, LlmProvider, PromptRequest};

const ARG_CAP: usize = 180_000;

/// Pinned model for every Meridian AI process on Cursor: small, fast, cheap and ZDR-eligible
/// (Cursor flags non-ZDR models explicitly in `--list-models`; this is not one) - the right tier
/// for summarise/classify/draft, and deterministic across users, unlike the account default
/// `auto` which is server-routed and varies per call AND per account.
///
/// `-medium` is the effort suffix Cursor displays as plain "GPT-5.4 Mini"; the ladder is
/// `-none/-low/-medium/-high/-xhigh`. A given plan may not carry it, so [`FALLBACK_MODEL`] keeps
/// the call working rather than failing.
const DEFAULT_MODEL: &str = "gpt-5.4-mini-medium";

/// Cursor's server-routed default, used only when [`DEFAULT_MODEL`] is unavailable on the account.
const FALLBACK_MODEL: &str = "auto";

pub struct CursorBackend {
    pub cfg: LlmConfig,
}

/// How far the call has degraded. Each step is a response to a specific, detected failure - never
/// a blind retry - so a working install always takes the fully hardened path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Hardening {
    /// Everything on: no tools, skills-free sandbox HOME.
    Full,
    /// This CLI build rejected `--allowed-tools` (undocumented flag removed upstream).
    NoToolFlag,
    /// Auth could not be read from the sandbox HOME; fall back to the inherited one.
    RealHome,
}

impl CursorBackend {
    /// One `cursor-agent -p` call, returning the answer text or a classified error.
    #[tracing::instrument(skip(self, prompt), fields(model, hardening = ?hardening))]
    async fn run(
        &self,
        prompt: &str,
        model: &str,
        hardening: Hardening,
    ) -> Result<String, LlmError> {
        let mut args: Vec<String> = vec!["-p".into(), prompt.to_string()];
        args.extend(["--output-format".into(), "json".into()]);
        args.extend(cursor_cli::safety_args(
            model,
            hardening != Hardening::NoToolFlag,
        ));

        let sandbox = match hardening {
            Hardening::RealHome => None,
            _ => cursor_cli::sandbox_home(),
        };
        let env = cursor_cli::env_overrides(sandbox.as_deref());
        let env_refs: Vec<(&str, &str)> = env.iter().map(|(k, v)| (*k, v.as_str())).collect();

        let cap = run_capture(
            "cursor-agent",
            &args,
            "", // payload is in the prompt (argv), not stdin
            &self.cfg.meridian_home,
            self.cfg.cli_timeout_s,
            &env_refs,
            cursor_cli::ENV_REMOVE,
        )
        .await
        .map_err(super::resolver::from_summariser_error)?;

        // On a hard failure cursor-agent emits no well-formed JSON - the error is on stderr.
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
                "cursor-agent exited {:?}: {}",
                cap.code,
                sp::first_line(&cap.stderr)
            )));
        }

        // `--output-format json`: one {type:"result", subtype, is_error, result, usage} object.
        let payload: Value = serde_json::from_str(&cap.stdout).map_err(|e| {
            let head: String = cap.stdout.chars().take(200).collect();
            LlmError::Failed(format!("cursor-agent output not JSON ({e}): {head:?}"))
        })?;

        // Exit 0 does not mean success: the envelope can still report an error (a limit hit
        // mid-run, for instance), and that error is where the rate-limit signal lives.
        let is_error = payload
            .get("is_error")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let subtype = payload.get("subtype").and_then(Value::as_str);
        if is_error || !matches!(subtype, None | Some("success")) {
            let detail: String = payload
                .get("result")
                .and_then(Value::as_str)
                .or(subtype)
                .unwrap_or("error")
                .chars()
                .take(200)
                .collect();
            if sp::looks_rate_limited(&detail) {
                return Err(LlmError::rate_limited(detail));
            }
            return Err(LlmError::Failed(format!("cursor-agent reported: {detail}")));
        }

        let text = payload
            .get("result")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_string();
        if text.is_empty() {
            return Err(LlmError::Failed(
                "cursor-agent returned an empty answer".into(),
            ));
        }

        if let Some(usage) = payload.get("usage") {
            tracing::debug!(usage = %usage, "cursor: call usage");
        }
        Ok(text)
    }
}

#[async_trait]
impl LlmBackend for CursorBackend {
    fn provider(&self) -> LlmProvider {
        LlmProvider::Cursor
    }

    #[tracing::instrument(skip(self, req), fields(label = %req.label))]
    async fn complete(&self, req: &PromptRequest) -> Result<LlmOutput, LlmError> {
        let t0 = std::time::Instant::now();

        // Marker first: cursor-agent has no --no-session-persistence, so this call persists a
        // chat to ~/.cursor/chats. The ingest self-guard drops any session whose first prompt
        // carries MERIDIAN_PROMPT_MARKER, so our own calls are never re-ingested and
        // re-summarised - for every AI process routed through Cursor, not just the summariser.
        let mut prompt = format!(
            "{}\n{}\n\n{}",
            super::MERIDIAN_PROMPT_MARKER,
            req.system,
            cap_transcript(&req.user, ARG_CAP)
        );
        if let Some(schema) = &req.schema {
            prompt.push_str(&prompts::schema_instruction(schema));
        }

        // Empty override = the pinned default (deterministic, ZDR). A non-empty override is the
        // user's explicit choice and is passed through as-is.
        let model = if self.cfg.model.is_empty() {
            DEFAULT_MODEL
        } else {
            self.cfg.model.as_str()
        };

        // Degrade one step at a time on a DETECTED cause, never blindly: drop the undocumented
        // tool flag if this build rejects it, fall back to the real HOME if the sandbox hid the
        // credentials, then to the server-routed model if the pinned one is not on this plan.
        // A rate limit deliberately does NOT retry - Cursor limits are account-level, so a second
        // call would burn quota to hit the same wall; the work stays pending for the next cycle.
        let text = match self.run(&prompt, model, Hardening::Full).await {
            Ok(t) => t,
            Err(LlmError::Failed(msg)) if cursor_cli::looks_unsupported_flag(&msg) => {
                tracing::warn!(
                    "cursor: this CLI build rejects --allowed-tools - retrying without it \
                     (call still works, just carries the full tool context)"
                );
                self.run(&prompt, model, Hardening::NoToolFlag).await?
            }
            Err(LlmError::Failed(msg)) if cursor_cli::looks_auth_failure(&msg) => {
                tracing::warn!(
                    "cursor: credentials not visible from the sandbox HOME - retrying on the \
                     real HOME (call still works, just carries the user's skill list)"
                );
                self.run(&prompt, model, Hardening::RealHome).await?
            }
            Err(LlmError::Failed(msg)) if model != FALLBACK_MODEL && looks_unknown_model(&msg) => {
                tracing::warn!(
                    model,
                    fallback = FALLBACK_MODEL,
                    "cursor: pinned model unavailable on this account - retrying with auto"
                );
                self.run(&prompt, FALLBACK_MODEL, Hardening::Full).await?
            }
            Err(e) => return Err(e),
        };

        let elapsed_s = t0.elapsed().as_secs_f64();
        tracing::info!(
            model,
            elapsed_s,
            chars = text.len(),
            "cursor: call complete"
        );
        Ok(LlmOutput {
            text,
            // The CLI reports usage in its envelope but not a comparable prompt/completion split;
            // logged on the call span instead of guessed at here.
            input_tokens: 0,
            output_tokens: 0,
            elapsed_s,
        })
    }
}

/// Heuristic: did the CLI reject the pinned model as unavailable on this account? Cursor plans
/// differ in model access, so a pinned id can be absent - detecting that lets
/// [`CursorBackend::complete`] fall back to `auto` rather than failing every call.
fn looks_unknown_model(msg: &str) -> bool {
    let m = msg.to_lowercase();
    m.contains("model")
        && (m.contains("unknown")
            || m.contains("not available")
            || m.contains("unavailable")
            || m.contains("invalid")
            || m.contains("not found")
            || m.contains("does not exist")
            || m.contains("no access"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_model_detection_matches_availability_errors() {
        assert!(looks_unknown_model("Unknown model: gpt-5.4-mini-medium"));
        assert!(looks_unknown_model(
            "cursor-agent reported: model not available on your plan"
        ));
        assert!(looks_unknown_model("Invalid model id"));
        // Not a model-availability problem - must NOT trigger a silent model swap.
        assert!(!looks_unknown_model("rate limit reached"));
        assert!(!looks_unknown_model("network error"));
        assert!(!looks_unknown_model("workspace trust required"));
    }

    #[test]
    fn marker_leads_the_prompt_so_ingest_can_drop_our_own_sessions() {
        // The self-ingest guard checks the FIRST user prompt for the marker; the backend must
        // put it there. Guards against a refactor that reorders the format!.
        let prompt = format!(
            "{}\n{}\n\n{}",
            super::super::MERIDIAN_PROMPT_MARKER,
            "sys",
            "user"
        );
        assert!(prompt.starts_with(super::super::MERIDIAN_PROMPT_MARKER));
    }
}
