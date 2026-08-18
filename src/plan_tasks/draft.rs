//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Shaping a rough note into a task draft — ONE provider-agnostic LLM call behind
//! the daily plan's task composer.
//!
//! # What this is
//! The dev types "fix the flaky login test" into the new-task box; this turns it into
//! `{title, description, issue_type}` for them to review and EDIT. It is a formatter,
//! not an author — see `assets/prompts/plan-task-draft.md`.
//!
//! # This call can never block a create
//! [`draft`] returns `Ok(TaskDraft)` even when the model is down, slow, or answers
//! garbage — the failure rides in `TaskDraft::error` and the text fields come back
//! empty for the user to fill in themselves. That is the product rule ("creation NEVER
//! hard-blocks on the AI") expressed in the type, so no caller can accidentally turn a
//! cold model into a dead end. The only `Err` here is a programming error (an empty
//! note), which the CLI rejects before we are called.
//!
//! # Who calls this
//! [`crate::plan_tasks::cli`]'s `plan-task-draft` subcommand → the tray's
//! `draft_plan_task` → the composer's "Draft it" button.
//!
//! # Related
//! - [`crate::llm`] — `complete` picks the user's configured provider per call and
//!   emits the `llm.*` subspans for free.
//! - [`crate::plan_tasks::create`] — where the reviewed draft goes next.

use anyhow::{bail, Result};
use serde::Serialize;
use tracing::field::Empty;
use tracing::Instrument;

use crate::llm::{self, parse_json_object, prompts, PromptRequest};

/// Token budget: three short fields. Generous enough that a long note still yields a
/// complete answer, tight enough that a rambling model can't run away.
const DRAFT_MAX_TOKENS: u32 = 700;

/// Longest note we hand the model. A note is a morning scribble; anything past this is
/// paste-bombing, and truncating beats blowing the context window.
const NOTE_CAP: usize = 2000;

/// Cap on how much of a provider's own words we repeat to the user. Long enough for a
/// real explanation ("model X does not exist", "invalid api key"), short enough that a
/// provider that dumps a stack trace cannot take over the composer.
const REASON_CAP: usize = 180;

/// Turn an [`llm::LlmError`] into one line the user can act on.
///
/// The engine's own words are the point. This used to be the fixed string "Couldn't draft
/// that - write it below." for every possible failure, which told a user whose key had
/// expired, whose model name was wrong, or who had simply run out of quota exactly the
/// same thing: nothing. The cause is already known here - the only reason it was not
/// shown is that nobody passed it on.
fn engine_failure_message(e: &llm::LlmError) -> String {
    let reason = match e {
        // Self-resolving, and worth saying so - "try again" is the right advice here and
        // the wrong advice for everything else.
        llm::LlmError::RateLimited { .. } => {
            return "Your AI provider is rate-limited right now - it will work again shortly."
                .to_string()
        }
        llm::LlmError::Failed(m) => m.trim(),
    };
    if reason.is_empty() {
        return "Your AI provider could not draft that - write it below.".to_string();
    }
    let mut short: String = reason.chars().take(REASON_CAP).collect();
    if reason.chars().count() > REASON_CAP {
        short.push('…');
    }
    format!("Your AI provider could not draft that - {short}")
}

/// A drafted task, in the shape the CLI prints and the tray/UI consume.
///
/// Every field can be empty: an empty draft with `error` set is the honest answer when
/// the model couldn't be reached, and the composer renders it as empty editable fields
/// plus a quiet note rather than an error wall.
#[derive(Debug, Clone, Serialize, Default)]
pub struct TaskDraft {
    pub title: String,
    pub description: String,
    /// `Task` | `Bug`. Defaults to `Task` — the safe guess when the model omits it.
    pub issue_type: String,
    /// Why the draft is empty, phrased for the user. `None` on success.
    pub error: Option<String>,
    /// `true` when the ENGINE is what failed - the provider could not be reached, refused,
    /// or answered with nothing - as opposed to answering with something unparseable.
    ///
    /// The composer needs this because the two cases want opposite affordances, and it
    /// cannot tell them apart from the message. A provider fault is fixable in the AI
    /// picker; a bad answer is transient and wants a retry. It also cannot infer it from
    /// health: `classify_provider_health` scores the last RECORDED test, so a key that
    /// connected fine a minute ago still reads `ok` while every real call is failing, and
    /// the user gets a Try again that fails identically, forever.
    pub provider_down: bool,
}

/// Draft a task from `note`. See the module header: this returns `Ok` with
/// `TaskDraft::error` set rather than `Err` on any model failure.
#[tracing::instrument(skip(note), fields(note_len = note.len()))]
pub async fn draft(note: &str) -> Result<TaskDraft> {
    let note = note.trim();
    if note.is_empty() {
        bail!("a note is required to draft a task");
    }
    let span = tracing::info_span!(
        "plan_task.draft",
        llm_provider = Empty,
        issue_type = Empty,
        drafted = Empty,
    );
    async move {
        let capped: String = note.chars().take(NOTE_CAP).collect();
        let req = PromptRequest {
            system: prompts::PLAN_TASK_DRAFT,
            user: format!("=== NOTE ===\n{capped}\n"),
            schema: Some(prompts::plan_task_draft_schema()),
            max_tokens: DRAFT_MAX_TOKENS,
            label: "plan-task-draft".to_string(),
            // The one call in this backend that a user is watching synchronously — see
            // `PromptRequest::interactive`'s doc. Lets `CursorBackend` pick a faster
            // model tier for this call specifically, without touching the pinned
            // default that summarisation/worklog generation rely on for quality.
            interactive: true,
        };

        let (out, provider) = match llm::complete(&req).await {
            Ok(v) => v,
            Err(e) => {
                // Not an error path for the user: they can still type the task.
                tracing::warn!(error = %e, "plan_task: draft call failed - falling back to manual");
                tracing::Span::current().record("drafted", false);
                return Ok(TaskDraft {
                    issue_type: "Task".to_string(),
                    error: Some(engine_failure_message(&e)),
                    provider_down: true,
                    ..Default::default()
                });
            }
        };
        tracing::Span::current().record("llm_provider", provider.as_str());

        let Some(parsed) = parse_answer(&out.text) else {
            tracing::warn!("plan_task: draft answer could not be parsed - falling back to manual");
            tracing::Span::current().record("drafted", false);
            return Ok(TaskDraft {
                issue_type: "Task".to_string(),
                error: Some("Couldn't draft that - write it below.".to_string()),
                ..Default::default()
            });
        };

        tracing::Span::current().record("issue_type", parsed.issue_type.as_str());
        tracing::Span::current().record("drafted", true);
        tracing::info!(
            input_tokens = out.input_tokens,
            output_tokens = out.output_tokens,
            elapsed_s = out.elapsed_s,
            "plan_task: draft generated"
        );
        Ok(parsed)
    }
    .instrument(span)
    .await
}

/// Tolerantly read the model's answer. `None` only when there was no usable object at
/// all — copilot/cursor enforce no schema, so a prose-wrapped or fenced answer is
/// normal and [`parse_json_object`] handles it.
///
/// A blank title is treated as no answer: the composer's Create button gates on the
/// title, so a draft with an empty title is indistinguishable from not drafting — and
/// saying so honestly beats handing back a description with nothing to call it.
fn parse_answer(text: &str) -> Option<TaskDraft> {
    let v = parse_json_object(text)?;
    let field = |k: &str| -> String {
        v.get(k)
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .trim()
            .to_string()
    };

    let title = field("title");
    if title.is_empty() {
        return None;
    }
    // Anything outside the enum (an unconstrained backend can emit "Story",
    // "Improvement", …) degrades to Task, matching the prompt's own tie-break.
    let issue_type = match field("issue_type").to_lowercase().as_str() {
        "bug" => "Bug",
        _ => "Task",
    };
    Some(TaskDraft {
        title,
        description: field("description"),
        issue_type: issue_type.to_string(),
        error: None,
        provider_down: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_clean_answer() {
        let d = parse_answer(
            r#"{"title":"Fix the flaky login test","description":"It fails in CI.",
                "issue_type":"Bug"}"#,
        )
        .unwrap();
        assert_eq!(d.title, "Fix the flaky login test");
        assert_eq!(d.description, "It fails in CI.");
        assert_eq!(d.issue_type, "Bug");
        assert!(d.error.is_none());
    }

    #[test]
    fn tolerates_a_fenced_or_prose_wrapped_answer() {
        // copilot/cursor get no schema enforcement — this is the normal case there.
        let d = parse_answer(
            "Sure! Here's the task:\n```json\n{\"title\":\"Draft the deck\",\
             \"description\":\"\",\"issue_type\":\"Task\"}\n```",
        )
        .unwrap();
        assert_eq!(d.title, "Draft the deck");
    }

    #[test]
    fn unknown_issue_type_degrades_to_task() {
        for raw in ["Story", "improvement", "", "epic"] {
            let d = parse_answer(&format!(
                r#"{{"title":"T","description":"d","issue_type":"{raw}"}}"#
            ))
            .unwrap();
            assert_eq!(d.issue_type, "Task", "unknown type {raw:?} must fall back");
        }
        // …but a real Bug is preserved regardless of case.
        let d = parse_answer(r#"{"title":"T","description":"d","issue_type":"BUG"}"#).unwrap();
        assert_eq!(d.issue_type, "Bug");
    }

    #[test]
    fn a_titleless_answer_is_no_answer() {
        // The composer gates Create on the title, so a blank one is the same as not
        // drafting — say so rather than hand back an orphaned description.
        assert!(parse_answer(r#"{"title":"  ","description":"d","issue_type":"Task"}"#).is_none());
        assert!(parse_answer(r#"{"description":"d","issue_type":"Task"}"#).is_none());
    }

    #[test]
    fn garbage_is_none_not_a_panic() {
        assert!(parse_answer("I couldn't work out what you meant.").is_none());
        assert!(parse_answer("").is_none());
        assert!(
            parse_answer("[]").is_none(),
            "a bare array is not an object"
        );
    }

    #[test]
    fn an_engine_failure_repeats_what_the_engine_said() {
        // The whole point: the cause reaches the user. This was a fixed string for every
        // failure, so an expired key, a wrong model name and an exhausted quota all read
        // as "Couldn't draft that" - three different fixes, one useless sentence.
        let m = engine_failure_message(&llm::LlmError::Failed(
            "custom provider returned 401: invalid api key".to_string(),
        ));
        assert!(m.contains("invalid api key"), "cause must survive: {m}");
    }

    #[test]
    fn a_rate_limit_is_named_as_temporary() {
        // The one failure where waiting IS the fix. Sending this user to the provider
        // picker to re-check a setup that is working would be a wasted trip.
        let m = llm::LlmError::RateLimited {
            message: "429 too many requests".to_string(),
            retry_after: None,
        };
        let m = engine_failure_message(&m);
        assert!(m.contains("rate-limited"), "{m}");
        assert!(m.contains("shortly"), "must say it resolves itself: {m}");
    }

    #[test]
    fn a_ranting_provider_cannot_take_over_the_composer() {
        let m = engine_failure_message(&llm::LlmError::Failed("x".repeat(5000)));
        assert!(
            m.chars().count() < REASON_CAP + 80,
            "message must stay one line, got {} chars",
            m.chars().count()
        );
    }

    #[test]
    fn an_unparseable_answer_is_not_blamed_on_the_provider() {
        // A model that answered - just badly - is a transient miss, and the composer
        // reads `provider_down` to decide between "retry" and "go fix your provider".
        // Sending someone to reconfigure a working engine over one bad answer is the
        // opposite of helpful.
        assert!(parse_answer("not json at all").is_none());
        let d = parse_answer(r#"{"title":"T","description":"d","issue_type":"Task"}"#).unwrap();
        assert!(!d.provider_down);
    }

    #[tokio::test]
    async fn an_empty_note_is_rejected_before_the_model() {
        assert!(draft("   ").await.is_err());
    }
}
