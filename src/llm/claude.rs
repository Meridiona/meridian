//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Claude Code backend — `claude -p`, on the user's own subscription.
//!
//! Structured output is real here: `--json-schema` is server-side validated, so a
//! schema request is honoured rather than merely suggested.
//!
//! Auth is the user's Claude login. `ANTHROPIC_API_KEY` is stripped from the child env
//! so a stray key can't silently switch them to metered API billing, and
//! `MERIDIAN_SUMMARISER=1` makes the coding-agent indexer ignore the throwaway session
//! this spawns (otherwise we would ingest our own prompt and summarise it next hour).
//!
//! NOTE: the inherited env must carry HOME/PATH/USER/LOGNAME or the login keychain
//! cannot unlock — the daemon's launchd plist owns that.

use async_trait::async_trait;
use serde_json::Value;

use crate::coding_agent_session_ingest::summariser::prompts as sp;
use crate::coding_agent_session_ingest::summariser::run_capture;

use super::{LlmBackend, LlmConfig, LlmError, LlmOutput, LlmProvider, PromptRequest};

pub struct ClaudeBackend {
    pub cfg: LlmConfig,
}

#[async_trait]
impl LlmBackend for ClaudeBackend {
    fn provider(&self) -> LlmProvider {
        LlmProvider::Claude
    }

    async fn complete(&self, req: &PromptRequest) -> Result<LlmOutput, LlmError> {
        let t0 = std::time::Instant::now();

        let mut args: Vec<String> = vec![
            "-p".into(),
            req.system.to_string(),
            "--output-format".into(),
            "json".into(),
            "--no-session-persistence".into(),
            "--strict-mcp-config".into(), // drop MCP overhead
            // Disable every built-in tool. This call is pure text summarisation, so the
            // default tool set (Bash/Read/Edit/Glob/…) is dead weight — and it otherwise
            // ships each tool's schema in the request context. `num_turns` is always 1.
            // Empty allowlist = no tools.
            "--allowedTools".into(),
            String::new(),
            // Load NO settings sources (user/project/local). Without this, Claude Code
            // injects the user's `~/CLAUDE.md` (+ project/local settings) into every
            // request's system prompt — irrelevant coding-assistant instructions for a
            // pure summarisation call. Empty = load none, so the request carries only our
            // own prompt + the hour's data.
            "--setting-sources".into(),
            String::new(),
        ];
        if let Some(schema) = &req.schema {
            args.push("--json-schema".into());
            args.push(schema.to_string());
        }
        if !self.cfg.model.is_empty() {
            args.push("--model".into());
            args.push(self.cfg.model.clone());
        }

        let cap = run_capture(
            "claude",
            &args,
            &req.user, // the hour's input goes on stdin
            &self.cfg.meridian_home,
            self.cfg.cli_timeout_s,
            &[
                ("MERIDIAN_SUMMARISER", "1"),
                // Privacy: the umbrella switch that turns off telemetry, error reporting,
                // and every other non-essential Anthropic egress in one flag. (Training on
                // Claude Code usage is an account-level policy, not a per-call flag — see
                // super::DO_NOT_TRACK.)
                ("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1"),
                super::DO_NOT_TRACK,
            ],
            &["ANTHROPIC_API_KEY"],
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
            let detail = {
                let s = sp::first_line(&cap.stderr);
                if s.is_empty() {
                    sp::first_line(&cap.stdout)
                } else {
                    s
                }
            };
            return Err(LlmError::Failed(format!(
                "claude exited {:?}: {detail}",
                cap.code
            )));
        }

        let payload: Value = serde_json::from_str(&cap.stdout).map_err(|e| {
            let head: String = cap.stdout.chars().take(200).collect();
            LlmError::Failed(format!("claude output not JSON ({e}): {head:?}"))
        })?;

        // Exit 0 does not mean success: the envelope can still report an error (a limit
        // hit mid-run, for instance), and that error is where the rate-limit signal lives.
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
                return Err(LlmError::RateLimited(detail));
            }
            return Err(LlmError::Failed(format!("claude reported: {detail}")));
        }

        // With a schema, the validated object is in `structured_output`; without one the
        // answer is the plain `result` string.
        let text = if req.schema.is_some() {
            payload
                .get("structured_output")
                .map(|v| v.to_string())
                .ok_or_else(|| {
                    LlmError::Failed("claude returned no structured_output".to_string())
                })?
        } else {
            payload
                .get("result")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        };

        Ok(LlmOutput {
            text,
            input_tokens: 0, // the CLI does not report token counts
            output_tokens: 0,
            elapsed_s: t0.elapsed().as_secs_f64(),
        })
    }
}
