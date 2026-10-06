//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Assembling the model request for one press.
//!
//! # Layout
//! The **system** half is the single static core prompt, identical on every press, so a
//! provider that caches a stable head can reuse it. Everything that varies - what kind of
//! box, what the user wants, the page, the text around the box, the box itself - goes in
//! the **user** half as labelled sections, in the order below. A section with nothing to
//! say is left out rather than shown empty, so the model never reads "Text below the box:
//! (none)" as a fact.
//!
//! 1. Box (surface block)  2. Task (intent block)  3. Page  4. Compose header
//! 5. Text above the box   6. The box itself       7. Text below the box
//! 8. Read-only content below the draft  9. Previous attempt
//!
//! # Prompt versioning
//! [`PROMPT_VERSION`] is stored with every draft. Change it whenever any file under
//! `assets/prompts/compose/` or this layout changes, so quality can be compared across
//! versions. A golden-snapshot test makes any such change a visible diff.
//!
//! # Who calls this
//! The compose pipeline, after [`crate::compose::classify::classify`].
//!
//! # Related
//! - [`super::budget`] - the trimming each section goes through.
//! - [`crate::compose::output`] - parses what the model sends back.

use std::fmt::Write as _;

use super::budget::{clip, dedupe_nearby, keep_head, keep_tail, tidy};
use crate::compose::classify::Plan;
use crate::compose::types::{DraftRequest, Intent, SurfaceKind};

/// Bump on any change to the prompt files or the layout. Stored with every draft.
pub const PROMPT_VERSION: &str = "compose-v5";

/// The static system prompt shared by every press.
pub const CORE: &str = include_str!("../../../assets/prompts/compose/core.md");

/// Character budgets per section. Chosen so a worst-case press stays near 5k tokens,
/// the size measured at 4 to 5 seconds on a warm CLI call.
pub const ABOVE_CHARS: usize = 8_000;
pub const BELOW_CHARS: usize = 1_200;
pub const FIELD_BEFORE_CHARS: usize = 3_000;
pub const FIELD_AFTER_CHARS: usize = 1_000;
pub const SELECTION_CHARS: usize = 3_000;
pub const BELOW_DRAFT_CHARS: usize = 1_500;
/// The rest of the focused window.
pub const REST_OF_WINDOW_CHARS: usize = 2_500;
pub const PREVIOUS_CHARS: usize = 2_000;
pub const HEADER_VALUE_CHARS: usize = 200;
pub const TITLE_CHARS: usize = 200;
/// Characters allowed around the header values in a line that still counts as an echo of the
/// header (the labels "To", "Subject", "Cc", "Bcc" and spacing).
const HEADER_ECHO_SLACK: usize = 40;

/// The marker shown where the caret sits inside the box text.
pub const CURSOR_MARKER: &str = "\u{25b6} CURSOR \u{25c0}";

fn surface_block(kind: SurfaceKind) -> &'static str {
    match kind {
        SurfaceKind::EmailCompose => {
            include_str!("../../../assets/prompts/compose/surface-email_compose.md")
        }
        SurfaceKind::DirectChat => {
            include_str!("../../../assets/prompts/compose/surface-direct_chat.md")
        }
        SurfaceKind::PublicComment => {
            include_str!("../../../assets/prompts/compose/surface-public_comment.md")
        }
        SurfaceKind::Document => {
            include_str!("../../../assets/prompts/compose/surface-document.md")
        }
        SurfaceKind::CodeEditor => {
            include_str!("../../../assets/prompts/compose/surface-code_editor.md")
        }
        SurfaceKind::Terminal => {
            include_str!("../../../assets/prompts/compose/surface-terminal.md")
        }
        SurfaceKind::AiPrompt => {
            include_str!("../../../assets/prompts/compose/surface-ai_prompt.md")
        }
        SurfaceKind::StructuredField => {
            include_str!("../../../assets/prompts/compose/surface-structured_field.md")
        }
        SurfaceKind::Generic => include_str!("../../../assets/prompts/compose/surface-generic.md"),
    }
}

fn intent_block(intent: Intent) -> &'static str {
    match intent {
        Intent::StartConversation => {
            include_str!("../../../assets/prompts/compose/intent-start_conversation.md")
        }
        Intent::Reply => include_str!("../../../assets/prompts/compose/intent-reply.md"),
        Intent::Continue => include_str!("../../../assets/prompts/compose/intent-continue.md"),
        Intent::Refine => include_str!("../../../assets/prompts/compose/intent-refine.md"),
        Intent::RewriteSelection => {
            include_str!("../../../assets/prompts/compose/intent-rewrite_selection.md")
        }
        Intent::FillField => include_str!("../../../assets/prompts/compose/intent-fill_field.md"),
    }
}

/// Sizes recorded on the prompt span. Counts only, never text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PromptStats {
    pub user_chars: usize,
    pub above_chars: usize,
    pub below_chars: usize,
    pub field_chars: usize,
    pub rest_of_window_chars: usize,
}

/// A request ready for the LLM layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuiltPrompt {
    pub system: &'static str,
    pub user: String,
    pub version: &'static str,
    pub stats: PromptStats,
}

fn section(out: &mut String, title: &str, body: &str) {
    if body.trim().is_empty() {
        return;
    }
    let _ = write!(out, "## {title}\n{}\n\n", body.trim_end());
}

/// Drop lines of surrounding text that only restate a compose header value ("To Nabeel
/// Subject Intro call Cc Bcc"). The header already has its own section, so the echo
/// spends budget and can be mistaken for a conversation.
fn drop_header_echo(text: &str, req: &DraftRequest) -> String {
    let values: Vec<String> = req
        .field
        .header
        .iter()
        .map(|h| h.value.trim().to_lowercase())
        .filter(|v| v.chars().count() >= 3)
        .collect();
    if values.is_empty() {
        return text.to_string();
    }
    // An echo restates the header: it carries every header value and little else. A
    // conversation line that merely mentions the recipient, or a word of the subject, stays.
    let echo_limit = values.iter().map(|v| v.chars().count()).sum::<usize>() + HEADER_ECHO_SLACK;
    text.lines()
        .filter(|line| {
            let l = line.to_lowercase();
            let is_echo =
                l.chars().count() <= echo_limit && values.iter().all(|v| l.contains(v.as_str()));
            !is_echo
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Whether text below the box can help. Below a chat, mail or prompt box there is only page
/// furniture (ads, other inboxes, footers); below a document or editor box it is the rest of
/// the document, which a continuation needs.
fn below_is_useful(surface: SurfaceKind) -> bool {
    matches!(
        surface,
        SurfaceKind::Document | SurfaceKind::CodeEditor | SurfaceKind::Generic
    )
}

/// Chrome appends its memory-saver note to the title of heavy tabs ("- High memory usage - 923
/// MB"). It says nothing about the page and costs tokens on every press.
fn clean_title(title: &str) -> String {
    const NOTE: &str = " - High memory usage";
    let title = title.trim();
    let Some(at) = title.find(NOTE) else {
        return title.to_string();
    };
    let (head, tail) = (&title[..at], &title[at + NOTE.len()..]);
    // The note is followed by a size ("- 923 MB"); anything after that is the browser and
    // profile name and is kept.
    let after = tail
        .strip_prefix(" - ")
        .and_then(|t| t.split_once(" - "))
        .map(|(_size, rest)| rest.trim())
        .filter(|rest| !rest.is_empty());
    match after {
        Some(rest) => format!("{head} - {rest}"),
        None => head.to_string(),
    }
}

/// Host plus the first path segment. Deeper segments are usually opaque ids (a thread id, a
/// document id) that mean nothing to the model.
fn site_summary(page: &crate::compose::classify::Page) -> String {
    let first = page.path.trim_matches('/').split('/').next().unwrap_or("");
    if first.is_empty() {
        page.host.clone()
    } else {
        format!("{}/{}", page.host, first)
    }
}

fn page_lines(req: &DraftRequest, plan: &Plan) -> String {
    let f = &req.field;
    let mut s = String::new();
    if !f.app_name.trim().is_empty() {
        let _ = writeln!(s, "App: {}", clip(f.app_name.trim(), TITLE_CHARS));
    }
    if !f.window_title.trim().is_empty() {
        let _ = writeln!(
            s,
            "Window: {}",
            clip(&clean_title(&f.window_title), TITLE_CHARS)
        );
    }
    if !plan.page.host.is_empty() {
        let _ = writeln!(s, "Site: {}", site_summary(&plan.page));
    }
    if !f.label.trim().is_empty() {
        let _ = writeln!(s, "Field label: {}", clip(f.label.trim(), TITLE_CHARS));
    }
    if !f.placeholder.trim().is_empty() {
        let _ = writeln!(
            s,
            "Field placeholder: {}",
            clip(f.placeholder.trim(), TITLE_CHARS)
        );
    }
    s
}

fn header_lines(req: &DraftRequest) -> String {
    req.field
        .header
        .iter()
        .filter(|h| !h.label.trim().is_empty() && !h.value.trim().is_empty())
        .map(|h| {
            format!(
                "{}: {}\n",
                clip(h.label.trim(), 60),
                clip(h.value.trim(), HEADER_VALUE_CHARS)
            )
        })
        .collect()
}

/// The box itself, in the shape its intent needs.
fn box_section(out: &mut String, req: &DraftRequest, plan: &Plan) -> usize {
    let f = &req.field;
    let before = keep_tail(&f.before, FIELD_BEFORE_CHARS);
    let after = keep_head(&f.after, FIELD_AFTER_CHARS);
    let mut used = 0;
    if plan.intent == Intent::RewriteSelection {
        let selected = keep_head(&f.selection, SELECTION_CHARS);
        used = before.len() + selected.len() + after.len();
        section(
            out,
            "Text before the highlight (stays in the box, do not output)",
            &before,
        );
        section(
            out,
            "Highlighted text (your output replaces only this)",
            &selected,
        );
        section(
            out,
            "Text after the highlight (stays in the box, do not output)",
            &after,
        );
    } else if f.is_empty_field() {
        section(out, "The box", "(empty)");
    } else {
        let mut body = String::new();
        body.push_str(&before);
        body.push_str(CURSOR_MARKER);
        body.push_str(&after);
        used = body.len();
        section(out, "Text in the box (the cursor is marked)", &body);
    }
    used
}

/// Build the request for a classified press. Pure.
pub fn build(req: &DraftRequest, plan: &Plan) -> BuiltPrompt {
    let f = &req.field;
    let mut user = String::with_capacity(8 * 1024);

    section(&mut user, "Box", surface_block(plan.surface));
    section(&mut user, "Task", intent_block(plan.intent));
    if let Some(name) = req
        .user_name
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty())
    {
        section(&mut user, "The user", &format!("Name: {}", clip(name, 80)));
    }
    section(&mut user, "Page", &page_lines(req, plan));
    section(&mut user, "Compose header", &header_lines(req));
    section(
        &mut user,
        "Already in the box (stays in place, written before your text; do not repeat it)",
        f.kept_prefix.trim(),
    );

    let above = keep_tail(
        &dedupe_nearby(&tidy(&drop_header_echo(&f.nearby.above, req))),
        ABOVE_CHARS,
    );
    let above_title = if f.nearby.broad {
        "Text above the box (the whole window, noisier than usual; the conversation is in here)"
    } else {
        "Text above the box (closest to the box last)"
    };
    section(&mut user, above_title, &above);

    let field_chars = box_section(&mut user, req, plan);

    let below = if below_is_useful(plan.surface) {
        keep_head(&dedupe_nearby(&tidy(&f.nearby.below)), BELOW_CHARS)
    } else {
        String::new()
    };
    section(&mut user, "Text below the box (closest first)", &below);
    let rest = keep_head(
        &dedupe_nearby(&tidy(&f.nearby.rest_of_window)),
        REST_OF_WINDOW_CHARS,
    );
    section(
        &mut user,
        "Rest of this window (further from the box; use only if relevant)",
        &rest,
    );
    section(
        &mut user,
        "Read-only content below the draft (preserved by the app, never output)",
        &keep_head(&dedupe_nearby(&tidy(&f.below_draft)), BELOW_DRAFT_CHARS),
    );

    if let Some(prev) = &req.previous {
        section(
            &mut user,
            "Previous attempt (the user rejected it; write something clearly different)",
            &keep_head(&prev.output, PREVIOUS_CHARS),
        );
    }

    let stats = PromptStats {
        user_chars: user.chars().count(),
        above_chars: above.chars().count(),
        below_chars: below.chars().count(),
        field_chars,
        rest_of_window_chars: rest.chars().count(),
    };
    BuiltPrompt {
        system: CORE,
        user: user.trim_end().to_string(),
        version: PROMPT_VERSION,
        stats,
    }
}

#[cfg(test)]
#[path = "builder_tests.rs"]
mod tests;
