//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! The rules for putting a draft into someone else's text box, as pure decisions.
//!
//! Writing into another app is the riskiest thing this feature does, and the part with no
//! honest automated test (it is Accessibility IPC into a foreign process). So everything
//! that can be decided without that IPC is decided here and tested to death, and the
//! macOS layer is left with nothing to judge: it reads facts, asks these functions, and
//! obeys.
//!
//! # The three rules
//! 1. **Edit only what the intent allows.** [`plan_edit`] turns an intent into the exact
//!    range to replace. Text after the caret is never touched except by a short
//!    structured field, which is replaced whole.
//! 2. **Write only into the field the press came from, unchanged.** [`guard`] refuses when
//!    the user switched apps, the field went away, it is no longer the same field, or they
//!    kept typing while the model worked. A refused draft goes to the clipboard, never
//!    into a field it was not meant for.
//! 3. **Believe a write only when it is seen.** [`landed`] compares the value read back
//!    with the value we expected, because several apps answer "success" to a write they
//!    ignore and Chromium applies writes a moment late.
//!
//! # Who calls this
//! [`super::controller`], between drafting and writing.
//!
//! # Related
//! - [`super::ranges`] - the UTF-16 maths the planned range is expressed in.
//! - [`meridian::compose`] - produces the draft and the intent this plans around.

use meridian::compose::types::{Intent, SurfaceKind};

use super::ranges::{splice, Utf16Range};

/// A draft longer than this is not written in place; it goes to the clipboard instead. A
/// field this large is a document, and replacing text we only partly read is how data is lost.
pub const MAX_FIELD_UTF16: usize = 40_000;

/// The range of the field a draft replaces, in UTF-16 units.
///
/// `selection` is the range the user had selected when they pressed the key; its length is
/// zero for a bare caret. `field_len` is the field's whole length.
pub fn plan_edit(
    intent: Intent,
    selection: Utf16Range,
    field_len: usize,
    protected_prefix: usize,
) -> Utf16Range {
    // Text the app put there (a tag of the person being answered) is never edited. The prefix
    // can never reach past the end of the field.
    let floor = protected_prefix.min(field_len);
    let caret = selection.location.min(field_len).max(floor);
    match intent {
        // Only the highlight changes, and never the protected tag or past the end of the field.
        Intent::RewriteSelection => {
            let start = selection.location.clamp(floor, field_len);
            let end = selection
                .location
                .saturating_add(selection.length)
                .clamp(start, field_len);
            Utf16Range {
                location: start,
                length: end - start,
            }
        }
        // Insert at the caret; nothing existing is removed.
        Intent::StartConversation | Intent::Reply | Intent::Continue => Utf16Range {
            location: caret,
            length: 0,
        },
        // Replace the draft the user wrote, which is everything between the protected prefix
        // and the caret. Text after the caret (a signature, a quoted thread) is preserved.
        Intent::Refine => Utf16Range {
            location: floor,
            length: caret - floor,
        },
        // A short field holds one value; replace it whole.
        Intent::FillField => Utf16Range {
            location: floor,
            length: field_len - floor,
        },
    }
}

/// Which field a press came from, compared again before writing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldIdentity {
    pub pid: i32,
    pub role: String,
    pub label: String,
    pub window_title: String,
}

impl FieldIdentity {
    /// Same app, same kind of box, same label. The window title is deliberately left out: chat
    /// and browser titles change while a draft is being written (unread counts, tab names), and
    /// a changed title says nothing about the box.
    pub fn is_same_field(&self, other: &FieldIdentity) -> bool {
        self.pid == other.pid && self.role == other.role && self.label == other.label
    }
}

/// What was true when the key was pressed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Origin {
    pub identity: FieldIdentity,
    /// The field's whole text at press time.
    pub value: String,
}

/// What is true now, just before writing. `None` means it could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Now {
    pub frontmost_pid: i32,
    pub element_alive: bool,
    pub value: Option<String>,
    pub identity: Option<FieldIdentity>,
}

/// Why a draft was not written into the field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    AppSwitched,
    FieldGone,
    FieldChanged,
    UserKeptTyping,
    TerminalMultiline,
    TooLong,
}

impl Refusal {
    pub fn as_str(self) -> &'static str {
        match self {
            Refusal::AppSwitched => "app_switched",
            Refusal::FieldGone => "field_gone",
            Refusal::FieldChanged => "field_changed",
            Refusal::UserKeptTyping => "user_kept_typing",
            Refusal::TerminalMultiline => "terminal_multiline",
            Refusal::TooLong => "too_long",
        }
    }

    /// One sentence for the user. Plain hyphens only, per the app-text rule.
    pub fn user_message(self) -> &'static str {
        match self {
            Refusal::AppSwitched => "You switched apps, so Meridian left your text alone. The draft is on your clipboard.",
            Refusal::FieldGone => "The text box went away. The draft is on your clipboard.",
            Refusal::FieldChanged => "The focus moved to a different text box. The draft is on your clipboard.",
            Refusal::UserKeptTyping => "You kept typing, so Meridian left your text alone. The draft is on your clipboard.",
            Refusal::TerminalMultiline => "That draft has several lines, which a terminal would run. It is on your clipboard.",
            Refusal::TooLong => "This text box is too long to replace safely. The draft is on your clipboard.",
        }
    }
}

/// Decide whether it is still safe to write `text` into the field the press came from.
pub fn guard(origin: &Origin, now: &Now, surface: SurfaceKind, text: &str) -> Result<(), Refusal> {
    if now.frontmost_pid != origin.identity.pid {
        return Err(Refusal::AppSwitched);
    }
    if !now.element_alive {
        return Err(Refusal::FieldGone);
    }
    match &now.identity {
        Some(id) if id.is_same_field(&origin.identity) => {}
        _ => return Err(Refusal::FieldChanged),
    }
    match &now.value {
        Some(v) if same_text(v, &origin.value) => {}
        // Unreadable counts as changed: writing blind is how a draft lands in the wrong place.
        _ => return Err(Refusal::UserKeptTyping),
    }
    if surface == SurfaceKind::Terminal && text.contains('\n') {
        return Err(Refusal::TerminalMultiline);
    }
    if super::ranges::utf16_len(&origin.value) > MAX_FIELD_UTF16 {
        return Err(Refusal::TooLong);
    }
    Ok(())
}

/// The field value we expect after replacing `range` of `before` with `text`, or `None`
/// when the range does not fit the text (the caller must then refuse, not guess).
pub fn expected_value(before: &str, range: Utf16Range, text: &str) -> Option<String> {
    splice(before, range, text)
}

/// Editors rewrite blanks as you type: a trailing space becomes a non-breaking space once a
/// character follows it, and invisible zero-width marks appear around tags. None of that is a
/// change to the user's text, so every comparison of field text goes through this one form.
pub fn canonical_blanks(s: &str) -> String {
    s.chars()
        .filter(|c| !matches!(c, '\u{200b}' | '\u{200c}' | '\u{200d}' | '\u{feff}'))
        .map(|c| {
            if matches!(c, '\u{a0}' | '\u{202f}') {
                ' '
            } else {
                c
            }
        })
        .collect()
}

/// True when two readings of a field hold the same text, allowing for blank normalisation.
pub fn same_text(a: &str, b: &str) -> bool {
    canonical_blanks(a) == canonical_blanks(b)
}

/// True when the value read back is the value we expected. Whitespace at the very ends is
/// ignored because some apps trim or normalise it, but nothing in the middle is.
pub fn landed(expected: &str, observed: Option<&str>) -> bool {
    observed.is_some_and(|o| same_text(o.trim(), expected.trim()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(location: usize, length: usize) -> Utf16Range {
        Utf16Range { location, length }
    }

    fn id() -> FieldIdentity {
        FieldIdentity {
            pid: 42,
            role: "AXTextArea".into(),
            label: "Message Body".into(),
            window_title: "Compose".into(),
        }
    }

    fn origin(value: &str) -> Origin {
        Origin {
            identity: id(),
            value: value.into(),
        }
    }

    fn now(value: &str) -> Now {
        Now {
            frontmost_pid: 42,
            element_alive: true,
            value: Some(value.into()),
            identity: Some(id()),
        }
    }

    #[test]
    fn rewrite_selection_replaces_only_the_selection() {
        assert_eq!(plan_edit(Intent::RewriteSelection, r(6, 5), 30, 0), r(6, 5));
    }

    #[test]
    fn start_reply_and_continue_insert_at_the_caret_and_remove_nothing() {
        for intent in [Intent::StartConversation, Intent::Reply, Intent::Continue] {
            assert_eq!(plan_edit(intent, r(10, 0), 30, 0), r(10, 0), "{intent:?}");
        }
    }

    #[test]
    fn refine_replaces_the_draft_before_the_caret_and_keeps_what_follows() {
        assert_eq!(plan_edit(Intent::Refine, r(25, 0), 60, 0), r(0, 25));
    }

    #[test]
    fn fill_field_replaces_the_whole_value() {
        assert_eq!(plan_edit(Intent::FillField, r(3, 0), 12, 0), r(0, 12));
    }

    #[test]
    fn a_caret_beyond_the_field_is_clamped_not_trusted() {
        assert_eq!(plan_edit(Intent::Refine, r(999, 0), 10, 0), r(0, 10));
        assert_eq!(plan_edit(Intent::Continue, r(999, 0), 10, 0), r(10, 0));
    }

    #[test]
    fn an_unchanged_field_in_the_same_app_is_safe() {
        assert_eq!(
            guard(
                &origin("hello"),
                &now("hello"),
                SurfaceKind::DirectChat,
                "hi"
            ),
            Ok(())
        );
    }

    #[test]
    fn switching_apps_is_refused() {
        let mut n = now("hello");
        n.frontmost_pid = 7;
        assert_eq!(
            guard(&origin("hello"), &n, SurfaceKind::DirectChat, "hi"),
            Err(Refusal::AppSwitched)
        );
    }

    #[test]
    fn a_vanished_field_is_refused() {
        let mut n = now("hello");
        n.element_alive = false;
        assert_eq!(
            guard(&origin("hello"), &n, SurfaceKind::DirectChat, "hi"),
            Err(Refusal::FieldGone)
        );
    }

    #[test]
    fn a_different_field_in_the_same_app_is_refused() {
        let mut n = now("hello");
        n.identity = Some(FieldIdentity {
            label: "Search".into(),
            ..id()
        });
        assert_eq!(
            guard(&origin("hello"), &n, SurfaceKind::DirectChat, "hi"),
            Err(Refusal::FieldChanged)
        );
        n.identity = None;
        assert_eq!(
            guard(&origin("hello"), &n, SurfaceKind::DirectChat, "hi"),
            Err(Refusal::FieldChanged)
        );
    }

    #[test]
    fn typing_while_the_model_worked_is_refused() {
        assert_eq!(
            guard(
                &origin("hello"),
                &now("hello w"),
                SurfaceKind::DirectChat,
                "hi"
            ),
            Err(Refusal::UserKeptTyping)
        );
    }

    #[test]
    fn an_unreadable_value_is_treated_as_changed() {
        let mut n = now("hello");
        n.value = None;
        assert_eq!(
            guard(&origin("hello"), &n, SurfaceKind::DirectChat, "hi"),
            Err(Refusal::UserKeptTyping)
        );
    }

    #[test]
    fn a_multiline_draft_never_goes_to_a_terminal() {
        assert_eq!(
            guard(&origin(""), &now(""), SurfaceKind::Terminal, "ls\nrm -rf x"),
            Err(Refusal::TerminalMultiline)
        );
        assert_eq!(
            guard(&origin(""), &now(""), SurfaceKind::Terminal, "ls -la"),
            Ok(())
        );
    }

    #[test]
    fn a_huge_field_is_not_replaced_in_place() {
        let big = "x".repeat(MAX_FIELD_UTF16 + 1);
        assert_eq!(
            guard(&origin(&big), &now(&big), SurfaceKind::Document, "hi"),
            Err(Refusal::TooLong)
        );
    }

    #[test]
    fn app_switch_is_reported_before_anything_else() {
        let mut n = now("changed");
        n.frontmost_pid = 7;
        n.element_alive = false;
        assert_eq!(
            guard(&origin("hello"), &n, SurfaceKind::Terminal, "a\nb"),
            Err(Refusal::AppSwitched)
        );
    }

    #[test]
    fn expected_value_splices_by_utf16_range() {
        assert_eq!(
            expected_value("Hi \u{1F600} there", r(0, 5), "Hello"),
            Some("Hello there".into())
        );
        assert_eq!(expected_value("abc", r(9, 0), "x"), None);
    }

    #[test]
    fn landed_ignores_edge_whitespace_but_not_content() {
        assert!(landed("Hello there", Some("Hello there\n")));
        assert!(!landed("Hello there", Some("Hello")));
        assert!(!landed("Hello there", None));
    }

    #[test]
    fn every_refusal_has_a_plain_hyphen_message() {
        for r in [
            Refusal::AppSwitched,
            Refusal::FieldGone,
            Refusal::FieldChanged,
            Refusal::UserKeptTyping,
            Refusal::TerminalMultiline,
            Refusal::TooLong,
        ] {
            let m = r.user_message();
            assert!(!m.is_empty());
            assert!(!m.contains('\u{2014}') && !m.contains('\u{2013}') && !m.contains("--"));
        }
    }

    #[test]
    fn a_protected_tag_is_never_replaced_by_a_refine() {
        // "Jurgen " (7) is the tag, "nice post" follows, caret at the end of 16.
        assert_eq!(plan_edit(Intent::Refine, r(16, 0), 16, 7), r(7, 9));
    }

    #[test]
    fn a_reply_inserts_after_the_tag_even_if_the_caret_sits_before_it() {
        assert_eq!(plan_edit(Intent::Reply, r(0, 0), 7, 7), r(7, 0));
    }

    #[test]
    fn a_protected_prefix_cannot_reach_past_the_field() {
        assert_eq!(plan_edit(Intent::Refine, r(3, 0), 5, 99), r(5, 0));
    }

    #[test]
    fn a_highlight_starting_inside_the_tag_is_clamped_to_after_it() {
        assert_eq!(
            plan_edit(Intent::RewriteSelection, r(2, 10), 30, 7),
            r(7, 5)
        );
    }

    #[test]
    fn a_highlight_past_the_end_of_the_field_is_clamped_to_it() {
        assert_eq!(
            plan_edit(Intent::RewriteSelection, r(8, 99), 12, 0),
            r(8, 4)
        );
    }

    #[test]
    fn blank_normalisation_does_not_count_as_a_change() {
        assert!(same_text("Ann\u{a0}", "Ann "));
        assert!(same_text("\u{200b}Ann", "Ann"));
        assert!(!same_text("Ann", "Anna"));
    }

    #[test]
    fn a_landed_write_tolerates_non_breaking_spaces() {
        assert!(landed("Hi there", Some("Hi\u{a0}there")));
        assert!(!landed("Hi there", Some("Hi there!")));
        assert!(!landed("Hi there", None));
    }

    #[test]
    fn a_changed_window_title_alone_does_not_block_the_write() {
        let mut moved = id();
        moved.window_title = "(3) Compose".into();
        assert!(moved.is_same_field(&id()));
        moved.label = "Subject".into();
        assert!(!moved.is_same_field(&id()));
    }
}
