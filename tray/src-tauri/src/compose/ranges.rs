//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Splitting and splicing text by UTF-16 offsets, the unit Accessibility speaks.
//!
//! macOS reports a text field's selection as a location and length counted in UTF-16 code
//! units. Rust strings are UTF-8, and a single emoji is two UTF-16 units but four bytes,
//! so slicing by those numbers directly would cut characters in half or panic. Every
//! conversion goes through here, returns `None` rather than guessing when a range does not
//! fit the text or lands inside a surrogate pair, and is tested against emoji.
//!
//! # Who calls this
//! [`super::ax`] when it turns a field's value and selected range into the
//! before / selection / after parts the compose pipeline wants.
//!
//! # Related
//! - [`meridian::compose::types::FieldSnapshot`] - the consumer of the three parts.

/// A range in UTF-16 code units, as Accessibility reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Utf16Range {
    pub location: usize,
    pub length: usize,
}

/// Length of `text` in UTF-16 code units.
pub fn utf16_len(text: &str) -> usize {
    text.chars().map(char::len_utf16).sum()
}

/// Byte offset of the character that starts at UTF-16 offset `units`, or `None` when
/// `units` is past the end or falls between the halves of a surrogate pair.
fn byte_offset(text: &str, units: usize) -> Option<usize> {
    let mut seen = 0;
    for (byte, ch) in text.char_indices() {
        if seen == units {
            return Some(byte);
        }
        if seen > units {
            return None;
        }
        seen += ch.len_utf16();
    }
    (seen == units).then_some(text.len())
}

/// Split `text` into the part before `range`, the part inside it, and the part after.
pub fn split(text: &str, range: Utf16Range) -> Option<(String, String, String)> {
    let start = byte_offset(text, range.location)?;
    let end = byte_offset(text, range.location.checked_add(range.length)?)?;
    Some((
        text[..start].to_string(),
        text[start..end].to_string(),
        text[end..].to_string(),
    ))
}

/// Replace `range` in `text` with `replacement`.
pub fn splice(text: &str, range: Utf16Range, replacement: &str) -> Option<String> {
    let (before, _, after) = split(text, range)?;
    Some(format!("{before}{replacement}{after}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(location: usize, length: usize) -> Utf16Range {
        Utf16Range { location, length }
    }

    #[test]
    fn splits_ascii() {
        assert_eq!(
            split("Hello world", r(6, 5)),
            Some(("Hello ".into(), "world".into(), String::new()))
        );
        assert_eq!(
            split("Hello", r(2, 0)),
            Some(("He".into(), String::new(), "llo".into()))
        );
    }

    #[test]
    fn counts_emoji_as_two_units() {
        assert_eq!(utf16_len("a\u{1F600}b"), 4);
        // "a" = 1 unit, emoji = 2 units, so the selection of "b" starts at offset 3.
        assert_eq!(
            split("a\u{1F600}b", r(3, 1)),
            Some(("a\u{1F600}".into(), "b".into(), String::new()))
        );
    }

    #[test]
    fn selecting_an_emoji_takes_two_units() {
        assert_eq!(
            split("a\u{1F600}b", r(1, 2)),
            Some(("a".into(), "\u{1F600}".into(), "b".into()))
        );
    }

    #[test]
    fn a_range_that_cuts_a_surrogate_pair_is_rejected() {
        assert_eq!(split("a\u{1F600}b", r(2, 1)), None);
        assert_eq!(split("a\u{1F600}b", r(1, 1)), None);
    }

    #[test]
    fn a_range_past_the_end_is_rejected() {
        assert_eq!(split("abc", r(2, 5)), None);
        assert_eq!(split("abc", r(4, 0)), None);
        assert_eq!(split("abc", r(usize::MAX, 2)), None);
    }

    #[test]
    fn the_end_of_the_text_is_a_valid_caret_position() {
        assert_eq!(
            split("abc", r(3, 0)),
            Some(("abc".into(), String::new(), String::new()))
        );
    }

    #[test]
    fn empty_text_has_one_valid_position() {
        assert_eq!(
            split("", r(0, 0)),
            Some((String::new(), String::new(), String::new()))
        );
        assert_eq!(split("", r(1, 0)), None);
    }

    #[test]
    fn multibyte_non_emoji_text_is_counted_by_utf16_units() {
        // "é" and "\u{4e2d}" are one UTF-16 unit each, though two and three bytes in UTF-8.
        assert_eq!(utf16_len("\u{e9}\u{4e2d}"), 2);
        assert_eq!(
            split("\u{e9}\u{4e2d}x", r(1, 1)),
            Some(("\u{e9}".into(), "\u{4e2d}".into(), "x".into()))
        );
    }

    #[test]
    fn splice_replaces_exactly_the_range() {
        assert_eq!(
            splice("Hello world", r(6, 5), "there"),
            Some("Hello there".into())
        );
        assert_eq!(splice("ab", r(1, 0), "XY"), Some("aXYb".into()));
        assert_eq!(splice("ab", r(5, 0), "XY"), None);
    }

    #[test]
    fn split_then_rejoin_is_the_identity() {
        let text = "caf\u{e9} \u{1F600} done";
        for loc in 0..=utf16_len(text) {
            if let Some((b, s, a)) = split(text, r(loc, 0)) {
                assert_eq!(format!("{b}{s}{a}"), text);
            }
        }
    }
}
