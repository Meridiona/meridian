//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Finding the part of a box that the app filled in, not the user: a mention chip.
//!
//! # Why this exists
//! Replying to a comment on LinkedIn (and in Slack, Teams, GitHub, Notion...) opens a box that
//! already holds a tag of the person being answered. Read naively, that tag is "a draft the
//! user wrote", so the model polishes it and the writer replaces the whole box, which turns the
//! tag into plain text. A tag is structure the user did not type and must keep.
//!
//! # How it is found
//! 1. **From the accessibility tree.** A tag is exposed as a link inside the text field. If
//!    the box starts with the title of such a link, that is the tag.
//! 2. **From the shape of the text**, when the tree does not expose it. A short, name-like
//!    value that appears on its own line in the text around the box (the comment's author) is
//!    the tag. Deliberately narrow: a sentence, digits or a name that appears nowhere nearby
//!    is treated as the user's own text.
//!
//! The result is the length of the protected prefix in UTF-16 units, including the blank space
//! that follows the tag. Everything after it is the user's.
//!
//! # Who calls this
//! [`super::reader`], when a field is read.
//!
//! # Related
//! - [`super::delivery::plan_edit`] - never edits inside the protected prefix.
//! - [`super::writer`] - never replaces the whole field while a prefix is protected.

use super::ranges::utf16_len;

/// Longest value still plausibly a person's name.
const MAX_NAME_CHARS: usize = 60;
const MAX_NAME_WORDS: usize = 5;

/// Blank in the sense editors use: ordinary space plus the invisible characters browsers
/// insert around a tag.
fn is_blank(c: char) -> bool {
    c.is_whitespace() || matches!(c, '\u{200b}' | '\u{200c}' | '\u{200d}' | '\u{feff}')
}

fn looks_like_name(text: &str) -> bool {
    if text.chars().count() > MAX_NAME_CHARS {
        return false;
    }
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.is_empty() || words.len() > MAX_NAME_WORDS {
        return false;
    }
    words.iter().all(|w| {
        w.chars().next().is_some_and(char::is_uppercase)
            && w.chars()
                .all(|c| c.is_alphabetic() || matches!(c, '-' | '\'' | '\u{2019}' | '.'))
    })
}

/// True when a line of the surrounding text is the name, or begins with it followed by a break
/// (a comment header reads "Name  .  2nd" or "Name  Title at Company", not just "Name").
fn appears_on_its_own_line(context: &str, name: &str) -> bool {
    let name = name.to_lowercase();
    context.lines().any(|l| {
        let line = l.trim_matches(is_blank).to_lowercase();
        line.strip_prefix(name.as_str())
            .is_some_and(|rest| rest.chars().next().is_none_or(|c| !c.is_alphanumeric()))
    })
}

/// Why the shape-based match did or did not fire, as three yes/no answers (for diagnostics):
/// the value looks like a name, the context holds it anywhere, the context holds it as a line.
pub fn name_checks(value: &str, context: &str) -> (bool, bool, bool) {
    let t = value.trim_matches(is_blank);
    (
        looks_like_name(t),
        !t.is_empty() && context.to_lowercase().contains(&t.to_lowercase()),
        !t.is_empty() && appears_on_its_own_line(context, t),
    )
}

/// Length in UTF-16 units of the leading part of `value` that the user did not write, or 0.
///
/// `link_titles` are the titles of link elements found inside the field; `context` is the
/// text around the box.
pub fn protected_prefix(value: &str, link_titles: &[String], context: &str) -> usize {
    let start = value.len() - value.trim_start_matches(is_blank).len();
    let rest = &value[start..];

    for title in link_titles {
        let title = title.trim_matches(is_blank);
        // The tag must end at a word boundary: a link "Ann" is not the start of "Annabelle".
        let ends_the_word = |after: &str| after.chars().next().is_none_or(|c| !c.is_alphanumeric());
        if !title.is_empty() && rest.starts_with(title) && ends_the_word(&rest[title.len()..]) {
            return through_blanks(value, start + title.len());
        }
    }

    let trimmed = value.trim_matches(is_blank);
    if !trimmed.is_empty() && looks_like_name(trimmed) && appears_on_its_own_line(context, trimmed)
    {
        return utf16_len(value);
    }
    0
}

/// UTF-16 length of `value` up to `end` (a byte offset), extended over the blanks after it.
fn through_blanks(value: &str, end: usize) -> usize {
    let tail = &value[end..];
    let blanks = tail.len() - tail.trim_start_matches(is_blank).len();
    utf16_len(&value[..end + blanks])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn titles(t: &[&str]) -> Vec<String> {
        t.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_link_at_the_start_is_the_tag_and_the_blank_after_it_is_included() {
        let v = "Jurgen P\u{eb}rgega\u{a0}";
        assert_eq!(
            protected_prefix(v, &titles(&["Jurgen P\u{eb}rgega"]), ""),
            utf16_len(v)
        );
    }

    #[test]
    fn text_typed_after_a_tag_is_not_protected() {
        let v = "Jurgen P\u{eb}rgega thanks for sharing";
        let n = protected_prefix(v, &titles(&["Jurgen P\u{eb}rgega"]), "");
        assert_eq!(n, utf16_len("Jurgen P\u{eb}rgega "));
        assert!(n < utf16_len(v));
    }

    #[test]
    fn a_lone_name_seen_nearby_is_the_tag_when_the_tree_hides_it() {
        let ctx = "View Jurgen P\u{eb}rgega's profile\nJurgen P\u{eb}rgega\nAuthor";
        let v = "Jurgen P\u{eb}rgega\u{200b}";
        assert_eq!(protected_prefix(v, &[], ctx), utf16_len(v));
    }

    #[test]
    fn a_name_at_the_start_of_a_comment_header_line_is_the_tag() {
        let ctx = "Hyunji J.  \u{2022} 2nd\nStaff Engineer\nGlad it landed.";
        assert_eq!(
            protected_prefix("Hyunji J. ", &[], ctx),
            utf16_len("Hyunji J. ")
        );
    }

    #[test]
    fn a_name_that_only_starts_a_longer_word_is_not_a_header_match() {
        assert_eq!(protected_prefix("Ann", &[], "Annabelle writes"), 0);
    }

    #[test]
    fn a_name_that_appears_nowhere_nearby_is_the_users_text() {
        assert_eq!(
            protected_prefix("Jurgen P\u{eb}rgega", &[], "Author\nSomeone"),
            0
        );
    }

    #[test]
    fn a_sentence_is_never_a_tag() {
        let ctx = "Thanks for sharing.";
        assert_eq!(protected_prefix("Thanks for sharing.", &[], ctx), 0);
        assert_eq!(protected_prefix("Call me at 5", &[], "Call me at 5"), 0);
    }

    #[test]
    fn an_empty_or_blank_box_has_nothing_protected() {
        assert_eq!(protected_prefix("", &titles(&["Ann"]), "Ann"), 0);
        assert_eq!(protected_prefix("  \u{200b}", &[], "Ann"), 0);
    }

    #[test]
    fn a_link_that_is_not_at_the_start_is_not_a_tag() {
        assert_eq!(
            protected_prefix("see Ann Lee for details", &titles(&["Ann Lee"]), ""),
            0
        );
    }

    #[test]
    fn a_link_that_is_only_the_start_of_a_longer_word_is_not_a_tag() {
        assert_eq!(
            protected_prefix("Annabelle thanks", &titles(&["Ann"]), ""),
            0
        );
        assert_eq!(
            protected_prefix("Ann, thanks", &titles(&["Ann"]), ""),
            utf16_len("Ann")
        );
    }
}
