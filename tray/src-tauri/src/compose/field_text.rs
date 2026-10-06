//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! What a text field reports, cleaned into what the user actually typed.
//!
//! Three readings that apps get wrong in the same few ways, kept together and tested without
//! a live app:
//! - [`visible_value`]: an empty rich-text box reports its placeholder ("Add a comment...") as
//!   its value; that is not the user's text.
//! - [`link_titles`]: a tag the app pre-fills (a mention) is exposed as a link inside the field.
//! - [`looks_like_quoted_reply`]: a reply box whose draft is empty but whose body already holds
//!   the quoted message being answered.
//!
//! # Who calls this
//! [`super::reader`] when a field is read, and [`super::dots`] and [`super::writer`] whenever
//! they re-read it, so every reader of a field sees the same text.

use super::ax::Element;

/// Strip an ellipsis, blanks and case so "Add a comment..." and "Add a comment\u{2026}" compare equal.
fn placeholder_key(s: &str) -> String {
    s.trim()
        .trim_end_matches(|c: char| c == '.' || c == '\u{2026}' || c.is_whitespace())
        .to_lowercase()
}

/// The text the user typed, given what the field reports as its value.
///
/// An empty rich-text box (a LinkedIn comment, a Gmail body) often reports its placeholder
/// ("Add a comment...") as its value. Taken at face value that reads as a draft the user wrote,
/// which turns "reply" into "polish this" and makes the box look changed the moment anything is
/// typed. A value equal to one of the field's own hint texts is therefore the empty string.
/// Only an exact match counts, so real text that merely starts the same way is kept.
pub(super) fn without_placeholder(value: String, hints: &[Option<String>]) -> String {
    let key = placeholder_key(&value);
    let is_hint = !key.is_empty() && hints.iter().flatten().any(|h| placeholder_key(h) == key);
    if is_hint {
        String::new()
    } else {
        value
    }
}

/// What the user can see typed in the box: the field's value with a placeholder removed.
/// `None` when the field reports no value at all.
pub(super) fn visible_value(element: &Element) -> Option<String> {
    let raw = element.string("AXValue")?;
    let hints = [
        element.string("AXPlaceholderValue"),
        element.string("AXDescription"),
        element.string("AXTitle"),
    ];
    Some(without_placeholder(raw, &hints))
}

/// Titles of the links inside a text field. A tag the app pre-fills (a mention) is exposed as a
/// link, so these are what the leading tag is matched against. Bounded: a field has a handful
/// of children, and a runaway tree must not slow a key press.
pub(super) fn link_titles(field: &Element) -> Vec<String> {
    const MAX_NODES: usize = 40;
    const MAX_DEPTH: usize = 3;
    let mut out = Vec::new();
    let mut seen = 0usize;
    let mut stack: Vec<(Element, usize)> = field
        .elements("AXChildren")
        .into_iter()
        .map(|e| (e, 1))
        .collect();
    while let Some((el, depth)) = stack.pop() {
        seen += 1;
        if seen > MAX_NODES {
            break;
        }
        let kids = el.elements("AXChildren");
        if el.string("AXRole").as_deref() == Some("AXLink") {
            let title = el
                .string("AXTitle")
                .or_else(|| el.string("AXDescription"))
                .or_else(|| kids.iter().find_map(|k| k.string("AXValue")));
            if let Some(t) = title
                .map(|t| t.trim().to_string())
                .filter(|t| !t.is_empty())
            {
                out.push(t);
            }
        } else if depth < MAX_DEPTH {
            stack.extend(kids.into_iter().map(|k| (k, depth + 1)));
        }
    }
    out
}

/// True when `text` (what follows an empty draft) is the quoted message being replied to, as
/// opposed to the user's own text or a signature: an attribution line ("On Mon ... wrote:"), a
/// forwarded-message rule, or lines quoted with ">".
pub(super) fn looks_like_quoted_reply(text: &str) -> bool {
    text.lines().take(12).map(str::trim).any(|line| {
        let lower = line.to_lowercase();
        lower.ends_with("wrote:")
            || (lower.starts_with("on ") && lower.contains(" wrote"))
            || lower.starts_with("-----original message")
            || lower.starts_with("begin forwarded message")
            || line.starts_with('>')
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_value_equal_to_the_placeholder_is_an_empty_box() {
        let hints = [Some("Add a comment\u{2026}".to_string()), None];
        assert_eq!(without_placeholder("Add a comment...".into(), &hints), "");
        assert_eq!(
            without_placeholder("  add a comment\u{2026} ".into(), &hints),
            ""
        );
    }

    #[test]
    fn real_text_is_kept_even_if_it_starts_like_the_placeholder() {
        let hints = [Some("Add a comment".to_string())];
        assert_eq!(
            without_placeholder("Add a comment about the launch".into(), &hints),
            "Add a comment about the launch"
        );
    }

    #[test]
    fn a_box_with_no_hint_text_keeps_its_value() {
        assert_eq!(without_placeholder("hello".into(), &[None, None]), "hello");
        assert_eq!(without_placeholder("".into(), &[Some("".into())]), "");
    }

    #[test]
    fn an_attribution_line_marks_a_quoted_reply() {
        assert!(looks_like_quoted_reply(
            "\n\nOn Mon, 5 Oct 2026 at 11:06, Stuti Rastogi wrote:\n> Hi, thank you"
        ));
        assert!(looks_like_quoted_reply("Stuti wrote:\nCould we do 3:30?"));
        assert!(looks_like_quoted_reply("> quoted line\n> another"));
        assert!(looks_like_quoted_reply(
            "-----Original Message-----\nFrom: A"
        ));
    }

    #[test]
    fn a_signature_or_plain_text_is_not_a_quoted_reply() {
        assert!(!looks_like_quoted_reply("\n\nBest,\nAdithya"));
        assert!(!looks_like_quoted_reply("--\nAdithya Harish\nFounder"));
        assert!(!looks_like_quoted_reply(""));
    }
}
