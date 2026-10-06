//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Turning the model's raw answer into text that is safe to put in someone's text box.
//!
//! The prompt asks for the text and nothing else, but models still sometimes wrap it:
//! a markdown fence, a pair of quotes, "Here's a draft:". Whatever reaches the user's
//! field is typed into their real message, so this layer strips those wrappers
//! conservatively and refuses to insert anything it cannot trust.
//!
//! # Outcomes
//! - [`Parsed::Text`] - clean text to deliver.
//! - [`Parsed::NoContext`] - the model returned the reserved sentinel: not enough to go
//!   on. The app shows a notice instead of inserting prose.
//! - [`Parsed::Unusable`] - empty, or the model answered with its own commentary instead
//!   of text for the box. Never inserted.
//!
//! # Who calls this
//! The compose pipeline, after the LLM call.
//!
//! # Related
//! - [`crate::compose::prompt`] - defines the contract this enforces.

/// The reserved token the model returns when it has nothing trustworthy to write.
pub const NO_CONTEXT: &str = "[[NO_CONTEXT]]";

/// What a model answer turned into.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Parsed {
    Text(String),
    NoContext,
    Unusable,
}

/// Openers that mean the model is talking to the user instead of writing for the box.
/// Matched only on a first line that ends in a colon, so a message that happens to start
/// with "Here" is never touched.
const PREAMBLE_STARTS: &[&str] = &[
    "here's",
    "here is",
    "here are",
    "sure",
    "certainly",
    "of course",
    "below is",
    "draft:",
    "rewritten",
    "revised",
];

fn strip_fence(s: &str) -> String {
    let t = s.trim();
    if let Some(rest) = t.strip_prefix("```") {
        // Drop the optional language tag on the opening line.
        let body = rest.split_once('\n').map_or("", |(_, b)| b);
        if let Some(inner) = body.trim_end().strip_suffix("```") {
            return inner.trim_matches('\n').to_string();
        }
    }
    t.to_string()
}

fn strip_wrapping_quotes(s: &str) -> String {
    let t = s.trim();
    let pairs = [
        ('"', '"'),
        ('\u{201c}', '\u{201d}'),
        ('\'', '\''),
        ('`', '`'),
    ];
    for (open, close) in pairs {
        if t.chars().count() >= 2 && t.starts_with(open) && t.ends_with(close) {
            let inner = &t[open.len_utf8()..t.len() - close.len_utf8()];
            // Only unwrap when the quote does not recur inside, so a message that merely
            // starts and ends with its own quotation survives.
            if !inner.contains(open) && !inner.contains(close) {
                return inner.trim().to_string();
            }
        }
    }
    t.to_string()
}

fn strip_preamble(s: &str) -> String {
    let Some((first, rest)) = s.split_once('\n') else {
        return s.to_string();
    };
    let lower = first.trim().to_ascii_lowercase();
    if first.trim_end().ends_with(':') && PREAMBLE_STARTS.iter().any(|p| lower.starts_with(p)) {
        return rest.trim_start_matches('\n').to_string();
    }
    s.to_string()
}

/// Text the prompt builder inserts into the box for the model's benefit.
const SCAFFOLDING: [&str; 3] = [
    crate::compose::prompt::CURSOR_MARKER,
    crate::compose::prompt::OMITTED_BEFORE,
    crate::compose::prompt::OMITTED_AFTER,
];

/// The line the prompt asks the model to open with, naming who wrote the last message. It
/// only exists to make the model decide that before it writes, so it is never inserted.
const LEAD_LINE: &str = "last message from:";

fn strip_lead_line(s: &str) -> &str {
    let t = s.trim_start();
    let first = t.lines().next().unwrap_or("");
    // Tolerate markup around the label (`**Last message from:** user`, a leading bracket).
    let plain: String = first
        .trim()
        .trim_start_matches(|c: char| !c.is_alphanumeric())
        .to_ascii_lowercase();
    if plain.starts_with(LEAD_LINE) && first.len() <= 80 {
        return t[first.len()..].trim_start_matches(['\r', '\n']);
    }
    t
}

/// Parse a model answer. `single_line` is true for fields that must hold one line (a
/// terminal, a structured field): extra lines are dropped rather than inserted.
pub fn parse(raw: &str, single_line: bool) -> Parsed {
    // An answer wrapped in a fence may carry the lead line inside it, so look past the fence.
    let unfenced = strip_fence(raw);
    let trimmed = strip_lead_line(&unfenced).trim();
    if trimmed.is_empty() {
        return Parsed::Unusable;
    }
    // The prompt's own scaffolding is never the user's text: an answer that echoes the box with
    // its cursor marker, or an "omitted" notice, would put it into their message.
    if SCAFFOLDING.iter().any(|m| trimmed.contains(m)) {
        return Parsed::Unusable;
    }
    if trimmed == NO_CONTEXT
        || (trimmed.contains(NO_CONTEXT) && trimmed.chars().count() <= NO_CONTEXT.len() + 8)
    {
        return Parsed::NoContext;
    }
    // Order matters: a preamble line comes first, and what follows it may itself be
    // fenced or quoted, so peel from the outside in.
    let mut text = strip_preamble(trimmed);
    text = strip_fence(&text);
    text = strip_wrapping_quotes(&text);
    if single_line {
        text = text
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty())
            .unwrap_or("")
            .to_string();
    }
    let text = text.trim().to_string();
    if text.is_empty() {
        Parsed::Unusable
    } else {
        Parsed::Text(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(raw: &str) -> String {
        match parse(raw, false) {
            Parsed::Text(t) => t,
            other => panic!("expected Text, got {other:?}"),
        }
    }

    #[test]
    fn the_who_wrote_last_line_is_removed() {
        assert_eq!(
            text("Last message from: user\nHey, any update on this?"),
            "Hey, any update on this?"
        );
        assert_eq!(
            text("last message from: other\n\nSounds good."),
            "Sounds good."
        );
    }

    #[test]
    fn a_marked_up_lead_line_is_removed_too() {
        assert_eq!(
            text("**Last message from:** user\nSee you then."),
            "See you then."
        );
        assert_eq!(
            text("[Last message from: other]\nSee you then."),
            "See you then."
        );
    }

    #[test]
    fn a_lead_line_inside_a_fence_is_removed_too() {
        assert_eq!(
            text("```\nLast message from: user\nHello there\n```"),
            "Hello there"
        );
    }

    #[test]
    fn the_sentinel_still_counts_after_the_lead_line() {
        assert_eq!(
            parse("Last message from: none\n[[NO_CONTEXT]]", false),
            Parsed::NoContext
        );
    }

    #[test]
    fn a_lead_line_with_nothing_after_it_is_unusable() {
        assert_eq!(parse("Last message from: user", false), Parsed::Unusable);
    }

    #[test]
    fn plain_text_passes_through_trimmed() {
        assert_eq!(text("  Thursday works for me.\n"), "Thursday works for me.");
    }

    #[test]
    fn markdown_fences_are_removed() {
        assert_eq!(text("```\nThursday works.\n```"), "Thursday works.");
        assert_eq!(text("```text\nThursday works.\n```"), "Thursday works.");
    }

    #[test]
    fn wrapping_quotes_are_removed_but_inner_quotes_survive() {
        assert_eq!(text("\"Thursday works.\""), "Thursday works.");
        assert_eq!(text("\u{201c}Thursday works.\u{201d}"), "Thursday works.");
        assert_eq!(
            text("\"He said \"yes\" to it\""),
            "\"He said \"yes\" to it\""
        );
    }

    #[test]
    fn a_colon_terminated_preamble_line_is_dropped() {
        assert_eq!(
            text("Here's a draft:\n\nThursday works."),
            "Thursday works."
        );
        assert_eq!(
            text("Sure, here is the reply:\nThursday works."),
            "Thursday works."
        );
    }

    #[test]
    fn a_message_that_starts_with_here_is_kept() {
        assert_eq!(
            text("Here is the deck.\nLet me know."),
            "Here is the deck.\nLet me know."
        );
    }

    #[test]
    fn the_sentinel_means_no_context() {
        assert_eq!(parse("[[NO_CONTEXT]]", false), Parsed::NoContext);
        assert_eq!(parse("  [[NO_CONTEXT]]\n", false), Parsed::NoContext);
    }

    #[test]
    fn a_long_answer_that_mentions_the_sentinel_is_not_treated_as_one() {
        let long = format!("Please see the note about {NO_CONTEXT} in the docs, it explains the empty case in detail.");
        assert!(matches!(parse(&long, false), Parsed::Text(_)));
    }

    #[test]
    fn empty_answers_are_unusable() {
        assert_eq!(parse("", false), Parsed::Unusable);
        assert_eq!(parse("  \n ", false), Parsed::Unusable);
        assert_eq!(parse("```\n```", false), Parsed::Unusable);
    }

    #[test]
    fn single_line_fields_take_only_the_first_line() {
        assert_eq!(
            parse("ls -la\nThis lists files", true),
            Parsed::Text("ls -la".into())
        );
    }

    #[test]
    fn multi_paragraph_text_keeps_its_structure() {
        assert_eq!(
            text("Hi Sara,\n\nThanks for the intro.\n\nBest,\nA"),
            "Hi Sara,\n\nThanks for the intro.\n\nBest,\nA"
        );
    }
    #[test]
    fn a_preamble_followed_by_a_fenced_block_is_fully_unwrapped() {
        assert_eq!(
            text("Here's a draft:\n```\nThursday works.\n```"),
            "Thursday works."
        );
        assert_eq!(
            text("Sure, here is the reply:\n\"Thursday works.\""),
            "Thursday works."
        );
    }

    #[test]
    fn an_answer_that_echoes_the_cursor_marker_is_unusable() {
        let raw = format!(
            "Sounds good{} thanks",
            crate::compose::prompt::CURSOR_MARKER
        );
        assert_eq!(parse(&raw, false), Parsed::Unusable);
    }

    #[test]
    fn an_answer_that_echoes_an_omitted_notice_is_unusable() {
        assert_eq!(
            parse("[earlier text omitted]\nSure, Thursday works.", false),
            Parsed::Unusable
        );
        assert_eq!(
            parse("Sure.\n[later text omitted]", false),
            Parsed::Unusable
        );
    }
}
