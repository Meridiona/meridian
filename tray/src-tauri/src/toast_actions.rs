//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Toast-XML construction for the Windows interactive-notification path — the
//! pure half of [`crate::win_toast`], deliberately compiled on every platform.
//!
//! macOS carries interactive toasts as pre-registered `UNNotificationCategory`
//! objects: the button set is declared once at startup
//! ([`crate::sys::register_notification_categories`]) and each toast merely
//! names its category. Windows has no such registry — a toast's buttons live
//! **inline in that toast's XML payload** — so the same category descriptors
//! have to be translated to markup at delivery time. That translation is what
//! this module does, and it is the whole reason the two platforms diverge in
//! [`crate::sys::notify_outbox`].
//!
//! It lives outside the `cfg(target_os = "windows")` module on purpose. WinRT
//! code cannot be compiled, run, or tested from a macOS dev machine, so
//! everything that could be a logic bug — escaping, the button cap, element
//! ordering, `hint-inputId` wiring — is kept here where `cargo test` reaches it
//! on any host. What remains in [`crate::win_toast`] is only the WinRT calls
//! themselves.
//!
//! # Who calls this
//! [`crate::win_toast::show`], which loads the returned string into an
//! `XmlDocument` and hands it to `ToastNotificationManager`.
//!
//! # Related
//! - [`meridian_core::notifications::categories`] — the shared JSON descriptors
//!   this parses; the same bytes the daemon stamps into the outbox `actions`
//!   column and macOS registers as categories.
//! - [`crate::sys::notify_outbox`] — the delivery fork that calls this on Windows.
//! - `docs/notifications.md` — the lifecycle these buttons answer into.

// Off Windows nothing calls this - `win_toast` is the only consumer - but the
// module is still COMPILED everywhere so its tests run on a macOS dev machine.
// `cfg_attr(allow(dead_code))` is what expresses that; a bare `cfg` would
// delete the module off Windows and take the tests with it (see the
// backend_install.rs cfg-vs-cfg_attr note - mixing the two is how a Windows
// build breaks invisibly from here).
#![cfg_attr(not(target_os = "windows"), allow(dead_code))]

use serde::Deserialize;

/// Windows renders at most 5 buttons on a toast; extras are dropped by the
/// platform, silently. macOS's own budget is 4, so no shipped category is near
/// either limit - this is a guard against a future one, not a live trim.
pub const MAX_ACTIONS: usize = 5;

/// One button from a category descriptor.
///
/// Field names match the plugin's `ActionType` JSON (the macOS side
/// deserializes the identical bytes into its own struct), so the two platforms
/// can never disagree about what a category contains. Unknown fields
/// (`destructive`, `foreground`) are ignored rather than rejected: they are
/// macOS presentation hints with no Windows analogue.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct ToastAction {
    /// The action id echoed back on activation - becomes `response_action` on
    /// the outbox row, so it must round-trip byte-for-byte.
    pub id: String,
    /// The button label the user reads.
    pub title: String,
    /// Whether pressing this button should collect a line of text first.
    #[serde(default)]
    pub input: bool,
    /// Label for the send button of an inline-reply field.
    #[serde(default, rename = "inputButtonTitle")]
    pub input_button_title: Option<String>,
    /// Greyed-out prompt inside an empty inline-reply field.
    #[serde(default, rename = "inputPlaceholder")]
    pub input_placeholder: Option<String>,
}

/// Parse a category's JSON descriptor into its button set.
///
/// Returns an empty vec for malformed or absent JSON rather than failing:
/// a plain title+body toast is a strictly better outcome than no toast at all,
/// and the caller logs the miss. Over-long sets are truncated to
/// [`MAX_ACTIONS`] here so the truncation is visible to tests instead of
/// happening inside Windows.
pub fn parse_actions(json: &str) -> Vec<ToastAction> {
    let mut actions: Vec<ToastAction> = serde_json::from_str(json).unwrap_or_default();
    actions.truncate(MAX_ACTIONS);
    actions
}

/// The button set for an outbox row: its own `actions` column first, the
/// category registry as a fallback.
///
/// The column is the more truthful source — it is what the daemon decided when
/// it enqueued the row, and it survives a category being edited afterwards —
/// but rows predating the column carry only a category, and those must stay
/// interactive. A row with neither still delivers, as plain title+body.
///
/// Takes the two strings rather than a `PendingNotification` so it stays here,
/// on the platform-independent side, where `cargo test` reaches it.
pub fn resolve_actions(actions_json: Option<&str>, category: Option<&str>) -> Vec<ToastAction> {
    if let Some(json) = actions_json {
        let parsed = parse_actions(json);
        if !parsed.is_empty() {
            return parsed;
        }
    }
    category
        .and_then(meridian_core::notifications::categories::actions_json)
        .map(parse_actions)
        .unwrap_or_default()
}

/// Build the complete toast payload for a notification.
///
/// The shape is Windows' `ToastGeneric` template. Three details are
/// load-bearing and easy to get wrong:
///
/// - **`launch="tap"`** makes a click on the toast body activate with the
///   argument `tap`, which is the same vocabulary macOS reports for a body tap
///   and what [`crate::commands::record_notification_response`] matches on to
///   route a click-through.
/// - **`<input>` must precede `<action>`** inside `<actions>`. The toast schema
///   is ordered, and Windows rejects the whole document if it isn't - which
///   surfaces as a toast that simply never appears.
/// - **`hint-inputId` binds a button to a field.** The input's id is the
///   action's own id, so the activation handler can read the typed text back
///   out of `UserInput` keyed by the action id it was just handed, with no
///   second lookup table to keep in sync.
///
/// `duration="long"` is applied only when the toast has buttons: an
/// interactive toast the user is meant to answer gets the ~25 s banner rather
/// than the ~7 s default. It is the closest Windows equivalent to the
/// persistent Alerts style macOS uses for the same notifications; either way
/// the toast lands in Action Center afterwards with its buttons live, which is
/// what expiry ([`crate::sys::retract_toast`]) exists to clean up.
pub fn build_toast_xml(title: &str, body: &str, actions: &[ToastAction]) -> String {
    let duration = if actions.is_empty() {
        ""
    } else {
        r#" duration="long""#
    };
    let mut xml = format!(
        r#"<toast activationType="foreground" launch="tap"{duration}><visual><binding template="ToastGeneric"><text>{}</text><text>{}</text></binding></visual>"#,
        escape_xml(title),
        escape_xml(body),
    );

    if !actions.is_empty() {
        xml.push_str("<actions>");
        // Inputs first - the schema demands it (see the doc comment).
        for a in actions.iter().filter(|a| a.input) {
            xml.push_str(&format!(
                r#"<input id="{}" type="text" placeHolderContent="{}"/>"#,
                escape_xml(&a.id),
                escape_xml(a.input_placeholder.as_deref().unwrap_or("")),
            ));
        }
        for a in actions {
            let label = if a.input {
                a.input_button_title.as_deref().unwrap_or(&a.title)
            } else {
                &a.title
            };
            let bind = if a.input {
                format!(r#" hint-inputId="{}""#, escape_xml(&a.id))
            } else {
                String::new()
            };
            xml.push_str(&format!(
                r#"<action content="{}" arguments="{}" activationType="foreground"{bind}/>"#,
                escape_xml(label),
                escape_xml(&a.id),
            ));
        }
        xml.push_str("</actions>");
    }

    xml.push_str("</toast>");
    xml
}

/// Escape the five XML metacharacters.
///
/// Not cosmetic: toast bodies are assembled from user data (task titles, app
/// names, branch names), and a single `&` produces a document `XmlDocument::
/// LoadXml` rejects - which manifests as a toast that silently never shows,
/// on a machine no one can attach a debugger to. Attribute values need `"`
/// and `'` escaped too, so one function covers both positions.
fn escape_xml(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c if is_xml_char(c) => out.push(c),
            // DROPPED, not escaped. There is no escape for these: `&#1;` is
            // just as illegal as a literal U+0001, so a numeric entity would
            // fail identically. See [`is_xml_char`] for why they get this far.
            _ => {}
        }
    }
    out
}

/// Whether a character may appear in an XML 1.0 document at all.
///
/// Escaping the five metacharacters is not sufficient on its own. XML 1.0 §2.2
/// forbids most C0 control characters OUTRIGHT - there is no representation for
/// them, escaped or otherwise - and `XmlDocument::LoadXml` rejects the whole
/// document if one appears. That failure looks exactly like the `&` case this
/// module already guards: **the toast silently never shows**, on a machine no
/// one can attach a debugger to.
///
/// It is reachable because toast text is assembled from the user's own data.
/// Task titles and worklog bodies come from tracker APIs, editor buffers and
/// pasted terminal output, all of which carry stray control bytes often enough
/// to matter - a `\u{1}` in a Jira summary is not exotic, it is what a bad
/// copy-paste leaves behind.
///
/// Tab, newline and carriage return ARE legal and are kept: Windows renders a
/// newline inside a `<text>` element, and stripping them would reflow a body
/// the daemon deliberately laid out.
fn is_xml_char(c: char) -> bool {
    matches!(c,
        '\u{9}' | '\u{A}' | '\u{D}'
        | '\u{20}'..='\u{D7FF}'
        | '\u{E000}'..='\u{FFFD}'
        | '\u{10000}'..='\u{10FFFF}')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(id: &str, title: &str) -> ToastAction {
        ToastAction {
            id: id.to_string(),
            title: title.to_string(),
            input: false,
            input_button_title: None,
            input_placeholder: None,
        }
    }

    /// Every shipped category must survive the round-trip - this is the guard
    /// that a category added for macOS doesn't silently render button-less on
    /// Windows.
    #[test]
    fn every_shipped_category_parses_into_buttons() {
        use meridian_core::notifications::categories;
        for id in categories::ALL {
            let json = categories::actions_json(id)
                .unwrap_or_else(|| panic!("category {id} has no actions JSON"));
            let actions = parse_actions(json);
            assert!(
                !actions.is_empty(),
                "category {id} parsed to zero buttons on Windows"
            );
            assert!(
                actions.len() <= MAX_ACTIONS,
                "category {id} exceeds the cap"
            );
        }
    }

    /// The inline-reply category is the one that exercises every branch, so it
    /// is pinned end to end rather than by field.
    #[test]
    fn verify_switch_renders_input_before_actions_and_binds_it() {
        use meridian_core::notifications::categories;
        let actions = parse_actions(categories::actions_json(categories::VERIFY_SWITCH).unwrap());
        let xml = build_toast_xml("Still on the same task?", "You switched apps.", &actions);

        let input_at = xml.find("<input ").expect("no input element rendered");
        let first_action_at = xml.find("<action ").expect("no action element rendered");
        assert!(
            input_at < first_action_at,
            "Windows rejects a toast whose <input> follows an <action>: {xml}"
        );
        assert!(xml.contains(r#"<input id="reply" type="text""#), "{xml}");
        assert!(xml.contains(r#"hint-inputId="reply""#), "{xml}");
        // The input action's button carries its inputButtonTitle, not its title.
        assert!(xml.contains(r#"content="Send""#), "{xml}");
        // ...and the plain buttons keep their own labels and ids.
        assert!(
            xml.contains(r#"content="Yes, new task" arguments="yes""#),
            "{xml}"
        );
    }

    #[test]
    fn body_tap_is_reported_as_the_tap_action() {
        let xml = build_toast_xml("t", "b", &[]);
        assert!(xml.contains(r#"launch="tap""#), "{xml}");
    }

    /// A toast with no buttons must not carry an empty `<actions/>` block, and
    /// gets the default (short) banner duration.
    #[test]
    fn a_plain_toast_has_no_actions_block_and_default_duration() {
        let xml = build_toast_xml("Title", "Body", &[]);
        assert!(!xml.contains("<actions>"), "{xml}");
        assert!(!xml.contains("duration="), "{xml}");
        assert!(xml.contains("<text>Title</text><text>Body</text>"), "{xml}");
    }

    #[test]
    fn an_interactive_toast_gets_the_long_banner() {
        let xml = build_toast_xml("T", "B", &[plain("open", "Open")]);
        assert!(xml.contains(r#"duration="long""#), "{xml}");
    }

    /// The failure this prevents is invisible: an unescaped `&` makes
    /// `LoadXml` reject the document and the toast never appears.
    #[test]
    fn metacharacters_in_content_are_escaped() {
        let xml = build_toast_xml(
            "R&D <urgent>",
            r#"He said "it's fine""#,
            &[plain("a&b", "Fix & ship")],
        );
        assert!(xml.contains("<text>R&amp;D &lt;urgent&gt;</text>"), "{xml}");
        assert!(xml.contains("&quot;it&apos;s fine&quot;"), "{xml}");
        assert!(xml.contains(r#"content="Fix &amp; ship""#), "{xml}");
        assert!(xml.contains(r#"arguments="a&amp;b""#), "{xml}");
        // Nothing raw survives past the markup we intended.
        assert!(!xml.contains("R&D"), "{xml}");
    }

    /// The SAME invisible failure as the `&` case above, from the half of it
    /// escaping cannot reach.
    ///
    /// XML 1.0 has no representation for most C0 controls - `&#1;` is as
    /// illegal as a literal `\u{1}` - so `LoadXml` rejects the document and the
    /// toast silently never shows. They arrive with the user's own text: a
    /// title pasted out of a terminal or returned by a tracker API carries
    /// stray control bytes often enough to matter.
    #[test]
    fn control_characters_cannot_reach_the_document() {
        let xml = build_toast_xml(
            "ship \u{1}it\u{7}",
            "log \u{1b}[31mred\u{1b}[0m",
            &[plain("go\u{c}", "Do \u{b}it")],
        );
        for bad in ['\u{1}', '\u{7}', '\u{b}', '\u{c}', '\u{1b}'] {
            assert!(!xml.contains(bad), "{bad:?} survived into {xml}");
        }
        // Dropped, not mangled into an entity that would fail identically.
        assert!(!xml.contains("&#"), "{xml}");
        // The readable text is intact around the hole.
        assert!(xml.contains("<text>ship it</text>"), "{xml}");
        assert!(xml.contains(r#"content="Do it""#), "{xml}");
        assert!(xml.contains(r#"arguments="go""#), "{xml}");
    }

    /// Tab, newline and carriage return are LEGAL XML and must survive.
    ///
    /// Windows renders a newline inside `<text>`, and the daemon lays some
    /// bodies out deliberately - stripping every control character wholesale
    /// would reflow them, which is a real regression traded for a fix.
    #[test]
    fn the_three_legal_whitespace_controls_survive() {
        let xml = build_toast_xml("t", "one\ntwo\tthree\r", &[]);
        assert!(xml.contains("one\ntwo\tthree\r"), "{xml:?}");
    }

    /// Above the BMP, so the surrogate-range hole in [`is_xml_char`] is
    /// expressed as a range check rather than as an accidental cut-off. An
    /// emoji in a task title is ordinary, and dropping it would be a visible
    /// bug introduced by the fix for an invisible one.
    #[test]
    fn astral_characters_are_not_collateral() {
        let xml = build_toast_xml("done 🎉", "café 日本語", &[]);
        assert!(xml.contains("done 🎉"), "{xml}");
        assert!(xml.contains("café 日本語"), "{xml}");
    }

    /// The row's own column wins when it has content; the category registry
    /// covers rows that predate the column; neither is a plain toast, not a
    /// dropped one.
    #[test]
    fn the_rows_actions_column_wins_over_the_category_registry() {
        use meridian_core::notifications::categories;

        let from_row = resolve_actions(
            Some(r#"[{"id":"only","title":"Only"}]"#),
            Some(categories::VERIFY_SWITCH),
        );
        assert_eq!(from_row, vec![plain("only", "Only")]);

        // No column (old row) -> the registry keeps it interactive.
        let from_registry = resolve_actions(None, Some(categories::VERIFY_SWITCH));
        assert!(from_registry.len() > 1, "{from_registry:?}");

        // An unusable column falls back rather than delivering button-less.
        assert_eq!(
            resolve_actions(Some("[]"), Some(categories::VERIFY_SWITCH)),
            from_registry
        );

        // Neither -> a plain toast.
        assert!(resolve_actions(None, None).is_empty());
        assert!(resolve_actions(None, Some("no_such_category")).is_empty());
    }

    #[test]
    fn malformed_or_empty_descriptors_degrade_to_a_plain_toast() {
        assert!(parse_actions("not json").is_empty());
        assert!(parse_actions("[]").is_empty());
        assert!(parse_actions(r#"{"id":"x"}"#).is_empty());
    }

    #[test]
    fn more_than_five_buttons_are_capped() {
        let json = r#"[{"id":"1","title":"a"},{"id":"2","title":"b"},{"id":"3","title":"c"},
                       {"id":"4","title":"d"},{"id":"5","title":"e"},{"id":"6","title":"f"}]"#;
        let actions = parse_actions(json);
        assert_eq!(actions.len(), MAX_ACTIONS);
        assert_eq!(actions.last().unwrap().id, "5");
    }

    /// macOS-only presentation hints must not break the shared descriptor.
    #[test]
    fn unknown_macos_fields_are_ignored() {
        let actions =
            parse_actions(r#"[{"id":"x","title":"X","destructive":true,"foreground":true}]"#);
        assert_eq!(actions, vec![plain("x", "X")]);
    }
}
