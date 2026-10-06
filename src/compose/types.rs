//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! The values that cross the boundary between the tray (which reads a text field)
//! and the compose pipeline (which decides what to write into it).
//!
//! # Why these are plain data
//! The tray owns Accessibility, so it is the only place that can know what is in
//! the focused field. Everything after that read - classifying the box, choosing the
//! intent, building the prompt, parsing the answer - is a pure function of these
//! values. Keeping them free of handles and I/O is what makes the whole decision
//! table unit-testable without a live window.
//!
//! The tray converts the UTF-16 offsets Accessibility speaks into plain strings
//! (`before` / `selection` / `after`) itself, so no logic here ever touches offsets.
//!
//! # Who calls this
//! The tray's field reader builds a [`FieldSnapshot`]; [`crate::compose::classify`]
//! and [`crate::compose::prompt`] consume it.
//!
//! # Related
//! - [`crate::compose::classify`] - turns a snapshot into a surface and an intent.
//! - [`crate::compose::prompt`] - turns a classified snapshot into the model request.

use serde::{Deserialize, Serialize};

/// One labelled value read from a compose window's own fields, such as
/// `To = Nabeel` or `Subject = Intro call`. The label is in the app's own language.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct HeaderField {
    pub label: String,
    pub value: String,
}

/// Text read from around the focused field, in document order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct NearbyText {
    /// Content above the field. The lines closest to the field come last.
    pub above: String,
    /// Content below the field. The lines closest to the field come first.
    pub below: String,
    /// True when a tight walk around the field failed and this is the whole window
    /// instead. It is noisier and the prompt says so.
    pub broad: bool,
    /// The rest of the focused window: text that is on screen but was not collected near the
    /// box (side panels, a document beside a chat, other columns). Lines already in `above`
    /// or `below` are not repeated here.
    #[serde(default)]
    pub rest_of_window: String,
}

/// Everything the tray read about the focused text field at the moment of the press.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct FieldSnapshot {
    /// Foreground app display name, e.g. `Slack`.
    pub app_name: String,
    /// Foreground app bundle id, e.g. `com.tinyspeck.slackmacgap`. Empty when unknown.
    pub bundle_id: String,
    pub window_title: String,
    /// Page address for browser content, with query and fragment already stripped.
    pub url: Option<String>,
    /// Accessibility role of the focused element, e.g. `AXTextArea`.
    pub ax_role: String,
    /// Accessibility subrole, e.g. `AXSecureTextField`.
    pub ax_subrole: String,
    /// The element's accessibility description or aria-label, e.g. `Message Body`.
    pub label: String,
    pub placeholder: String,
    /// True when the field can hold more than one line.
    pub multiline: bool,
    /// Field text before the selection (or before the caret when nothing is selected).
    pub before: String,
    /// The highlighted text, empty when nothing is selected.
    pub selection: String,
    /// Field text after the selection or caret.
    pub after: String,
    /// A tag the app put in the box before the user typed anything (the person being
    /// answered). It stays in the box; the draft is written after it. Empty when there is none.
    pub kept_prefix: String,
    /// Read-only content that sits below the draft inside the same field, such as a
    /// quoted thread or a signature. It is preserved by the app and never output.
    pub below_draft: String,
    pub nearby: NearbyText,
    pub header: Vec<HeaderField>,
    /// The window is a private or incognito window.
    pub private_window: bool,
    /// The system reports secure keyboard input is active (a password prompt).
    pub secure_input: bool,
}

impl FieldSnapshot {
    /// The text the user would see in the field: before + selection + after.
    pub fn field_text(&self) -> String {
        let mut out =
            String::with_capacity(self.before.len() + self.selection.len() + self.after.len());
        out.push_str(&self.before);
        out.push_str(&self.selection);
        out.push_str(&self.after);
        out
    }

    /// True when the field holds no visible text at all.
    pub fn is_empty_field(&self) -> bool {
        self.field_text().trim().is_empty()
    }

    /// True when a non-blank selection is present.
    pub fn has_selection(&self) -> bool {
        !self.selection.trim().is_empty()
    }
}

/// What kind of text box this is. Drives the register and the rules the model gets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SurfaceKind {
    EmailCompose,
    DirectChat,
    PublicComment,
    Document,
    CodeEditor,
    Terminal,
    AiPrompt,
    StructuredField,
    Generic,
}

impl SurfaceKind {
    /// Every supported kind, in a stable order. Used by tests that must cover them all.
    pub const ALL: [SurfaceKind; 9] = [
        SurfaceKind::EmailCompose,
        SurfaceKind::DirectChat,
        SurfaceKind::PublicComment,
        SurfaceKind::Document,
        SurfaceKind::CodeEditor,
        SurfaceKind::Terminal,
        SurfaceKind::AiPrompt,
        SurfaceKind::StructuredField,
        SurfaceKind::Generic,
    ];

    /// Stable machine name, used in logs, outcome rows and golden file names.
    pub fn as_str(self) -> &'static str {
        match self {
            SurfaceKind::EmailCompose => "email_compose",
            SurfaceKind::DirectChat => "direct_chat",
            SurfaceKind::PublicComment => "public_comment",
            SurfaceKind::Document => "document",
            SurfaceKind::CodeEditor => "code_editor",
            SurfaceKind::Terminal => "terminal",
            SurfaceKind::AiPrompt => "ai_prompt",
            SurfaceKind::StructuredField => "structured_field",
            SurfaceKind::Generic => "generic",
        }
    }
}

/// Why a press does nothing. Each one is a safety or privacy decision, not a failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RefusalReason {
    /// A password or other secure text field.
    SecureField,
    /// The system reports secure keyboard input.
    SecureInput,
    /// A private or incognito window.
    PrivateWindow,
    /// A search box or the browser address bar.
    SearchOrAddressBar,
}

impl RefusalReason {
    pub fn as_str(self) -> &'static str {
        match self {
            RefusalReason::SecureField => "secure_field",
            RefusalReason::SecureInput => "secure_input",
            RefusalReason::PrivateWindow => "private_window",
            RefusalReason::SearchOrAddressBar => "search_or_address_bar",
        }
    }

    /// One sentence the overlay can show. Plain hyphens only, per the app-text rule.
    pub fn user_message(self) -> &'static str {
        match self {
            RefusalReason::SecureField | RefusalReason::SecureInput => {
                "Meridian does not write in password fields."
            }
            RefusalReason::PrivateWindow => "Meridian does not read private windows.",
            RefusalReason::SearchOrAddressBar => "Meridian does not write in search boxes.",
        }
    }
}

/// What the user is trying to do, decided from facts about the field alone.
///
/// Whether typed text is a draft to polish or a note to a ghostwriter cannot be decided
/// by rules, so [`Intent::Refine`] hands that call to the model, with a default of
/// "draft" in the prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Intent {
    /// Empty field and nothing to answer: write an opener.
    StartConversation,
    /// Empty field with a conversation or post above it: write the user's next message.
    Reply,
    /// Text present, caret at the end, in a surface where prose runs on: continue it.
    Continue,
    /// Text present: either a draft to polish or an instruction to carry out.
    Refine,
    /// A highlight is present: rewrite only the highlight.
    RewriteSelection,
    /// A short structured field: produce the value only.
    FillField,
}

impl Intent {
    pub const ALL: [Intent; 6] = [
        Intent::StartConversation,
        Intent::Reply,
        Intent::Continue,
        Intent::Refine,
        Intent::RewriteSelection,
        Intent::FillField,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Intent::StartConversation => "start_conversation",
            Intent::Reply => "reply",
            Intent::Continue => "continue",
            Intent::Refine => "refine",
            Intent::RewriteSelection => "rewrite_selection",
            Intent::FillField => "fill_field",
        }
    }
}

/// The model's previous answer for this same field, when the user pressed the key again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreviousAttempt {
    /// What Meridian wrote last time.
    pub output: String,
}

/// A press, ready to be classified and turned into a prompt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DraftRequest {
    pub field: FieldSnapshot,
    /// Set when the user re-ran the key on the same field soon after a draft.
    pub previous: Option<PreviousAttempt>,
    /// The user's own name, so the model can recognise their earlier messages in a thread
    /// (they are the best evidence of how the user writes to this person). Optional.
    #[serde(default)]
    pub user_name: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn field_text_joins_the_three_parts_in_order() {
        let f = FieldSnapshot {
            before: "Hi ".into(),
            selection: "there".into(),
            after: ", thanks".into(),
            ..Default::default()
        };
        assert_eq!(f.field_text(), "Hi there, thanks");
    }

    #[test]
    fn whitespace_only_field_counts_as_empty() {
        let f = FieldSnapshot {
            before: "  \n ".into(),
            ..Default::default()
        };
        assert!(f.is_empty_field());
    }

    #[test]
    fn blank_selection_is_not_a_selection() {
        let f = FieldSnapshot {
            selection: "   ".into(),
            ..Default::default()
        };
        assert!(!f.has_selection());
    }

    #[test]
    fn machine_names_are_unique() {
        let mut names: Vec<_> = SurfaceKind::ALL.iter().map(|k| k.as_str()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), SurfaceKind::ALL.len());
        let mut names: Vec<_> = Intent::ALL.iter().map(|k| k.as_str()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), Intent::ALL.len());
    }

    #[test]
    fn refusal_messages_use_plain_hyphens_only() {
        for r in [
            RefusalReason::SecureField,
            RefusalReason::SecureInput,
            RefusalReason::PrivateWindow,
            RefusalReason::SearchOrAddressBar,
        ] {
            let m = r.user_message();
            assert!(!m.contains('\u{2014}') && !m.contains('\u{2013}') && !m.contains("--"));
        }
    }
}
