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
        };

        let (out, provider) = match llm::complete(&req).await {
            Ok(v) => v,
            Err(e) => {
                // Not an error path for the user: they can still type the task.
                tracing::warn!(error = %e, "plan_task: draft call failed - falling back to manual");
                tracing::Span::current().record("drafted", false);
                return Ok(TaskDraft {
                    issue_type: "Task".to_string(),
                    error: Some("Couldn't draft that - write it below.".to_string()),
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

    #[tokio::test]
    async fn an_empty_note_is_rejected_before_the_model() {
        assert!(draft("   ").await.is_err());
    }
}
