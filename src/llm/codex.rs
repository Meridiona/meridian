//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Codex backend — `codex exec`, on the user's own ChatGPT subscription.
//!
//! Structured output is real: `--output-schema` is validated, and the final message is
//! written to a file (`-o`) rather than stdout, so we read it from there. Both the schema
//! and the output file live in a temp dir removed on drop, even on a panic or timeout.
//!
//! `-s read-only` + `--skip-git-repo-check` + `--ephemeral`: this is a summarisation call,
//! not an agent session. It must not touch the filesystem or leave a rollout behind.
//!
//! The shared schema is rewritten to OpenAI's strict dialect on the way out — see
//! [`crate::llm::schema::strictify`], without which no schema-bearing call reaches the
//! model at all.
//!
//! # The whole prompt goes over stdin, not argv (Windows)
//!
//! It used to be `codex exec <req.system>` (positional argv) + `req.user` piped over stdin.
//! `codex` resolves to `codex.cmd` on Windows - an npm-generated batch file, not a native exe -
//! and Rust's std library REFUSES to spawn a `.bat`/`.cmd` target when an argument contains
//! characters it cannot safely escape (the CVE-2024-24576 "BatBadBut" fix), notably embedded
//! newlines. `req.system` is loaded from a Markdown rules file (`SUMMARY_RULES`), so it is
//! always multi-line — meaning every real summarisation call failed to even spawn on Windows
//! with `io::Error { kind: InvalidInput, "batch file arguments are invalid" }`, confirmed live
//! with the exact production prompt. `codex exec` (no positional prompt at all) reads the whole
//! prompt from stdin instead - documented, and confirmed live - and stdin has no such
//! restriction on any platform, so this fixes Windows without changing behaviour anywhere else.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use serde_json::Value;

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

/// The real reason a `codex exec` call failed, out of its stderr.
///
/// Codex writes an informational banner first — `Reading additional input from
/// stdin...`, the model/sandbox header — so the FIRST line is never the error, and
/// reporting it (as this backend once did) surfaced the stdin notice as the cause of
/// every failure, which is how a 400 `invalid_json_schema` masqueraded as a stdin
/// problem. The real failure is an `ERROR:` line followed by the API's JSON body, so
/// prefer that body's `message`; fall back to the raw `ERROR:` text, and only then to
/// the first line (a crash with no ERROR block at all).
fn codex_error(stderr: &str) -> String {
    let Some(rest) = stderr.split("ERROR:").nth(1) else {
        return sp::first_line(stderr);
    };
    // The body is pretty-printed JSON, so it spans lines — let serde find where it ends
    // rather than guessing, and fall back to the text if it isn't JSON at all.
    if let Some(Ok(v)) = serde_json::Deserializer::from_str(rest.trim())
        .into_iter::<Value>()
        .next()
    {
        if let Some(msg) = v
            .pointer("/error/message")
            .or_else(|| v.pointer("/message"))
            .and_then(Value::as_str)
        {
            return msg.chars().take(300).collect();
        }
    }
    sp::first_line(rest)
}

/// How long the cheap "are you even signed in" pre-check may take. Purely local (reads a
/// cached credential file, no network round-trip) — measured ~0.2s on codex-cli 0.145.0 — so
/// this is a generous ceiling, not a real budget.
const LOGIN_STATUS_TIMEOUT_S: u64 = 8;

/// Fast pre-flight: is Codex signed out? `None` means "proceed with the real call" — covers
/// both "signed in" and "couldn't tell" (a pre-check that itself fails or times out must never
/// block a call that might otherwise succeed; fail open).
///
/// # Why this exists
///
/// Without it, a signed-out Codex fails through the SLOW path: `codex exec` reaches the
/// network, gets a 401, and retries — 5 WebSocket reconnects, a fallback to HTTPS, 5 more
/// retries — before finally giving up at ~23s (measured live). That is LONGER than the Test
/// Connection button's 20s budget (`PROBE_TIMEOUT_S` in `super::detect`), so the real "you're
/// not signed in" never reaches the user — they see a bare, unactionable "timed out" instead,
/// on every real hourly call too, not just the Test button. `codex login status` answers the
/// same question in a fraction of a second because it only reads a local credential file.
async fn signed_out(cfg: &LlmConfig) -> Option<String> {
    (login_status_signed_in(&cfg.meridian_home).await == Some(false)).then(|| {
        "Codex isn't signed in yet - run `codex login` in a terminal, then try again.".to_string()
    })
}

/// Ternary read of `codex login status`: `Some(true)` = confirmed signed in, `Some(false)` =
/// confirmed signed out (the same "not logged in" text [`classify_login_status`] already
/// recognises), `None` = the check itself was inconclusive (spawn/timeout failure, or a
/// result that matches neither case) and must not be trusted either way.
///
/// `pub(crate)`: shared ground truth for two callers that must never disagree - this call's
/// own [`signed_out`] pre-flight, and `detect::codex_sign_in`'s post-hoc verification of
/// whether an interactive login actually took, independent of what the spawned CLI process
/// itself reported (a hung or non-zero-exiting `codex login` doesn't necessarily mean the
/// OAuth round-trip failed - only this file read does).
pub(crate) async fn login_status_signed_in(meridian_home: &std::path::Path) -> Option<bool> {
    let cap = run_capture(
        "codex",
        &["login".into(), "status".into()],
        "",
        meridian_home,
        LOGIN_STATUS_TIMEOUT_S,
        &[],
        &[],
    )
    .await
    .ok()?;
    ternary_login_status(cap.success, &cap.stdout, &cap.stderr)
}

/// The pure decision [`login_status_signed_in`] makes from a captured result — split out so
/// it is unit-testable without spawning a process, matching [`classify_login_status`].
fn ternary_login_status(success: bool, stdout: &str, stderr: &str) -> Option<bool> {
    if classify_login_status(success, stdout, stderr).is_some() {
        return Some(false);
    }
    success.then_some(true)
}

/// The pure classification `signed_out` applies to `codex login status`'s result — split out
/// so it can be unit-tested without spawning a process or touching `resolve_cli`'s global
/// success cache (a fake `codex` on `PATH` would otherwise memoise under the real key).
///
/// Only trusts the "not logged in" text when the command also FAILED (non-zero exit): the
/// message is checked as a substring, so a coincidental match in a differently-shaped success
/// message (or a future CLI version's phrasing) must not misfire and block a working call.
/// Anything else — an unrelated failure, a version that phrases this differently — returns
/// `None` and lets the real call proceed; a wrong "you're signed out" would be worse than one
/// slow, honestly-reported real failure.
fn classify_login_status(success: bool, stdout: &str, stderr: &str) -> Option<String> {
    let blob = format!("{stdout}\n{stderr}").to_lowercase();
    (!success && blob.contains("not logged in")).then(|| {
        "Codex isn't signed in yet - run `codex login` in a terminal, then try again.".to_string()
    })
}

/// The full prompt sent over stdin - see the module doc on why nothing goes over argv
/// anymore. `req.system` (always multi-line - it's a Markdown rules file) is the
/// instructions; `req.user`, when present, is the data to summarise, demarcated with
/// `<stdin>` tags the same way `codex exec` itself used to wrap piped stdin when a
/// positional prompt was ALSO given - so the model sees the identical effective text it did
/// before this moved off argv, just assembled by us instead of by codex internally.
fn codex_stdin(req: &PromptRequest) -> String {
    if req.user.is_empty() {
        req.system.to_string()
    } else {
        format!("{}\n\n<stdin>\n{}\n</stdin>\n", req.system, req.user)
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

        if let Some(msg) = signed_out(&self.cfg).await {
            return Err(LlmError::Failed(msg));
        }

        let td = std::env::temp_dir().join(format!(
            "meridian-llm-codex-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&td)
            .map_err(|e| LlmError::Failed(format!("codex: temp dir: {e}")))?;
        let _guard = TempDirGuard(td.clone());

        let out_path = td.join("last_message.txt");
        // No positional prompt argument - see the module doc. `req.system` goes over stdin
        // instead, along with `req.user`.
        let mut args: Vec<String> = vec![
            "exec".into(),
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
            std::fs::write(&schema_path, super::schema::strictify(schema).to_string())
                .map_err(|e| LlmError::Failed(format!("codex: write schema: {e}")))?;
            args.push("--output-schema".into());
            args.push(schema_path.display().to_string());
        }
        if !self.cfg.model.is_empty() {
            args.push("-m".into());
            args.push(self.cfg.model.clone());
        }

        let stdin_text = codex_stdin(req);
        let cap = run_capture(
            "codex",
            &args,
            &stdin_text,
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
                return Err(LlmError::rate_limited(if msg.is_empty() {
                    "rate/usage limit".into()
                } else {
                    msg
                }));
            }
            // The summary line is bounded, so keep the whole of stderr on the span: a
            // rejected request answers WHY only in the API's JSON body, and that body is
            // the one thing worth having when this fails.
            tracing::error!(
                code = ?cap.code,
                stderr = %cap.stderr,
                "codex exec failed"
            );
            return Err(LlmError::Failed(format!(
                "codex exited {:?}: {}",
                cap.code,
                codex_error(&cap.stderr)
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Verbatim stderr from the live failure (codex-cli 0.141.0). The banner's first line
    /// is what this backend used to report as the cause.
    const REAL_STDERR: &str = "Reading additional input from stdin...\nOpenAI Codex v0.141.0\n--------\nworkdir: /Users/x/.meridian\nmodel: gpt-5.5\n--------\nERROR: {\n  \"type\": \"error\",\n  \"error\": {\n    \"type\": \"invalid_request_error\",\n    \"code\": \"invalid_json_schema\",\n    \"message\": \"Invalid schema for response_format 'codex_output_schema': In context=('properties', 'placements', 'items'), 'required' is required to be supplied and to be an array including every key in properties. Missing 'id'.\",\n    \"param\": \"text.format.schema\"\n  },\n  \"status\": 400\n}\n";

    #[test]
    fn codex_error_reports_the_api_message_not_the_stdin_banner() {
        let msg = codex_error(REAL_STDERR);
        assert!(
            msg.starts_with("Invalid schema for response_format"),
            "got: {msg}"
        );
        assert!(
            !msg.contains("stdin"),
            "the banner must never be the reported cause: {msg}"
        );
    }

    /// No ERROR block (a crash / a usage message) still has to say something.
    #[test]
    fn codex_error_falls_back_to_the_first_line() {
        assert_eq!(
            codex_error("codex: command failed\n\n"),
            "codex: command failed"
        );
        assert_eq!(codex_error(""), "");
    }

    /// An ERROR block that isn't JSON (a panic, a plain-text failure) reports its text.
    #[test]
    fn codex_error_handles_a_non_json_error_block() {
        assert_eq!(
            codex_error("banner\nERROR: stream disconnected\n"),
            "stream disconnected"
        );
    }

    /// The regression this exists for: a signed-out Codex used to fail through `codex exec`'s
    /// slow retry-then-401 path (~23s, measured live) - longer than the Test Connection
    /// button's 20s budget, so the user saw a bare "timed out" instead of "you're not signed
    /// in". `codex login status` (real output, verbatim) answers the same question instantly.
    #[test]
    fn classify_login_status_recognises_the_real_not_logged_in_output() {
        let msg = classify_login_status(false, "Not logged in\n", "");
        assert!(msg.is_some());
        assert!(msg.unwrap().contains("codex login"));
    }

    #[test]
    fn classify_login_status_is_case_insensitive_and_checks_stderr_too() {
        assert!(classify_login_status(false, "NOT LOGGED IN", "").is_some());
        assert!(classify_login_status(false, "", "Not Logged In").is_some());
    }

    /// A SUCCESSFUL call must never be misread as signed-out, even if "not logged in" somehow
    /// appears in its output (e.g. echoed back from a prompt) - trusting text over the exit
    /// code here would risk blocking a call that actually works.
    #[test]
    fn classify_login_status_ignores_the_text_on_success() {
        assert_eq!(classify_login_status(true, "not logged in", ""), None);
    }

    /// An unrelated failure (network hiccup, a future CLI's different phrasing, a crash) must
    /// return `None` and let the real call proceed - misclassifying it as "signed out" would
    /// block a call for the wrong reason and hide the actual error.
    #[test]
    fn classify_login_status_does_not_misclassify_an_unrelated_failure() {
        assert_eq!(
            classify_login_status(false, "", "connection reset by peer"),
            None
        );
        assert_eq!(classify_login_status(false, "", ""), None);
    }

    #[test]
    fn ternary_login_status_reads_the_positive_case() {
        assert_eq!(ternary_login_status(true, "Logged in as x", ""), Some(true));
    }

    #[test]
    fn ternary_login_status_reads_the_confirmed_negative_case() {
        assert_eq!(
            ternary_login_status(false, "Not logged in\n", ""),
            Some(false)
        );
    }

    /// An unrecognised failure must stay `None` (inconclusive), never collapse to either
    /// extreme - the whole reason this returns three states instead of a bool. A caller
    /// treating this as "confirmed signed out" would report a real sign-in as failed; treating
    /// it as "confirmed signed in" would report a real failure as success.
    #[test]
    fn ternary_login_status_is_inconclusive_on_an_unrecognised_failure() {
        assert_eq!(
            ternary_login_status(false, "", "connection reset by peer"),
            None
        );
    }

    /// The regression this exists for: `req.system` used to go over argv (`codex exec
    /// <system>`), and `req.system` is loaded from a Markdown rules file - always multi-line.
    /// `codex.cmd` is a batch file on Windows, and Rust refuses to spawn a `.bat`/`.cmd`
    /// target with a newline in any argument (confirmed live: "batch file arguments are
    /// invalid"), so every real summarisation call failed to even spawn. Pins that the
    /// combined stdin text preserves the exact same content the model used to see, just
    /// assembled by us instead of relying on codex's own `<stdin>`-wrapping of piped input.
    #[test]
    fn codex_stdin_wraps_user_data_the_way_codex_itself_used_to() {
        let req = PromptRequest::new("RULES\nmulti-line", "the transcript", "test");
        let text = codex_stdin(&req);
        assert!(text.starts_with("RULES\nmulti-line"));
        assert!(text.contains("<stdin>\nthe transcript\n</stdin>"));
    }

    /// No data to summarise (the connectivity probe: `req.user` is empty) must not leave a
    /// pointless empty `<stdin></stdin>` wrapper in what the model sees.
    #[test]
    fn codex_stdin_is_just_the_system_prompt_when_there_is_no_user_data() {
        let req = PromptRequest::new("Reply with exactly: OK.", "", "test");
        assert_eq!(codex_stdin(&req), "Reply with exactly: OK.");
    }
}
