//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! The model and thinking effort Meridian asks each provider's CLI for when the user has not
//! pinned a model of their own.
//!
//! # Why this exists
//! Left alone, a CLI uses whatever default its vendor ships this month. That default can change
//! under the user without a Meridian release, and it is tuned for open-ended coding work, not
//! for the short, structured jobs Meridian runs. One table here makes the choice explicit and
//! reviewable instead of scattered across backends.
//!
//! # Rules
//! - A user's own pin (`llm_provider_model` in settings, or a per-request override) always wins
//!   over `model`; `effort` still applies.
//! - The settings are an optimisation, never a requirement: when the CLI rejects them (a retired
//!   model id, an older CLI without `--effort`) the call is retried once without them. Other
//!   failures - a timeout, signed out, a crash - are not the tuning's fault and are returned
//!   as they are, because a second attempt would only double the wait and the quota.
//! - Providers not listed here are left alone on purpose: Cursor has its own tier ladder,
//!   Copilot's CLI takes neither flag, and a custom endpoint's model is its own required field.
//! - The coding-agent session summariser is separate and keeps its own Haiku default
//!   (`SUMMARISER_MODEL`); it does not read this table.
//!
//! # Who calls this
//! [`super::claude`] and [`super::codex`], once per call, through [`complete_with_fallback`].

use std::future::Future;

use super::{LlmError, LlmOutput, LlmProvider};

/// What to pass to a provider's CLI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tuning {
    /// Model id to use when the user has not pinned one. `None` leaves the model to the CLI.
    pub model: Option<&'static str>,
    /// Thinking effort. `low` is the lowest the CLIs offer; none can switch thinking off.
    pub effort: &'static str,
}

/// The default tuning for a provider, if it has one.
pub fn default_for(provider: LlmProvider) -> Option<Tuning> {
    match provider {
        LlmProvider::Claude => Some(Tuning {
            model: Some("claude-sonnet-5-5"),
            effort: "low",
        }),
        // No model pinned: Codex model ids differ by plan, and a wrong id fails the call.
        LlmProvider::Codex => Some(Tuning {
            model: None,
            effort: "low",
        }),
        _ => None,
    }
}

/// Phrases a CLI uses when it refuses a flag or a model, lower case.
const REJECTION_PHRASES: &[&str] = &[
    "unknown option",
    "unknown argument",
    "unrecognized",
    "unexpected argument",
    "invalid value",
    "invalid model",
    "unknown model",
    "model not found",
    "model_not_found",
    "does not exist",
    "may not exist",
    "issue with the selected model",
    "not available",
    "unsupported",
    "--effort",
    "reasoning_effort",
];

/// True when a failure reads as the CLI refusing the tuning (a flag or a model), as opposed to
/// a timeout, a sign-in problem or a crash. A timeout is checked first: its text can mention
/// a model name.
fn looks_like_tuning_rejection(error: &LlmError) -> bool {
    let LlmError::Failed(message) = error else {
        return false;
    };
    let message = message.to_lowercase();
    if message.contains("timed out") || message.contains("timeout") {
        return false;
    }
    REJECTION_PHRASES.iter().any(|p| message.contains(p))
}

/// Run `attempt` with the provider's default tuning, and once more with none if the CLI
/// rejected the tuning.
///
/// This is the one place the retry rule lives, so every backend gets it identically. Rate
/// limits, timeouts and every other failure are returned as-is. A provider with no table entry
/// is called once, untuned.
pub async fn complete_with_fallback<F, Fut>(
    provider: LlmProvider,
    attempt: F,
) -> Result<LlmOutput, LlmError>
where
    F: Fn(Option<Tuning>) -> Fut,
    Fut: Future<Output = Result<LlmOutput, LlmError>>,
{
    let Some(tuning) = default_for(provider) else {
        return attempt(None).await;
    };
    match attempt(Some(tuning)).await {
        Err(e) if looks_like_tuning_rejection(&e) => {
            tracing::warn!(
                error = %e,
                provider = provider.as_str(),
                "llm: the CLI rejected the speed settings, retrying without them"
            );
            attempt(None).await
        }
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_is_pinned_to_sonnet_5_5_at_low_effort() {
        let t = default_for(LlmProvider::Claude).unwrap();
        assert_eq!(t.model, Some("claude-sonnet-5-5"));
        assert_eq!(t.effort, "low");
    }

    #[test]
    fn codex_gets_low_effort_but_keeps_the_users_model() {
        let t = default_for(LlmProvider::Codex).unwrap();
        assert_eq!(t.model, None);
        assert_eq!(t.effort, "low");
    }

    #[test]
    fn providers_with_their_own_model_handling_are_left_alone() {
        assert_eq!(default_for(LlmProvider::Copilot), None);
        assert_eq!(default_for(LlmProvider::Cursor), None);
    }

    use std::sync::atomic::{AtomicU32, Ordering};

    fn ok() -> LlmOutput {
        LlmOutput::default()
    }

    #[tokio::test]
    async fn a_failed_tuned_call_is_retried_once_without_tuning() {
        let calls = AtomicU32::new(0);
        let out = complete_with_fallback(LlmProvider::Claude, |t| {
            let n = calls.fetch_add(1, Ordering::SeqCst);
            async move {
                if t.is_some() {
                    assert_eq!(n, 0);
                    Err(LlmError::Failed("error: unknown option '--effort'".into()))
                } else {
                    Ok(ok())
                }
            }
        })
        .await;
        assert!(out.is_ok());
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn a_rate_limit_is_not_retried() {
        let calls = AtomicU32::new(0);
        let out = complete_with_fallback(LlmProvider::Claude, |_| {
            calls.fetch_add(1, Ordering::SeqCst);
            async { Err(LlmError::rate_limited("limit")) }
        })
        .await;
        assert!(out.unwrap_err().is_rate_limited());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn an_untuned_provider_is_called_once() {
        let calls = AtomicU32::new(0);
        let out = complete_with_fallback(LlmProvider::Copilot, |t| {
            assert!(t.is_none());
            calls.fetch_add(1, Ordering::SeqCst);
            async { Err(LlmError::Failed("boom".into())) }
        })
        .await;
        assert!(out.is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_timeout_is_not_retried() {
        let calls = AtomicU32::new(0);
        let out = complete_with_fallback(LlmProvider::Claude, |_| {
            calls.fetch_add(1, Ordering::SeqCst);
            async {
                Err(LlmError::Failed(
                    "claude timed out after 300s (model sonnet)".into(),
                ))
            }
        })
        .await;
        assert!(out.is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_signed_out_cli_is_not_retried() {
        let calls = AtomicU32::new(0);
        let out = complete_with_fallback(LlmProvider::Codex, |_| {
            calls.fetch_add(1, Ordering::SeqCst);
            async { Err(LlmError::Failed("Codex isn't signed in yet".into())) }
        })
        .await;
        assert!(out.is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn model_and_flag_rejections_are_recognised() {
        for m in [
            "error: unknown option '--effort'",
            "There's an issue with the selected model (claude-sonnet-5-5). It may not exist",
            "Model not found: claude-sonnet-5-5",
            "unexpected argument '-c' found",
        ] {
            assert!(
                looks_like_tuning_rejection(&LlmError::Failed(m.into())),
                "{m}"
            );
        }
    }
}
