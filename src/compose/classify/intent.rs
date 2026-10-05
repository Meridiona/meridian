//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Deciding what the user is trying to do, from facts about the field alone.
//!
//! # The rule order is the design
//! A highlight is the most explicit thing a user can point at, so it wins over
//! everything. A short structured field is a value, never prose. An empty field means
//! "write the message this place implies" (an opener, or the next reply). Text in the
//! field is either a draft or a note to a ghostwriter - which rules cannot tell apart,
//! so [`Intent::Refine`] passes that call to the model, with a default of "draft" in the
//! prompt because wrongly answering a draft destroys the user's words while wrongly
//! polishing a note only costs a second press.
//!
//! # Who calls this
//! [`super::classify`], after the surface is known.
//!
//! # Related
//! - [`super::surface`] - runs first; the intent depends on the surface.

use crate::compose::types::{FieldSnapshot, Intent, SurfaceKind};

/// Fewer meaningful characters than this above the field and there is no conversation
/// to answer. Chosen above a stray "Reply" or "Send" label and below one real sentence.
const MIN_THREAD_CHARS: usize = 20;

fn meaningful_chars(s: &str) -> usize {
    s.chars().filter(|c| !c.is_whitespace()).count()
}

/// True when something above the field is a conversation or post to answer.
///
/// An email compose window reports its own header fields as nearby text, which would
/// make every new email look like a reply, so for email only the quoted thread inside the
/// compose body counts.
fn has_thread(field: &FieldSnapshot, kind: SurfaceKind) -> bool {
    if kind == SurfaceKind::EmailCompose {
        return meaningful_chars(&field.below_draft) >= MIN_THREAD_CHARS;
    }
    meaningful_chars(&field.nearby.above) >= MIN_THREAD_CHARS
        || meaningful_chars(&field.below_draft) >= MIN_THREAD_CHARS
}

/// True when prose looks cut off mid-thought: it does not end in terminal punctuation.
fn is_unfinished(text: &str) -> bool {
    match text.trim_end().chars().last() {
        None => false,
        Some(c) => !matches!(
            c,
            '.' | '!'
                | '?'
                | ':'
                | ';'
                | ')'
                | ']'
                | '"'
                | '\''
                | '\u{2026}'
                | '\u{201d}'
                | '\u{2019}'
        ),
    }
}

/// The intent for a press that was not refused.
pub fn detect(field: &FieldSnapshot, kind: SurfaceKind) -> Intent {
    if field.has_selection() {
        return Intent::RewriteSelection;
    }
    if kind == SurfaceKind::StructuredField {
        return Intent::FillField;
    }
    if field.is_empty_field() {
        return if has_thread(field, kind) {
            Intent::Reply
        } else {
            Intent::StartConversation
        };
    }
    let caret_at_end = field.after.trim().is_empty();
    let continues = match kind {
        // Code almost always wants the next chunk, finished line or not.
        SurfaceKind::CodeEditor => caret_at_end,
        // Prose documents continue only when the last sentence is cut off; a finished
        // paragraph is a draft to polish.
        SurfaceKind::Document => caret_at_end && is_unfinished(&field.before),
        _ => false,
    };
    if continues {
        Intent::Continue
    } else {
        Intent::Refine
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compose::types::NearbyText;

    fn typed(text: &str) -> FieldSnapshot {
        FieldSnapshot {
            before: text.into(),
            multiline: true,
            ..Default::default()
        }
    }

    fn with_thread(mut f: FieldSnapshot) -> FieldSnapshot {
        f.nearby = NearbyText {
            above: "Priya: can you confirm the rollout plan for Thursday?".into(),
            ..Default::default()
        };
        f
    }

    #[test]
    fn selection_wins_over_everything() {
        let mut f = typed("");
        f.selection = "make this better".into();
        for kind in SurfaceKind::ALL {
            assert_eq!(detect(&f, kind), Intent::RewriteSelection, "{kind:?}");
        }
    }

    #[test]
    fn structured_field_is_always_a_value() {
        assert_eq!(
            detect(&typed("Intro call"), SurfaceKind::StructuredField),
            Intent::FillField
        );
        assert_eq!(
            detect(&typed(""), SurfaceKind::StructuredField),
            Intent::FillField
        );
    }

    #[test]
    fn empty_field_with_a_thread_is_a_reply() {
        assert_eq!(
            detect(&with_thread(typed("")), SurfaceKind::DirectChat),
            Intent::Reply
        );
    }

    #[test]
    fn empty_field_without_a_thread_starts_a_conversation() {
        assert_eq!(
            detect(&typed(""), SurfaceKind::DirectChat),
            Intent::StartConversation
        );
    }

    #[test]
    fn a_stray_label_is_not_a_thread() {
        let mut f = typed("");
        f.nearby.above = "Reply".into();
        assert_eq!(
            detect(&f, SurfaceKind::DirectChat),
            Intent::StartConversation
        );
    }

    #[test]
    fn new_email_with_only_header_fields_is_a_start_not_a_reply() {
        let mut f = typed("");
        f.nearby.above = "To: Nabeel Al-Kady   Subject: Intro call   Cc Bcc".into();
        assert_eq!(
            detect(&f, SurfaceKind::EmailCompose),
            Intent::StartConversation
        );
    }

    #[test]
    fn email_with_a_quoted_thread_is_a_reply() {
        let mut f = typed("");
        f.below_draft = "On Mon, Priya wrote: can you confirm the rollout plan?".into();
        assert_eq!(detect(&f, SurfaceKind::EmailCompose), Intent::Reply);
    }

    #[test]
    fn typed_text_in_chat_is_refine() {
        assert_eq!(
            detect(&typed("sounds good will do"), SurfaceKind::DirectChat),
            Intent::Refine
        );
    }

    #[test]
    fn unfinished_document_text_continues_but_finished_text_refines() {
        assert_eq!(
            detect(&typed("The rollout begins with"), SurfaceKind::Document),
            Intent::Continue
        );
        assert_eq!(
            detect(&typed("The rollout begins Monday."), SurfaceKind::Document),
            Intent::Refine
        );
    }

    #[test]
    fn a_caret_in_the_middle_never_continues() {
        let mut f = typed("The rollout begins with");
        f.after = " and then the rest".into();
        assert_eq!(detect(&f, SurfaceKind::Document), Intent::Refine);
    }

    #[test]
    fn code_continues_even_after_a_finished_line() {
        assert_eq!(
            detect(&typed("let x = 1;"), SurfaceKind::CodeEditor),
            Intent::Continue
        );
    }

    #[test]
    fn terminal_text_is_refined() {
        assert_eq!(
            detect(&typed("list files by size"), SurfaceKind::Terminal),
            Intent::Refine
        );
    }

    #[test]
    fn every_surface_and_state_yields_an_intent() {
        let states = [typed(""), typed("hello"), with_thread(typed(""))];
        for kind in SurfaceKind::ALL {
            for s in &states {
                let _ = detect(s, kind);
            }
        }
    }
}
