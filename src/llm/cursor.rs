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
//! and the answer is parsed tolerantly. The payload goes over **stdin**, not argv.
//!
//! # Why stdin, not argv (Windows)
//!
//! It used to be argv (`-p <prompt>`). `cursor-agent` resolves to `cursor-agent.cmd` on
//! Windows, a batch file rather than a native exe, and Rust's std library REFUSES to spawn a
//! `.bat`/`.cmd` target when an argument contains characters it cannot safely escape (the
//! CVE-2024-24576 "BatBadBut" fix), notably embedded newlines. [`build_prompt`] always
//! produces a multi-line string (marker + system prompt + blank line + transcript), so EVERY
//! real call failed to even spawn on Windows, with `io::Error { kind: InvalidInput, "batch
//! file arguments are invalid" }`, confirmed live. `cursor-agent -p` (bare flag, no positional
//! value) reads the full prompt from stdin instead, also confirmed live, and stdin has no such
//! restriction on any platform (it is a byte stream, not a command-line argument), so this
//! fixes Windows without changing behaviour anywhere else.
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

/// Above this, a call logs a WARN (not just the routine INFO summary) so a latency
/// regression is visible in OpenObserve on a packaged install, not just in a local spool
/// nobody is tailing. Comfortably above the ~6s a warm, hardened call took when measured
/// live, and below the point where a user watching "Draft with AI" would call it stuck.
const SLOW_CALL_WARN_THRESHOLD_S: f64 = 30.0;

/// How long `cursor-agent status` may take — a local credential read; measured ~2.3s live
/// (slower than Codex's/Claude's equivalent, still comfortably sub-second-budget territory).
/// Generous ceiling, not a real budget.
const LOGIN_STATUS_TIMEOUT_S: u64 = 15;

/// Ternary read of `cursor-agent status --format json`'s `isAuthenticated` field.
/// `Some(true)`/`Some(false)` are authoritative; `None` means the check itself failed (spawn
/// error, timeout, malformed output) and must not be trusted either way — see
/// [`super::detect::interactive_login`], the sole caller: it uses this to confirm an
/// interactive sign-in actually took even when the spawned `cursor-agent login` process
/// itself timed out or exited non-zero.
///
/// **The exit code is NOT authoritative here** — measured live: `cursor-agent status` can
/// exit 1 ("unable to fetch user details") while its own JSON body reports
/// `isAuthenticated: true`. The body is the only trustworthy signal, which is why this reads
/// it regardless of `cap.success`.
pub(crate) async fn login_status_signed_in(meridian_home: &std::path::Path) -> Option<bool> {
    let cap = run_capture(
        "cursor-agent",
        &["status".into(), "--format".into(), "json".into()],
        "",
        meridian_home,
        LOGIN_STATUS_TIMEOUT_S,
        &[],
        cursor_cli::ENV_REMOVE,
    )
    .await
    .ok()?;
    parse_is_authenticated(&cap.stdout)
}

/// The pure parse [`login_status_signed_in`] applies to the captured stdout — split out so
/// it is unit-testable without spawning a process.
fn parse_is_authenticated(stdout: &str) -> Option<bool> {
    let v: Value = serde_json::from_str(stdout.trim()).ok()?;
    v.get("isAuthenticated").and_then(Value::as_bool)
}

pub struct CursorBackend {
    pub cfg: LlmConfig,
}

/// Lets [`cursor_cli::run_hardened`] classify our failures: a `Failed` may be a degradable
/// cause, a `RateLimited` never is (account-level, so a retry spends quota to hit the same
/// wall).
impl cursor_cli::CursorCallError for LlmError {
    fn degradable_message(&self) -> Option<&str> {
        match self {
            LlmError::Failed(m) => Some(m),
            LlmError::RateLimited { .. } => None,
        }
    }
}

impl CursorBackend {
    /// One `cursor-agent -p` attempt with the safety argv and environment handed down by
    /// [`cursor_cli::run_hardened`], returning the answer text or a classified error.
    #[tracing::instrument(skip(self, prompt, safety, env))]
    async fn attempt(
        &self,
        prompt: &str,
        safety: Vec<String>,
        env: Vec<(&'static str, String)>,
    ) -> Result<String, LlmError> {
        // `-p` is a bare flag here - no positional value. See the module doc: a positional
        // value forces the prompt through argv, which Windows' batch-file spawn guard rejects
        // outright whenever it contains a newline (every real prompt does).
        let mut args: Vec<String> = vec!["-p".into()];
        args.extend(["--output-format".into(), "json".into()]);
        args.extend(safety);

        let env_refs: Vec<(&str, &str)> = env.iter().map(|(k, v)| (*k, v.as_str())).collect();

        let cap = run_capture(
            "cursor-agent",
            &args,
            prompt, // payload goes over stdin - see the module doc
            // The neutral workspace, NOT ~/.meridian: `--workspace` governs rule discovery
            // today, but the cwd is inside $HOME, so if Cursor ever also seeds discovery from
            // it the no-rule-injection guarantee would regress silently. Free to pin.
            &cursor_cli::neutral_workspace(),
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
        let prompt = build_prompt(req);

        let model = resolve_model(&self.cfg.model, req.interactive);

        // The degradation ladder lives in `cursor_cli` so the summariser inherits it too.
        let text =
            cursor_cli::run_hardened(model, |safety, env| self.attempt(&prompt, safety, env))
                .await?;

        let elapsed_s = t0.elapsed().as_secs_f64();
        tracing::info!(
            model,
            elapsed_s,
            chars = text.len(),
            "cursor: call complete"
        );
        // WARN, not INFO: this is the one signal that ships on a packaged install (only
        // WARN+ egresses - see CLAUDE.md's Observability section), and a Cursor call that
        // blows past a normal budget is exactly the kind of regression that must be visible
        // without an engineer already looking at this one machine's local spool.
        if elapsed_s > SLOW_CALL_WARN_THRESHOLD_S {
            tracing::warn!(
                model,
                elapsed_s,
                interactive = req.interactive,
                "cursor: call took longer than expected"
            );
        }
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

/// Which model tier this call uses. An explicit user override (`cfg_model` non-empty) always
/// wins, matching every other provider's "the user's choice is never silently swapped" rule -
/// [`PromptRequest::interactive`] only ever picks between OUR two pinned tiers, never
/// overrides what the user chose. Extracted so the choice is testable without spawning a
/// process — see [`CursorBackend::complete`].
fn resolve_model(cfg_model: &str, interactive: bool) -> &str {
    if !cfg_model.is_empty() {
        cfg_model
    } else if interactive {
        cursor_cli::FAST_MODEL
    } else {
        cursor_cli::DEFAULT_MODEL
    }
}

/// The exact text sent to `cursor-agent -p`.
///
/// Extracted from [`CursorBackend::complete`] so a test can assert on the prompt the product
/// actually builds. Inlining it made the marker assertion a tautology - the test rebuilt the
/// `format!` itself, so deleting the marker from the real call site still passed.
///
/// The marker leads: cursor-agent has no `--no-session-persistence`, so this call persists a
/// chat to `~/.cursor/chats`. The ingest self-guard drops any session carrying
/// [`super::MERIDIAN_PROMPT_MARKER`], so our own calls are never re-ingested and
/// re-summarised - for every AI process routed through Cursor, not just the summariser.
fn build_prompt(req: &PromptRequest) -> String {
    let mut prompt = format!(
        "{}\n{}\n\n{}",
        super::MERIDIAN_PROMPT_MARKER,
        req.system,
        cap_transcript(&req.user, ARG_CAP)
    );
    if let Some(schema) = &req.schema {
        prompt.push_str(&prompts::schema_instruction(schema));
    }
    prompt
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req() -> PromptRequest {
        PromptRequest::new("SYSTEM PROMPT", "USER INPUT", "test-label")
    }

    #[test]
    fn a_background_call_with_no_override_uses_the_default_tier() {
        assert_eq!(resolve_model("", false), cursor_cli::DEFAULT_MODEL);
    }

    #[test]
    fn an_interactive_call_with_no_override_uses_the_fast_tier() {
        assert_eq!(resolve_model("", true), cursor_cli::FAST_MODEL);
    }

    /// THE property that matters: a user's explicit model choice is never silently swapped
    /// for a faster one just because the call happens to be interactive.
    #[test]
    fn an_explicit_override_always_wins_over_the_interactive_hint() {
        assert_eq!(resolve_model("claude-opus-4-8", true), "claude-opus-4-8");
        assert_eq!(resolve_model("claude-opus-4-8", false), "claude-opus-4-8");
    }

    /// THE regression this parse exists for: verbatim shape captured live from
    /// `cursor-agent status --format json` while genuinely authenticated - the exit code was
    /// 1 ("unable to fetch user details") but the body says `isAuthenticated: true`. A caller
    /// that gated on exit code instead of this field would misreport a working sign-in as
    /// failed.
    #[test]
    fn parse_is_authenticated_reads_the_real_authenticated_response_despite_a_nonzero_exit() {
        let body = r#"{"status":"authenticated","isAuthenticated":true,"hasAccessToken":true,
            "hasRefreshToken":true,"message":"Logged in (unable to fetch user details)"}"#;
        assert_eq!(parse_is_authenticated(body), Some(true));
    }

    #[test]
    fn parse_is_authenticated_reads_a_signed_out_response() {
        assert_eq!(
            parse_is_authenticated(r#"{"isAuthenticated":false}"#),
            Some(false)
        );
    }

    /// Malformed/empty output (a crash, an incompatible future CLI version) must be
    /// inconclusive, never misread as either extreme.
    #[test]
    fn parse_is_authenticated_is_none_on_malformed_output() {
        assert_eq!(parse_is_authenticated(""), None);
        assert_eq!(parse_is_authenticated("not json"), None);
        assert_eq!(parse_is_authenticated("{}"), None);
    }

    /// The marker is what cuts the summarise -> ingest -> summarise loop, so the property to
    /// pin is its PRESENCE in the prompt `complete()` really sends. Asserted against
    /// [`build_prompt`] rather than a string rebuilt here, so deleting or moving the marker at
    /// the call site fails this test.
    #[test]
    fn the_real_prompt_carries_the_self_ingest_marker() {
        let prompt = build_prompt(&req());
        assert!(
            prompt.contains(super::super::MERIDIAN_PROMPT_MARKER),
            "without the marker every Meridian cursor call is re-ingested as developer activity"
        );
        // The guard uses `contains`, so position is not the contract - but the system prompt
        // and the user input must both actually make it through.
        assert!(prompt.contains("SYSTEM PROMPT"));
        assert!(prompt.contains("USER INPUT"));
    }

    #[test]
    fn a_schema_request_appends_the_json_contract() {
        // Cursor has no --json-schema, so the contract can only ride in the prompt; losing
        // this silently degrades every structured caller to unparseable prose.
        let plain = build_prompt(&req());
        let with_schema = build_prompt(&req().with_schema(serde_json::json!({"type": "object"})));
        assert!(with_schema.len() > plain.len());
        assert!(with_schema.starts_with(&plain));
    }
}
