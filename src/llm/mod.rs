//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! The centralised LLM layer — one choice, one factory, every prose call obeys it.
//!
//! The user picks their AI once (setup wizard, changeable in Settings): their own
//! Claude / Codex / Cursor / Copilot CLI subscription, or the on-device MLX model.
//! [`resolver::resolve`] turns that stored choice into a live [`LlmBackend`], and
//! **every** prose LLM call in the product goes through it.
//!
//! # The one exception, on purpose
//!
//! The coding-agent summariser (`crate::coding_agent_session_ingest::summariser`) does
//! **not** use this. It routes each transcript to the agent that produced it — a Codex
//! session is summarised by `codex`, a Claude session by `claude`. That agent already
//! has the context and its subscription is already paid for. Do not "unify" it.
//!
//! # Division of labour: the model judges, code counts
//!
//! Backends return text. They are never asked for arithmetic — asked to sum minutes the
//! 2B once wrote "13:04-13:58 (2 min)" for a 54-minute span. Durations are computed from
//! the session records by the caller.
//!
//! # Related
//! - [`meridian_core::LlmProvider`] — the enum + the stored setting.
//! - [`resolver`] — the single factory, and the rate-limit backoff.

pub mod claude;
pub mod codex;
pub mod config;
pub mod copilot;
pub mod cursor;
pub mod detect;
pub mod openai_compat;
pub mod probe;
pub mod prompts;
pub mod reset_time;
pub mod resolver;
pub mod schema;

use std::fmt;

use async_trait::async_trait;
use serde_json::Value;

pub use config::LlmConfig;
pub use meridian_core::LlmProvider;
pub use resolver::{complete, resolve};

/// The one env var every CLI provider gets, on every call: the widely-respected
/// `DO_NOT_TRACK` opt-out (https://consoledonottrack.com). Provider-specific telemetry
/// levers are added on top of this inside each backend (Claude's
/// `CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC`, Codex's `--config analytics.enabled=false`).
///
/// SCOPE — read this before "improving" it. This disables **telemetry / usage
/// collection**, which we CAN control per invocation. It does **not** disable **training
/// on your prompts** — that is an account-level setting for all four providers (OpenAI
/// Data Controls, GitHub Copilot policy, Cursor Privacy Mode, Anthropic settings) and a
/// subprocess cannot flip it. The setup/Settings UI says so and links each provider's
/// toggle; do not add copy here claiming we turned training off, because we didn't.
pub const DO_NOT_TRACK: (&str, &str) = ("DO_NOT_TRACK", "1");

/// Why a backend failed.
///
/// The distinction drives the fallback: a `RateLimited` provider is out of quota and
/// retrying is pointless — switch to the on-device model *now*. A `Failed` one may be a
/// transient blip, so it earns one retry first.
///
/// Mirrors the summariser's `SummariserError`, which established this split.
#[derive(Debug, Clone)]
pub enum LlmError {
    /// The user's subscription is exhausted. Do not retry — fall back immediately.
    RateLimited(String),
    /// Anything else: not installed, timed out, bad output, non-zero exit.
    Failed(String),
}

impl fmt::Display for LlmError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LlmError::RateLimited(m) => write!(f, "rate-limited: {m}"),
            LlmError::Failed(m) => write!(f, "{m}"),
        }
    }
}
impl std::error::Error for LlmError {}

impl LlmError {
    pub fn is_rate_limited(&self) -> bool {
        matches!(self, LlmError::RateLimited(_))
    }
}

/// One prose call: a system prompt, the assembled input, and optionally a demand for
/// structured output.
///
/// `schema` means "answer in this JSON shape". How that is enforced differs per backend
/// and the trait deliberately hides it: `claude` has `--json-schema`, `codex` has
/// `--output-schema`, and `copilot`/`cursor` have nothing at all — for those two the
/// contract rides in the prompt and the answer is
/// parsed tolerantly. The caller must therefore treat a schema as a *request*, not a
/// guarantee, and cope with an unparseable answer (see the callers' even-split fallback).
pub struct PromptRequest {
    pub system: &'static str,
    pub user: String,
    pub schema: Option<Value>,
    pub max_tokens: u32,
    /// Trace label (the hour, e.g. "2026-07-10T13") — for spans and logs, never the model.
    pub label: String,
}

impl PromptRequest {
    pub fn new(system: &'static str, user: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            system,
            user: user.into(),
            schema: None,
            max_tokens: 2048,
            label: label.into(),
        }
    }

    pub fn with_schema(mut self, schema: Value) -> Self {
        self.schema = Some(schema);
        self
    }

    pub fn with_max_tokens(mut self, n: u32) -> Self {
        self.max_tokens = n;
        self
    }
}

/// What a backend said. A transport type — deliberately NOT the summariser's
/// `EngineOutput`, which is a domain type meaning "a validated session summary".
#[derive(Debug, Clone, Default)]
pub struct LlmOutput {
    /// The model's answer: prose, or JSON when a schema was requested.
    pub text: String,
    /// The CLI backends do not report token counts, so these are 0 for them; a custom
    /// OpenAI-compatible endpoint returns real numbers.
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub elapsed_s: f64,
}

/// One AI backend. Implemented by the four CLIs and the custom cloud endpoint.
///
/// Intentionally narrow: the trait does not expose stdin-vs-argv, model flags, or
/// schema mechanics, because the backends genuinely disagree about all three (`claude -p`
/// and `codex exec` read stdin; `copilot -p` ignores stdin entirely and needs the payload
/// in argv). Each impl decides. The caller only ever says: here is a prompt, answer it.
#[async_trait]
pub trait LlmBackend: Send + Sync {
    fn provider(&self) -> LlmProvider;
    async fn complete(&self, req: &PromptRequest) -> Result<LlmOutput, LlmError>;
}

/// Pull a JSON object out of a model's answer, however it wrapped it.
///
/// The schema-less backends (copilot, cursor) return JSON bare, inside ```fences```, or
/// buried in a sentence of prose. This tolerates all three. `None` means there was no
/// object at all — the caller degrades rather than failing the hour.
pub fn parse_json_object(text: &str) -> Option<Value> {
    if let Ok(v) = serde_json::from_str::<Value>(text.trim()) {
        if v.is_object() {
            return Some(v);
        }
    }
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    if start < end {
        serde_json::from_str::<Value>(&text[start..=end]).ok()
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bare_fenced_and_prose_wrapped_json() {
        let want = serde_json::json!({"activities": [{"activity": 1, "minutes": 30}]});
        // Bare — what a schema-enforced backend returns.
        assert_eq!(
            parse_json_object(r#"{"activities":[{"activity":1,"minutes":30}]}"#),
            Some(want.clone())
        );
        // Fenced — copilot's habit.
        assert_eq!(
            parse_json_object("```json\n{\"activities\":[{\"activity\":1,\"minutes\":30}]}\n```"),
            Some(want.clone())
        );
        // Prose-wrapped — cursor's habit.
        assert_eq!(
            parse_json_object(
                "Here is the breakdown:\n{\"activities\":[{\"activity\":1,\"minutes\":30}]}\nHope that helps!"
            ),
            Some(want)
        );
    }

    #[test]
    fn garbage_is_none_not_an_error() {
        // A model that ignored the contract entirely must not fail the hour — the caller
        // falls back to an even split, which is honest, rather than dying.
        assert_eq!(
            parse_json_object("I could not determine the minutes."),
            None
        );
        assert_eq!(parse_json_object(""), None);
        assert_eq!(parse_json_object("{ not json at all"), None);
    }

    #[test]
    fn a_bare_array_is_not_an_object() {
        assert_eq!(parse_json_object("[1, 2, 3]"), None);
    }

    #[test]
    fn rate_limited_is_distinguishable() {
        assert!(LlmError::RateLimited("quota".into()).is_rate_limited());
        assert!(!LlmError::Failed("not on PATH".into()).is_rate_limited());
    }
}
