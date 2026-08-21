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
//!
//! # The whole prompt goes over stdin, not argv — two separate bugs, one fix
//!
//! It used to be `claude -p <req.system>` (positional argv) + `req.user` piped over stdin
//! SEPARATELY, at the same time. Two independent things were wrong with that:
//!
//! 1. **`req.user` never reached the model, on any platform.** Confirmed live: `claude -p
//!    "<prompt>"` with data ALSO piped on stdin silently ignores the piped data entirely —
//!    stdin is only consulted as a FALLBACK when no positional prompt is given, never as a
//!    second input alongside one. Every real Claude summarisation call has been running on
//!    instructions alone, with the actual hour's data discarded before it ever reached Claude.
//! 2. **Windows spawn failure.** `req.system` is a Markdown-sourced instruction prompt, so
//!    it is always multi-line for a real call. On THIS machine `claude` happens to resolve to
//!    a native `.exe` (unaffected), but an npm-installed Claude Code (`npm i -g
//!    @anthropic-ai/claude-code` — the exact command
//!    [`meridian_core::LlmProvider::install_command`] runs for this provider) resolves to
//!    `claude.cmd` on Windows — an npm-generated batch file — and Rust's std library REFUSES
//!    to spawn a `.bat`/`.cmd` target when an argument contains characters it cannot safely
//!    escape (the CVE-2024-24576 "BatBadBut" fix), notably embedded newlines: the same
//!    `io::Error { kind: InvalidInput, "batch file arguments are invalid" }` confirmed live
//!    for [`crate::llm::cursor`] and [`crate::llm::codex`] would hit any Windows user who
//!    installed this way.
//!
//! `claude -p` (bare flag, no positional value) reads the whole prompt from stdin instead —
//! confirmed live — with no restriction on content or platform, fixing both at once:
//! [`claude_stdin`] combines `req.system` and `req.user` into the one blob the model actually
//! sees now.

use async_trait::async_trait;
use serde_json::Value;

use crate::coding_agent_session_ingest::summariser::prompts as sp;
use crate::coding_agent_session_ingest::summariser::run_capture;

use super::{LlmBackend, LlmConfig, LlmError, LlmOutput, LlmProvider, PromptRequest};

/// How long `claude auth status --json` may take — a local credential read, no network round
/// trip; measured ~0.2s live. Generous ceiling, not a real budget.
const LOGIN_STATUS_TIMEOUT_S: u64 = 8;

/// Ternary read of `claude auth status --json`'s `loggedIn` field. `Some(true)`/`Some(false)`
/// are authoritative; `None` means the check itself failed (spawn error, timeout, malformed
/// output) and must not be trusted either way — see [`super::detect::interactive_login`],
/// the sole caller: it uses this to confirm an interactive sign-in actually took even when
/// the spawned `claude auth login` process itself timed out or exited non-zero.
///
/// The JSON body also carries the account email/org (`email`, `orgName`) — callers must log
/// only the boolean this returns, never the raw response.
pub(crate) async fn login_status_signed_in(meridian_home: &std::path::Path) -> Option<bool> {
    let cap = run_capture(
        "claude",
        &["auth".into(), "status".into(), "--json".into()],
        "",
        meridian_home,
        LOGIN_STATUS_TIMEOUT_S,
        &[],
        &[],
    )
    .await
    .ok()?;
    parse_logged_in(&cap.stdout)
}

/// The pure parse [`login_status_signed_in`] applies to the captured stdout — split out so
/// it is unit-testable without spawning a process.
fn parse_logged_in(stdout: &str) -> Option<bool> {
    let v: Value = serde_json::from_str(stdout.trim()).ok()?;
    v.get("loggedIn").and_then(Value::as_bool)
}

/// The full prompt sent over stdin — see the module doc. `req.user`, when present, follows
/// `req.system` after a blank line (the same shape [`crate::llm::cursor`]'s `build_prompt`
/// already uses for its combined single-channel prompt).
fn claude_stdin(req: &PromptRequest) -> String {
    if req.user.is_empty() {
        req.system.to_string()
    } else {
        format!("{}\n\n{}", req.system, req.user)
    }
}

/// The concise, human-readable message out of claude's JSON result envelope
/// (`{"is_error": bool, "subtype": "...", "result": "..."}`), when the envelope actually
/// reports an error. `None` when it doesn't look like an error envelope at all — a payload
/// with `is_error: false` and `subtype` absent/`"success"` — so callers know to treat the
/// call as having succeeded rather than manufacturing a failure out of a normal result.
///
/// Used for BOTH exit codes: claude reports plenty of failures (not signed in, workspace
/// trust, a limit hit mid-run) with a full envelope on stdout and a non-zero exit, not just
/// a bare line on stderr — see the module doc.
fn claude_envelope_detail(payload: &Value) -> Option<String> {
    let is_error = payload
        .get("is_error")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let subtype = payload.get("subtype").and_then(Value::as_str);
    if !is_error && matches!(subtype, None | Some("success")) {
        return None;
    }
    Some(
        payload
            .get("result")
            .and_then(Value::as_str)
            .or(subtype)
            .unwrap_or("error")
            .chars()
            .take(200)
            .collect(),
    )
}

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

        // `-p` is a bare flag here - no positional value. See the module doc: a positional
        // value forces the prompt through argv, which Windows' batch-file spawn guard rejects
        // outright whenever it contains a newline (every real prompt does).
        let mut args: Vec<String> = vec![
            "-p".into(),
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

        let stdin_text = claude_stdin(req);
        let cap = run_capture(
            "claude",
            &args,
            &stdin_text,
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

        // Parsed regardless of exit code: `claude -p --output-format json` still emits a
        // full `{is_error, subtype, result, ...}` envelope on many failures (not signed in,
        // workspace trust, a limit hit mid-run) — not just on success. Ignoring that on a
        // non-zero exit is what used to dump the WHOLE raw envelope as the error message
        // (`claude exited Some(1): {"type":"result",...400 more characters...}`) instead of
        // the CLI's own already-clean one-liner (`"result":"Not logged in · Please run
        // /login"`) — confirmed live: a signed-out Claude showed exactly that raw dump.
        let envelope: Option<Value> = serde_json::from_str(&cap.stdout).ok();

        if !cap.success {
            if let Some(detail) = envelope.as_ref().and_then(claude_envelope_detail) {
                if sp::looks_rate_limited(&detail) {
                    return Err(LlmError::rate_limited(detail));
                }
                return Err(LlmError::Failed(format!("claude reported: {detail}")));
            }
            // No parseable envelope at all (a crash before the CLI could emit one) - fall
            // back to whatever text it managed to print.
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

        let payload = envelope.ok_or_else(|| {
            let head: String = cap.stdout.chars().take(200).collect();
            LlmError::Failed(format!("claude output not JSON: {head:?}"))
        })?;

        // Exit 0 does not mean success: the envelope can still report an error (a limit
        // hit mid-run, for instance), and that error is where the rate-limit signal lives.
        if let Some(detail) = claude_envelope_detail(&payload) {
            if sp::looks_rate_limited(&detail) {
                return Err(LlmError::rate_limited(detail));
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Verbatim shape of a live `claude auth status --json` response (signed in).
    #[test]
    fn parse_logged_in_reads_a_real_signed_in_response() {
        let body = r#"{"loggedIn":true,"authMethod":"claude.ai","apiProvider":"firstParty",
            "email":"user@example.com","orgId":"x","orgName":"x's Organization","subscriptionType":"max"}"#;
        assert_eq!(parse_logged_in(body), Some(true));
    }

    #[test]
    fn parse_logged_in_reads_a_signed_out_response() {
        assert_eq!(parse_logged_in(r#"{"loggedIn":false}"#), Some(false));
    }

    /// Malformed/empty output (a crash, an incompatible future CLI version) must be
    /// inconclusive, never misread as either extreme.
    #[test]
    fn parse_logged_in_is_none_on_malformed_output() {
        assert_eq!(parse_logged_in(""), None);
        assert_eq!(parse_logged_in("not json"), None);
        assert_eq!(parse_logged_in("{}"), None);
    }

    /// The regression this exists for: `req.user` used to be piped over stdin ALONGSIDE a
    /// positional `-p <req.system>` argv value — but `claude -p` silently discards piped
    /// stdin whenever a positional prompt is ALSO given (confirmed live), so the hour's data
    /// never reached the model on any platform. Pins that the combined stdin text carries
    /// both.
    #[test]
    fn claude_stdin_carries_both_the_system_prompt_and_the_user_data() {
        let req = PromptRequest::new("RULES\nmulti-line", "the transcript", "test");
        let text = claude_stdin(&req);
        assert!(text.starts_with("RULES\nmulti-line"));
        assert!(text.contains("the transcript"));
    }

    /// No data to summarise (the connectivity probe: `req.user` is empty) must not leave a
    /// trailing blank-line artefact in what the model sees.
    #[test]
    fn claude_stdin_is_just_the_system_prompt_when_there_is_no_user_data() {
        let req = PromptRequest::new("Reply with exactly: OK.", "", "test");
        assert_eq!(claude_stdin(&req), "Reply with exactly: OK.");
    }

    /// Verbatim envelope from the live failure: a signed-out Claude exits non-zero but still
    /// prints a full, already-clean JSON envelope. This exists because that clean text used
    /// to be thrown away entirely - the error shown was the whole raw envelope dumped as
    /// text (`claude exited Some(1): {"type":"result",...}`), not this one-liner.
    const REAL_NOT_LOGGED_IN_ENVELOPE: &str = r#"{"type":"result","subtype":"success","is_error":true,"api_error_status":null,"duration_ms":108,"duration_api_ms":0,"num_turns":1,"result":"Not logged in · Please run /login","stop_reason":"stop_sequence"}"#;

    #[test]
    fn envelope_detail_extracts_the_clean_message_from_a_real_not_logged_in_failure() {
        let payload: serde_json::Value = serde_json::from_str(REAL_NOT_LOGGED_IN_ENVELOPE).unwrap();
        assert_eq!(
            claude_envelope_detail(&payload).as_deref(),
            Some("Not logged in · Please run /login")
        );
    }

    /// The load-bearing property this whole fix is for: `subtype` alone is NOT enough to
    /// tell success from failure (the real payload above has `"subtype":"success"` on a
    /// genuine failure) — `is_error` is the field that actually says so, and it must win.
    #[test]
    fn envelope_detail_trusts_is_error_over_a_misleading_subtype() {
        let payload = serde_json::json!({
            "subtype": "success",
            "is_error": true,
            "result": "something went wrong",
        });
        assert_eq!(
            claude_envelope_detail(&payload).as_deref(),
            Some("something went wrong")
        );
    }

    /// A genuine success (`is_error: false`, `subtype: "success"`) must not be reinterpreted
    /// as a failure - that would turn every working call into an error.
    #[test]
    fn envelope_detail_is_none_for_a_real_success() {
        let payload = serde_json::json!({
            "subtype": "success",
            "is_error": false,
            "result": "the actual summary text",
        });
        assert_eq!(claude_envelope_detail(&payload), None);
    }

    /// Not JSON shaped like an envelope at all (e.g. an empty object from a crash) - `None`,
    /// not a manufactured error, so the caller falls back to raw stderr/stdout text instead
    /// of claiming a specific cause it can't actually support.
    #[test]
    fn envelope_detail_is_none_for_something_that_is_not_an_error_envelope() {
        assert_eq!(claude_envelope_detail(&serde_json::json!({})), None);
        assert_eq!(
            claude_envelope_detail(&serde_json::json!("just a string")),
            None
        );
    }
}
