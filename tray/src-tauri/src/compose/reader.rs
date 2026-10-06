//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Reading the focused text field and the text around it.
//!
//! # Tight walk, not a page dump
//! The text that matters is the text nearest the field: the last messages above a reply
//! box, the subject line of a compose window. A depth-first walk from the top of a browser
//! page would spend its whole time budget on navigation chrome before reaching it. Instead
//! this climbs from the field up through its ancestors and, at each level, takes the
//! siblings before it (nearest first) and after it. The cost then follows what is used, and
//! the most relevant text is always collected first. If that yields nothing it falls back
//! to a bounded walk of the whole window and marks the result `broad`.
//!
//! # What is read, and what is not
//! Text is read only from the front window of the app the user is in, only when they press
//! the key. Secure fields, private windows and secure keyboard input are reported to the
//! classifier, which refuses them before anything is sent to a model.
//!
//! # Who calls this
//! [`super::controller`], once per press, on its worker thread.
//!
//! # Related
//! - [`super::ax`] - the Accessibility calls used here.
//! - [`super::ranges`] - splitting the value around the selection.
//! - [`meridian::compose::types::FieldSnapshot`] - what this produces.

use std::time::Duration;

use meridian::compose::types::{FieldSnapshot, NearbyText};
use objc2_app_kit::NSWorkspace;

use super::ax::{self, Element};
use super::delivery::FieldIdentity;
use super::element_tree::{label_of, limits, whole_window_lines, TEXT_ROLES};
use super::field_text;
pub(super) use super::field_text::visible_value;
use super::mention;
use super::ranges::{self, Utf16Range};
use super::walk;

#[link(name = "Carbon", kind = "framework")]
extern "C" {
    fn IsSecureEventInputEnabled() -> bool;
}

/// Everything remembered about the field a press came from, kept for the write step.
#[derive(Debug)]
pub struct FieldHandle {
    pub element: Element,
    pub pid: i32,
    pub identity: FieldIdentity,
    /// The field's whole text at press time.
    pub value: String,
    /// The selection at press time (a bare caret has length zero).
    pub selection: Utf16Range,
    /// True when `selection` is what the app reported, not a guess. Keystrokes are only sent at
    /// a caret that is known.
    pub selection_known: bool,
    /// Length in UTF-16 units of the leading text the app put in the box (a tag of the person
    /// being answered). Never edited; zero when the box holds only what the user typed.
    pub protected_prefix: usize,
}

/// A read field: what the pipeline sees and what the writer needs.
#[derive(Debug)]
pub struct ReadField {
    pub snapshot: FieldSnapshot,
    pub handle: FieldHandle,
}

/// Why no field could be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadError {
    AccessibilityDenied,
    NoFocusedApp,
    NoFocusedField,
    NotATextField,
}

impl ReadError {
    pub fn as_str(self) -> &'static str {
        match self {
            ReadError::AccessibilityDenied => "accessibility_denied",
            ReadError::NoFocusedApp => "no_focused_app",
            ReadError::NoFocusedField => "no_focused_field",
            ReadError::NotATextField => "not_a_text_field",
        }
    }
}

/// The app in front: process id, display name, bundle id.
pub fn frontmost_app() -> Option<(i32, String, String)> {
    let workspace = NSWorkspace::sharedWorkspace();
    let app = workspace.frontmostApplication()?;
    let name = app
        .localizedName()
        .map(|n| n.to_string())
        .unwrap_or_default();
    let bundle = app
        .bundleIdentifier()
        .map(|b| b.to_string())
        .unwrap_or_default();
    Some((app.processIdentifier(), name, bundle))
}

/// The pid of the frontmost app, for the write-time guard.
pub fn frontmost_pid() -> Option<i32> {
    frontmost_app().map(|(pid, _, _)| pid)
}

/// True when secure keyboard input is active somewhere on the system.
pub fn secure_input_active() -> bool {
    // SAFETY: a plain query with no arguments.
    unsafe { IsSecureEventInputEnabled() }
}

fn focused_element(pid: i32, app: &Element) -> Option<Element> {
    if let Some(system) = Element::system_wide() {
        if let Some(el) = system.element("AXFocusedUIElement") {
            if el.pid() == Some(pid) {
                return Some(el);
            }
        }
    }
    app.element("AXFocusedUIElement")
}

/// Chromium and Electron apps build their accessibility tree only when asked. Setting this
/// one attribute is the documented way to ask; native apps ignore it harmlessly.
fn wake_accessibility(app: &Element) {
    let _ = app.set_bool("AXManualAccessibility", true);
}

fn strip_query(url: &str) -> String {
    url.split(['?', '#']).next().unwrap_or(url).to_string()
}

fn page_url(field: &Element) -> Option<String> {
    let mut current = field.clone();
    for _ in 0..40 {
        if current.string("AXRole").as_deref() == Some("AXWebArea") {
            return current.url("AXURL").map(|u| strip_query(&u));
        }
        current = current.element("AXParent")?;
    }
    None
}

fn looks_private(title: &str) -> bool {
    let t = title.to_ascii_lowercase();
    t.contains("incognito")
        || t.contains("private browsing")
        || t.contains("inprivate")
        || t.contains("private window")
}

/// Decides whether an app or page is on the user's capture ignore list.
pub type IgnoreFn = std::sync::Arc<dyn Fn(&str, Option<&str>) -> bool + Send + Sync>;

/// What the reader needs to know about the user's settings.
pub struct ReadContext {
    /// Also read the other windows visible on screen.
    pub other_windows: bool,
    /// The user's capture ignore list: `(app name, page address)` -> skip it.
    pub is_ignored: IgnoreFn,
}

/// Read the focused text field of the frontmost app.
#[tracing::instrument(
    skip_all,
    fields(app, role, value_chars, above_chars, broad, rest_chars, other_windows)
)]
pub fn read_focused_field(ctx: &ReadContext) -> Result<ReadField, ReadError> {
    if !ax::is_trusted() {
        return Err(ReadError::AccessibilityDenied);
    }
    let (pid, app_name, bundle_id) = frontmost_app().ok_or(ReadError::NoFocusedApp)?;
    let app = Element::application(pid).ok_or(ReadError::NoFocusedApp)?;

    // A Chromium app with no tree yet has no focused element. Ask for the tree and retry
    // briefly; the first press in a fresh browser session is the one that needs this.
    let mut element = focused_element(pid, &app);
    if element.is_none() {
        wake_accessibility(&app);
        for _ in 0..4 {
            std::thread::sleep(Duration::from_millis(120));
            element = focused_element(pid, &app);
            if element.is_some() {
                break;
            }
        }
    }
    let element = element.ok_or(ReadError::NoFocusedField)?;

    let role = element.string("AXRole").unwrap_or_default();
    let subrole = element.string("AXSubrole").unwrap_or_default();
    if !TEXT_ROLES.contains(&role.as_str()) {
        return Err(ReadError::NotATextField);
    }

    let value = field_text::visible_value(&element).unwrap_or_default();
    let total = ranges::utf16_len(&value);
    let reported_selection = element
        .range("AXSelectedTextRange")
        .filter(|r| r.location.saturating_add(r.length) <= total);
    let mut selection = reported_selection.unwrap_or(Utf16Range {
        location: total,
        length: 0,
    });
    let (mut before, selected, mut after) = ranges::split(&value, selection)
        .unwrap_or_else(|| (value.clone(), String::new(), String::new()));

    // An empty draft above a quoted message (a reply box) is a reply to that message. The quoted
    // text stays in the box - only the snapshot calls it "below the draft" - so the intent is
    // read correctly and the write still inserts at the caret above it.
    let mut below_draft = String::new();
    if before.trim().is_empty()
        && selected.trim().is_empty()
        && field_text::looks_like_quoted_reply(&after)
    {
        below_draft = std::mem::take(&mut after);
    }

    let window_title = app
        .element("AXFocusedWindow")
        .and_then(|w| w.string("AXTitle"))
        .unwrap_or_default();
    let label = label_of(&element);
    let placeholder = element.string("AXPlaceholderValue").unwrap_or_default();
    let url = page_url(&element);

    let around = walk::surrounding(&element, limits());

    // A tag the app pre-filled (replying to a comment) is not the user's draft: keep it, and
    // treat what follows it as the box.
    let mut protected_prefix = mention::protected_prefix(
        &value,
        &field_text::link_titles(&element),
        &around.above.join("\n"),
    );
    let mut kept_prefix = String::new();
    if protected_prefix > 0 {
        if selection.length > 0 && selection.location < protected_prefix {
            // The user highlighted part of the tag itself: leave it entirely to them.
            protected_prefix = 0;
        } else {
            selection = Utf16Range {
                location: selection.location.max(protected_prefix),
                length: selection.length,
            };
            if let Some((_, tag, _)) = ranges::split(
                &value,
                Utf16Range {
                    location: 0,
                    length: protected_prefix,
                },
            ) {
                kept_prefix = tag
                    .trim_matches(|c: char| c.is_whitespace() || c == '\u{200b}')
                    .to_string();
            }
            if let Some((_, typed, _)) = ranges::split(
                &value,
                Utf16Range {
                    location: protected_prefix,
                    length: selection.location - protected_prefix,
                },
            ) {
                before = typed;
            }
        }
    }
    let whole = whole_window_lines(&app, &element);
    let header = around.header;

    // Text the nearby climb already found is not repeated in the rest of the window.
    let mut known = around.above.clone();
    known.extend(around.below.iter().cloned());
    known.extend(header.iter().map(|h| h.value.clone()));

    let tight_empty = around
        .above
        .iter()
        .chain(around.below.iter())
        .all(|l| l.trim().is_empty());
    let (above, below, rest_of_window, broad) = if tight_empty {
        // The climb found nothing (an app with a flat tree): the whole window is the context.
        (
            whole.join("\n"),
            String::new(),
            String::new(),
            !whole.is_empty(),
        )
    } else {
        (
            around.above.join("\n"),
            around.below.join("\n"),
            walk::lines_not_in(&whole, &known).join("\n"),
            false,
        )
    };

    let other_windows = if ctx.other_windows {
        super::screen::read_other_windows(pid, &*ctx.is_ignored)
    } else {
        Vec::new()
    };

    let multiline = role == "AXTextArea" || value.contains('\n');
    let snapshot = FieldSnapshot {
        app_name: app_name.clone(),
        bundle_id,
        window_title: window_title.clone(),
        url,
        ax_role: role.clone(),
        ax_subrole: subrole,
        label: label.clone(),
        placeholder,
        multiline,
        before,
        selection: selected,
        after,
        kept_prefix,
        below_draft,
        nearby: NearbyText {
            above,
            below,
            broad,
            rest_of_window,
        },
        header,
        other_windows,
        private_window: looks_private(&window_title),
        secure_input: secure_input_active(),
    };

    let span = tracing::Span::current();
    span.record("app", app_name.as_str());
    span.record("role", role.as_str());
    span.record("value_chars", value.chars().count());
    span.record("above_chars", snapshot.nearby.above.chars().count());
    span.record("broad", broad);
    span.record("rest_chars", snapshot.nearby.rest_of_window.chars().count());
    span.record("other_windows", snapshot.other_windows.len());

    let identity = FieldIdentity { pid, role, label };
    Ok(ReadField {
        snapshot,
        handle: FieldHandle {
            element,
            pid,
            identity,
            value,
            selection,
            selection_known: reported_selection == Some(selection),
            protected_prefix,
        },
    })
}

/// True when `element` is the box that currently has keyboard focus in app `pid`.
///
/// Every synthetic key goes to whatever has focus, and a paste lands there too. The frontmost
/// app being the same is not enough: the user can click another box in the same window while
/// the model works, and the key presses and the write would then go into that box.
pub(super) fn is_focused(element: &Element, pid: i32) -> bool {
    Element::application(pid)
        .and_then(|app| focused_element(pid, &app))
        .is_some_and(|focused| focused.same_as(element))
}

/// Re-read the pieces the write guard compares, for the field a press came from.
pub fn current_state(handle: &FieldHandle) -> super::delivery::Now {
    let alive = handle.element.is_alive();
    let value = alive.then(|| field_text::visible_value(&handle.element).unwrap_or_default());
    let identity = alive.then(|| FieldIdentity {
        pid: handle.pid,
        role: handle.element.string("AXRole").unwrap_or_default(),
        label: label_of(&handle.element),
    });
    super::delivery::Now {
        frontmost_pid: frontmost_pid().unwrap_or(-1),
        element_alive: alive,
        focused: alive && is_focused(&handle.element, handle.pid),
        value,
        identity,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_and_fragment_are_stripped_from_page_addresses() {
        assert_eq!(strip_query("https://a.com/p?q=1#x"), "https://a.com/p");
        assert_eq!(strip_query("https://a.com/p"), "https://a.com/p");
    }

    #[test]
    fn private_windows_are_recognised_by_title() {
        assert!(looks_private("Inbox - Google Chrome - Incognito"));
        assert!(looks_private("Private Browsing - Firefox"));
        assert!(looks_private("InPrivate - Microsoft Edge"));
        assert!(!looks_private("Compose - Meridiona Mail"));
    }

    #[test]
    fn read_errors_have_stable_names() {
        let names = [
            ReadError::AccessibilityDenied,
            ReadError::NoFocusedApp,
            ReadError::NoFocusedField,
            ReadError::NotATextField,
        ]
        .map(ReadError::as_str);
        let mut sorted = names.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), names.len());
    }
}
