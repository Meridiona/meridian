//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! The accessibility tree around a text field, as the walker sees it.
//!
//! Teaches [`super::walk`] how to read macOS accessibility elements ([`Node`] for
//! [`Element`]): which roles carry message text, which labels make a field a compose header,
//! and the budgets that keep a read of a huge window quick.
//!
//! # Who calls this
//! [`super::reader`], when it reads the text around the focused field and the rest of its window.
//!
//! # Related
//! - [`super::walk`] - the platform-neutral walk this plugs into.

use std::time::{Duration, Instant};

use super::ax::Element;
use super::walk::{self, Limits, Node};
use meridian::compose::types::HeaderField;

/// Roles that hold editable text.
pub(super) const TEXT_ROLES: &[&str] =
    &["AXTextField", "AXTextArea", "AXComboBox", "AXSearchField"];

/// Total time the surrounding-text walk may take.
pub(super) const WALK_BUDGET: Duration = Duration::from_millis(900);
/// Most elements visited while collecting one subtree.
const SUBTREE_NODE_CAP: usize = 400;
/// Most ancestor levels climbed.
const MAX_ANCESTORS: usize = 14;
/// Stop collecting above the box once this many characters are in hand.
const ABOVE_CAP_CHARS: usize = 9_000;
/// Stop collecting below the box once this many characters are in hand.
const BELOW_CAP_CHARS: usize = 2_000;
/// Labels that mark a compose-header field.
const HEADER_LABELS: &[&str] = &["to", "cc", "bcc", "subject", "recipients", "to recipients"];

pub(super) fn label_of(el: &Element) -> String {
    ["AXDescription", "AXTitle", "AXHelp"]
        .iter()
        .filter_map(|a| el.string(a))
        .map(|s| s.trim().to_string())
        .find(|s| !s.is_empty())
        .unwrap_or_default()
}

/// Text carried by one element, if it is the kind of element that carries message text.
fn text_of(el: &Element, role: &str) -> Option<String> {
    let raw = match role {
        "AXStaticText" => el.string("AXValue").or_else(|| el.string("AXDescription")),
        "AXHeading" => el.string("AXTitle").or_else(|| el.string("AXValue")),
        "AXLink" => el.string("AXTitle").or_else(|| el.string("AXDescription")),
        r if TEXT_ROLES.contains(&r) => el.string("AXValue"),
        _ => None,
    }?;
    let t = raw.trim();
    (!t.is_empty()).then(|| t.to_string())
}

impl Node for Element {
    fn role(&self) -> String {
        self.string("AXRole").unwrap_or_default()
    }

    fn own_text(&self, role: &str) -> Option<String> {
        text_of(self, role)
    }

    fn header(&self, role: &str) -> Option<HeaderField> {
        if !TEXT_ROLES.contains(&role) {
            return None;
        }
        let label = label_of(self).to_ascii_lowercase();
        if !HEADER_LABELS
            .iter()
            .any(|h| label == *h || label.starts_with(&format!("{h} ")))
        {
            return None;
        }
        let value = self
            .string("AXValue")
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())?;
        Some(HeaderField { label, value })
    }

    fn children(&self) -> Vec<Element> {
        self.elements("AXChildren")
    }

    fn parent(&self) -> Option<Element> {
        self.element("AXParent")
    }

    fn same_as(&self, other: &Element) -> bool {
        Element::same_as(self, other)
    }
}

pub(super) fn limits() -> Limits {
    Limits {
        deadline: Instant::now() + WALK_BUDGET,
        subtree_nodes: SUBTREE_NODE_CAP,
        ancestors: MAX_ANCESTORS,
        above_bytes: ABOVE_CAP_CHARS,
        below_bytes: BELOW_CAP_CHARS,
    }
}

/// Time the whole-window pass may take, after the nearby climb.
const REST_BUDGET: Duration = Duration::from_millis(700);

/// Every text line of the focused window, in document order, for the "rest of the window"
/// layer and as the fallback when the nearby climb finds nothing.
pub(super) fn whole_window_lines(app: &Element, field: &Element) -> Vec<String> {
    let Some(window) = app.element("AXFocusedWindow") else {
        return Vec::new();
    };
    let mut l = limits();
    l.deadline = Instant::now() + REST_BUDGET;
    l.subtree_nodes = SUBTREE_NODE_CAP * 2;
    walk::whole(&window, Some(field), l)
}
