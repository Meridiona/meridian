//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! The compose pipeline end to end: classify, build the prompt, ask the model, parse.
//!
//! # Never blocks the user
//! [`draft`] always returns a [`DraftOutcome`]. A refusal, a model that is down, a
//! rate limit and an unusable answer are all values the tray turns into a short message;
//! none of them is an `Err`, so no caller can forget to handle one and leave the user
//! staring at a spinner. This is the same rule the task composer's "Draft with AI" follows.
//!
//! # Safety property
//! Text only ever leaves this function as [`DraftOutcome::Text`], and only after
//! [`crate::compose::output::parse`] has accepted it. A refused press never reaches the
//! model at all.
//!
//! # Who calls this
//! The tray's compose controller, once per press.
//!
//! # Related
//! - [`crate::compose::classify`] and [`crate::compose::prompt`] - the steps this chains.
//! - [`crate::llm`] - the provider layer; the request is marked interactive so a hanging
//!   provider fails inside the interactive deadline instead of the tray's kill-timeout.

use std::future::Future;

use tracing::field::Empty;
use tracing::Instrument;

use crate::compose::classify::{classify, Classification};
use crate::compose::output::{self, Parsed};
use crate::compose::prompt::{build, PROMPT_VERSION};
use crate::compose::types::{DraftRequest, Intent, RefusalReason, SurfaceKind};
use crate::llm::{self, LlmError, LlmOutput, PromptRequest};
use meridian_core::llm_provider::LlmProvider;

/// Output budget. A long email stays well under this; a runaway model cannot go far past.
const DRAFT_MAX_TOKENS: u32 = 1200;

/// Longest provider explanation shown to the user.
const REASON_CAP: usize = 180;

/// What a press turned into.
#[derive(Debug, Clone, PartialEq)]
pub enum DraftOutcome {
    /// Do nothing; say why. The model was not called.
    Refused(RefusalReason),
    /// Text to deliver into the box.
    Text {
        text: String,
        surface: SurfaceKind,
        intent: Intent,
    },
    /// The model had too little context to write something trustworthy.
    NoContext,
    /// The model answered, but with nothing safe to insert.
    Unusable,
    /// The model could not be reached or refused. `message` is one line for the user.
    Failed { message: String },
}

fn single_line(surface: SurfaceKind) -> bool {
    matches!(
        surface,
        SurfaceKind::Terminal | SurfaceKind::StructuredField
    )
}

fn failure_message(e: &LlmError) -> String {
    match e {
        LlmError::RateLimited { .. } => {
            "Your AI provider is rate-limited right now - try again shortly.".to_string()
        }
        other => {
            let reason: String = other.to_string().chars().take(REASON_CAP).collect();
            format!("Meridian could not reach your AI provider: {reason}")
        }
    }
}

/// Run the pipeline with an injected completion function. [`draft`] passes the real one;
/// tests pass a fake so no model is needed.
pub async fn draft_with<F, Fut>(req: &DraftRequest, complete: F) -> DraftOutcome
where
    F: FnOnce(PromptRequest) -> Fut,
    Fut: Future<Output = Result<(LlmOutput, LlmProvider), LlmError>>,
{
    let span = tracing::info_span!(
        "compose.draft",
        surface = Empty,
        intent = Empty,
        prompt_version = PROMPT_VERSION,
        outcome = Empty,
        provider = Empty,
        elapsed_s = Empty,
        user_chars = Empty,
        above_chars = Empty,
        below_chars = Empty,
        field_chars = Empty,
        rest_of_window_chars = Empty,
    );
    async {
        let plan = match classify(&req.field) {
            Classification::Refused(reason) => {
                tracing::Span::current().record("outcome", "refused");
                return DraftOutcome::Refused(reason);
            }
            Classification::Ready(plan) => plan,
        };
        tracing::Span::current().record("surface", plan.surface.as_str());
        tracing::Span::current().record("intent", plan.intent.as_str());

        let built = build(req, &plan);
        // Sizes only, never text: what the model was shown, for debugging a press.
        let span = tracing::Span::current();
        let stats = built.stats;
        span.record("user_chars", stats.user_chars);
        span.record("above_chars", stats.above_chars);
        span.record("below_chars", stats.below_chars);
        span.record("field_chars", stats.field_chars);
        span.record("rest_of_window_chars", stats.rest_of_window_chars);

        let request = PromptRequest::new(built.system, built.user, "compose-draft")
            .with_max_tokens(DRAFT_MAX_TOKENS)
            .interactive();

        let out = match complete(request).await {
            Ok((out, provider)) => {
                let span = tracing::Span::current();
                span.record("provider", provider.as_str());
                span.record("elapsed_s", out.elapsed_s);
                out
            }
            Err(e) => {
                tracing::warn!(error = %e, "compose: model call failed");
                tracing::Span::current().record("outcome", "failed");
                return DraftOutcome::Failed {
                    message: failure_message(&e),
                };
            }
        };

        match output::parse(&out.text, single_line(plan.surface)) {
            Parsed::Text(text) => {
                tracing::Span::current().record("outcome", "text");
                DraftOutcome::Text {
                    text,
                    surface: plan.surface,
                    intent: plan.intent,
                }
            }
            Parsed::NoContext => {
                tracing::Span::current().record("outcome", "no_context");
                DraftOutcome::NoContext
            }
            Parsed::Unusable => {
                tracing::warn!("compose: model answer was empty or unusable");
                tracing::Span::current().record("outcome", "unusable");
                DraftOutcome::Unusable
            }
        }
    }
    .instrument(span)
    .await
}

/// Run the pipeline against the user's configured provider.
pub async fn draft(req: &DraftRequest) -> DraftOutcome {
    draft_with(req, |r| async move { llm::complete(&r).await }).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compose::types::{FieldSnapshot, NearbyText};
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn slack_reply() -> DraftRequest {
        DraftRequest {
            field: FieldSnapshot {
                bundle_id: "com.tinyspeck.slackmacgap".into(),
                multiline: true,
                nearby: NearbyText {
                    above: "Priya: can you confirm the rollout plan for Thursday?".into(),
                    ..Default::default()
                },
                ..Default::default()
            },
            previous: None,
            user_name: None,
        }
    }

    fn ok(text: &str) -> Result<(LlmOutput, LlmProvider), LlmError> {
        Ok((
            LlmOutput {
                text: text.into(),
                input_tokens: 0,
                output_tokens: 0,
                elapsed_s: 1.5,
            },
            LlmProvider::Claude,
        ))
    }

    #[tokio::test]
    async fn a_good_answer_becomes_text_with_its_metadata() {
        let out = draft_with(&slack_reply(), |_| async {
            ok("Thursday works, plan is staged by size.")
        })
        .await;
        match out {
            DraftOutcome::Text {
                text,
                surface,
                intent,
            } => {
                assert_eq!(text, "Thursday works, plan is staged by size.");
                assert_eq!(surface, SurfaceKind::DirectChat);
                assert_eq!(intent, Intent::Reply);
            }
            other => panic!("expected Text, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn the_request_is_interactive_static_system_and_carries_the_press() {
        let seen = std::sync::Mutex::new(None);
        let _ = draft_with(&slack_reply(), |r| {
            *seen.lock().unwrap() = Some((r.interactive, r.system, r.user.clone(), r.max_tokens));
            async { ok("fine") }
        })
        .await;
        let (interactive, system, user, max) = seen.lock().unwrap().take().unwrap();
        assert!(interactive);
        assert_eq!(system, crate::compose::prompt::builder::CORE);
        assert!(user.contains("rollout plan for Thursday"));
        assert_eq!(max, DRAFT_MAX_TOKENS);
    }

    #[tokio::test]
    async fn a_refused_press_never_calls_the_model() {
        let calls = AtomicUsize::new(0);
        let mut req = slack_reply();
        req.field.ax_subrole = "AXSecureTextField".into();
        let out = draft_with(&req, |_| {
            calls.fetch_add(1, Ordering::SeqCst);
            async { ok("should never be asked") }
        })
        .await;
        assert_eq!(out, DraftOutcome::Refused(RefusalReason::SecureField));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn the_sentinel_becomes_no_context() {
        assert_eq!(
            draft_with(&slack_reply(), |_| async { ok("[[NO_CONTEXT]]") }).await,
            DraftOutcome::NoContext
        );
    }

    #[tokio::test]
    async fn an_empty_answer_is_unusable_and_never_text() {
        assert_eq!(
            draft_with(&slack_reply(), |_| async { ok("  ") }).await,
            DraftOutcome::Unusable
        );
    }

    #[tokio::test]
    async fn fenced_and_preambled_answers_are_cleaned() {
        let out = draft_with(&slack_reply(), |_| async {
            ok("Here's a draft:\n```\nThursday works.\n```")
        })
        .await;
        assert!(matches!(out, DraftOutcome::Text { ref text, .. } if text == "Thursday works."));
    }

    #[tokio::test]
    async fn a_terminal_press_keeps_only_one_line() {
        let req = DraftRequest {
            field: FieldSnapshot {
                bundle_id: "com.googlecode.iterm2".into(),
                before: "list files by size".into(),
                multiline: false,
                ..Default::default()
            },
            previous: None,
            user_name: None,
        };
        let out = draft_with(&req, |_| async { ok("ls -lS\nThis lists files by size.") }).await;
        assert!(matches!(out, DraftOutcome::Text { ref text, .. } if text == "ls -lS"));
    }

    #[tokio::test]
    async fn provider_failures_become_a_one_line_message_not_an_error() {
        let out = draft_with(&slack_reply(), |_| async {
            Err(LlmError::RateLimited {
                message: "quota".into(),
                retry_after: None,
            })
        })
        .await;
        match out {
            DraftOutcome::Failed { message } => assert!(message.contains("rate-limited")),
            other => panic!("expected Failed, got {other:?}"),
        }
    }
}
