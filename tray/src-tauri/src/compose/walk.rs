//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Collecting the text around a text box from a tree of on-screen elements.
//!
//! # Why this is generic and tested
//! The first live run on a LinkedIn thread answered an old message instead of the newest
//! one. The walk visited the message list from its first (oldest) message and stopped at a
//! node limit, so the newest messages - the ones the reply is about - were exactly what got
//! cut. That is a pure-logic bug in how a tree is traversed, so the traversal lives here,
//! generic over a small [`Node`] trait, and is tested against a fake tree with a long
//! conversation. The macOS element type implements [`Node`] in [`super::reader`].
//!
//! # The rule
//! Text **above** the box is collected nearest-first and from the **end** of each sibling's
//! subtree, because the last message before the box is the one that matters. Text **below**
//! the box is collected from the start. Output is always returned in document order, with
//! the text closest to the box last (above) or first (below).
//!
//! # Who calls this
//! [`super::reader`], with the macOS Accessibility element type.
//!
//! # Related
//! - [`meridian::compose::prompt`] - trims the result to its budget, again keeping the
//!   text nearest the box.

use std::time::Instant;

use meridian::compose::types::HeaderField;

use super::sides::{self, Line, Side};

/// What the walk needs to know about an element, independent of how it is stored.
pub trait Node: Clone {
    /// The element's role, e.g. `AXStaticText`.
    fn role(&self) -> String;
    /// The text this element contributes, if it is the kind that carries message text.
    fn own_text(&self, role: &str) -> Option<String>;
    /// A compose-header field (`To`, `Subject`) this element represents, if it is one.
    fn header(&self, role: &str) -> Option<HeaderField>;
    fn children(&self) -> Vec<Self>;
    fn parent(&self) -> Option<Self>;
    fn same_as(&self, other: &Self) -> bool;
    /// Which side of a chat this node's text sits on, when the screen shows it.
    fn side(&self) -> Option<Side> {
        None
    }
}

/// Bounds on one walk.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub deadline: Instant,
    /// Most elements visited while collecting one sibling's subtree.
    pub subtree_nodes: usize,
    /// Most ancestor levels climbed.
    pub ancestors: usize,
    /// Stop collecting above the box once this many bytes are in hand.
    pub above_bytes: usize,
    /// Stop collecting below the box once this many bytes are in hand.
    pub below_bytes: usize,
}

/// Text found around the box.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Surround {
    /// Lines above the box in document order; the line closest to the box is last.
    pub above: Vec<String>,
    /// Lines below the box in document order; the line closest to the box is first.
    pub below: Vec<String>,
    pub header: Vec<HeaderField>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Direction {
    /// Visit the first child first. Used below the box.
    Forward,
    /// Visit the last child first, so a node cap keeps the end. Used above the box.
    Backward,
}

struct Collector<'a, N: Node> {
    limits: Limits,
    /// The text box itself, skipped so the draft never reads its own text back as context.
    focused: Option<&'a N>,
    header: Vec<HeaderField>,
}

impl<N: Node> Collector<'_, N> {
    fn out_of_time(&self) -> bool {
        Instant::now() >= self.limits.deadline
    }

    /// The text of `root`'s subtree, in document order. When `Backward`, the node cap keeps
    /// the END of the subtree rather than the start.
    fn collect(&mut self, root: &N, direction: Direction) -> Vec<Line> {
        let mut lines = Vec::new();
        let mut stack = vec![root.clone()];
        let mut visited = 0usize;
        while let Some(node) = stack.pop() {
            if visited >= self.limits.subtree_nodes || self.out_of_time() {
                break;
            }
            visited += 1;
            if self.focused.is_some_and(|f| node.same_as(f)) {
                continue;
            }
            let role = node.role();
            if let Some(h) = node.header(&role) {
                if !self
                    .header
                    .iter()
                    .any(|x| x.label.eq_ignore_ascii_case(&h.label))
                {
                    self.header.push(h);
                }
            }
            if let Some(text) = node.own_text(&role) {
                lines.push(Line {
                    text,
                    side: node.side(),
                });
            }
            let children = node.children();
            match direction {
                // Pushed in reverse so the stack pops the first child first.
                Direction::Forward => stack.extend(children.into_iter().rev()),
                // Pushed in order so the stack pops the last child first.
                Direction::Backward => stack.extend(children),
            }
        }
        if direction == Direction::Backward {
            lines.reverse();
        }
        lines
    }
}

fn header_rank(label: &str) -> usize {
    let l = label.to_ascii_lowercase();
    ["to", "cc", "bcc", "subject"]
        .iter()
        .position(|k| l == *k || l.starts_with(&format!("{k} ")))
        .unwrap_or(4)
}

fn bytes(lines: &[Line]) -> usize {
    lines.iter().map(|l| l.text.len() + 1).sum()
}

/// Climb from `field` through its ancestors, collecting the siblings before and after the
/// branch at every level.
pub fn surrounding<N: Node>(field: &N, limits: Limits) -> Surround {
    let mut collector = Collector {
        limits,
        focused: Some(field),
        header: Vec::new(),
    };
    // Per level, innermost first.
    let mut above_levels: Vec<Vec<Line>> = Vec::new();
    let mut below_levels: Vec<Vec<String>> = Vec::new();
    let (mut above_bytes, mut below_bytes) = (0usize, 0usize);

    let mut child = field.clone();
    for _ in 0..limits.ancestors {
        if collector.out_of_time()
            || (above_bytes >= limits.above_bytes && below_bytes >= limits.below_bytes)
        {
            break;
        }
        let Some(parent) = child.parent() else {
            break;
        };
        let siblings = parent.children();
        if let Some(index) = siblings.iter().position(|s| s.same_as(&child)) {
            // Nearest sibling first. Each sibling is collected from its end, then the level is
            // flipped back into document order.
            let mut level_above: Vec<Line> = Vec::new();
            for sibling in siblings[..index].iter().rev() {
                if above_bytes >= limits.above_bytes || collector.out_of_time() {
                    break;
                }
                let mut lines = collector.collect(sibling, Direction::Backward);
                above_bytes += bytes(&lines);
                lines.reverse();
                level_above.extend(lines);
            }
            level_above.reverse();
            above_levels.push(level_above);

            let mut level_below: Vec<String> = Vec::new();
            for sibling in &siblings[index + 1..] {
                if below_bytes >= limits.below_bytes || collector.out_of_time() {
                    break;
                }
                let lines = collector.collect(sibling, Direction::Forward);
                below_bytes += bytes(&lines);
                level_below.extend(lines.into_iter().map(|l| l.text));
            }
            below_levels.push(level_below);
        }
        child = parent;
    }

    let mut header = collector.header;
    // Collection order depends on traversal direction, so present the header in the order a
    // reader expects. A label seen twice keeps the occurrence nearest the box.
    header.sort_by_key(|h| header_rank(&h.label));
    Surround {
        // Outer levels come earlier in the document than inner ones.
        above: sides::label(above_levels.into_iter().rev().flatten().collect()),
        below: below_levels.into_iter().flatten().collect(),
        header,
    }
}

/// A bounded forward walk of a whole window. `skip` is a box to leave out (the focused text
/// field), if any.
pub fn whole<N: Node>(root: &N, skip: Option<&N>, limits: Limits) -> Vec<String> {
    let mut collector = Collector {
        limits,
        focused: skip,
        header: Vec::new(),
    };
    sides::label(collector.collect(root, Direction::Forward))
}

/// The lines of `candidates` that are not already in `known`, in order. Used to give the
/// "rest of the window" only what the nearby text did not already include. Comparison ignores
/// case and runs of whitespace.
pub fn lines_not_in(candidates: &[String], known: &[String]) -> Vec<String> {
    let canon = |l: &str| {
        // A speaker mark added for the model is not part of the line being compared.
        let l = l
            .strip_prefix("You: ")
            .or_else(|| l.strip_prefix("Them: "))
            .unwrap_or(l);
        l.split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase()
    };
    let seen: std::collections::HashSet<String> = known.iter().map(|l| canon(l)).collect();
    candidates
        .iter()
        .filter(|l| !seen.contains(&canon(l)))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::rc::Rc;
    use std::time::Duration;

    /// A fake element tree. `Rc` identity stands in for on-screen identity.
    #[derive(Clone)]
    struct Fake(Rc<Data>);

    struct Data {
        role: &'static str,
        text: Option<String>,
        header: Option<HeaderField>,
        children: std::cell::RefCell<Vec<Fake>>,
        parent: std::cell::RefCell<Option<std::rc::Weak<Data>>>,
    }

    impl Fake {
        fn new(role: &'static str, text: Option<&str>) -> Fake {
            Fake(Rc::new(Data {
                role,
                text: text.map(str::to_string),
                header: None,
                children: Default::default(),
                parent: Default::default(),
            }))
        }

        fn text(t: &str) -> Fake {
            Fake::new("AXStaticText", Some(t))
        }

        fn group(children: Vec<Fake>) -> Fake {
            let g = Fake::new("AXGroup", None);
            for c in &children {
                *c.0.parent.borrow_mut() = Some(Rc::downgrade(&g.0));
            }
            *g.0.children.borrow_mut() = children;
            g
        }

        fn with_header(label: &str, value: &str) -> Fake {
            Fake(Rc::new(Data {
                role: "AXTextField",
                text: None,
                header: Some(HeaderField {
                    label: label.into(),
                    value: value.into(),
                }),
                children: Default::default(),
                parent: Default::default(),
            }))
        }
    }

    impl Node for Fake {
        fn role(&self) -> String {
            self.0.role.to_string()
        }
        fn own_text(&self, _role: &str) -> Option<String> {
            self.0.text.clone()
        }
        fn header(&self, _role: &str) -> Option<HeaderField> {
            self.0.header.clone()
        }
        fn children(&self) -> Vec<Fake> {
            self.0.children.borrow().clone()
        }
        fn parent(&self) -> Option<Fake> {
            self.0
                .parent
                .borrow()
                .as_ref()
                .and_then(|w| w.upgrade())
                .map(Fake)
        }
        fn same_as(&self, other: &Fake) -> bool {
            Rc::ptr_eq(&self.0, &other.0)
        }
    }

    fn limits() -> Limits {
        Limits {
            deadline: Instant::now() + Duration::from_secs(60),
            subtree_nodes: 400,
            ancestors: 14,
            above_bytes: 9_000,
            below_bytes: 2_000,
        }
    }

    /// A conversation of `n` messages, then the reply box, as LinkedIn lays it out: one big
    /// list element holding every message, and the composer as its sibling.
    fn conversation(n: usize) -> (Fake, Fake) {
        let messages: Vec<Fake> = (0..n)
            .map(|i| Fake::group(vec![Fake::text(&format!("message {i}"))]))
            .collect();
        let list = Fake::group(messages);
        let field = Fake::new("AXTextArea", None);
        let root = Fake::group(vec![list, field.clone()]);
        // Parent links are weak, so the caller must keep the root alive.
        (field, root)
    }

    #[test]
    fn the_newest_message_survives_a_long_conversation() {
        // 500 messages is far past the 400-node cap. The old forward walk kept messages 0..~130
        // and lost the one the reply is about.
        let (field, _root) = conversation(500);
        let s = surrounding(&field, limits());
        assert_eq!(s.above.last().map(String::as_str), Some("message 499"));
        assert!(
            !s.above.iter().any(|l| l == "message 0"),
            "the oldest message should have been cut, not the newest"
        );
    }

    #[test]
    fn above_text_is_in_document_order_with_the_nearest_line_last() {
        let (field, _root) = conversation(5);
        let s = surrounding(&field, limits());
        assert_eq!(
            s.above,
            [
                "message 0",
                "message 1",
                "message 2",
                "message 3",
                "message 4"
            ]
        );
    }

    #[test]
    fn outer_levels_come_before_inner_levels() {
        let field = Fake::new("AXTextArea", None);
        let inner = Fake::group(vec![Fake::text("inner above"), field.clone()]);
        let _outer = Fake::group(vec![Fake::text("outer above"), inner]);
        let s = surrounding(&field, limits());
        assert_eq!(s.above, ["outer above", "inner above"]);
    }

    #[test]
    fn below_text_is_in_document_order_with_the_nearest_line_first() {
        let field = Fake::new("AXTextArea", None);
        let _root = Fake::group(vec![
            field.clone(),
            Fake::text("first below"),
            Fake::text("second below"),
        ]);
        let s = surrounding(&field, limits());
        assert_eq!(s.below, ["first below", "second below"]);
    }

    #[test]
    fn the_box_itself_is_never_included() {
        let field = Fake::new("AXTextArea", Some("my draft text"));
        let _root = Fake::group(vec![Fake::text("above"), field.clone()]);
        let s = surrounding(&field, limits());
        assert!(!s
            .above
            .iter()
            .chain(s.below.iter())
            .any(|l| l == "my draft text"));
    }

    #[test]
    fn header_fields_are_collected_once_each_and_ordered_to_before_subject() {
        let field = Fake::new("AXTextArea", None);
        let _root = Fake::group(vec![
            Fake::with_header("to", "Nabeel"),
            Fake::with_header("subject", "Intro call"),
            Fake::with_header("to", "someone else"),
            field.clone(),
        ]);
        let s = surrounding(&field, limits());
        assert_eq!(s.header.len(), 2);
        // To comes before Subject whatever order they were found in; a repeated label keeps
        // the occurrence nearest the box.
        assert_eq!(s.header[0].label, "to");
        assert_eq!(s.header[0].value, "someone else");
        assert_eq!(s.header[1].label, "subject");
    }

    #[test]
    fn a_byte_budget_keeps_the_nearest_siblings_not_the_farthest() {
        let field = Fake::new("AXTextArea", None);
        let mut kids: Vec<Fake> = (0..50)
            .map(|i| Fake::text(&format!("old block {i} {}", "x".repeat(100))))
            .collect();
        kids.push(Fake::text("the latest block"));
        kids.push(field.clone());
        let _root = Fake::group(kids);
        let mut l = limits();
        l.above_bytes = 600;
        let s = surrounding(&field, l);
        assert_eq!(s.above.last().map(String::as_str), Some("the latest block"));
        assert!(s.above.len() < 50);
    }

    #[test]
    fn an_expired_deadline_returns_what_it_has_without_hanging() {
        let (field, _root) = conversation(50);
        let mut l = limits();
        l.deadline = Instant::now() - Duration::from_secs(1);
        let s = surrounding(&field, l);
        assert!(s.above.is_empty());
    }

    #[test]
    fn lines_not_in_removes_what_was_already_collected_ignoring_case_and_spacing() {
        let candidates = vec![
            "Sidebar".to_string(),
            "Hello  World".to_string(),
            "Footer".to_string(),
        ];
        let known = vec!["hello world".to_string()];
        assert_eq!(lines_not_in(&candidates, &known), ["Sidebar", "Footer"]);
        assert_eq!(lines_not_in(&candidates, &[]), candidates);
    }

    #[test]
    fn a_speaker_mark_does_not_hide_a_duplicate() {
        let candidates = vec!["Hello".to_string(), "Other".to_string()];
        let known = vec!["You: hello".to_string()];
        assert_eq!(lines_not_in(&candidates, &known), ["Other"]);
    }

    #[test]
    fn a_box_with_no_parent_yields_nothing() {
        let field = Fake::new("AXTextArea", None);
        assert_eq!(surrounding(&field, limits()), Surround::default());
    }

    #[test]
    fn whole_window_walk_is_forward_and_skips_the_box() {
        let field = Fake::new("AXTextArea", Some("draft"));
        let root = Fake::group(vec![Fake::text("a"), field.clone(), Fake::text("b")]);
        assert_eq!(whole(&root, Some(&field), limits()), ["a", "b"]);
        assert_eq!(whole(&root, None, limits()), ["a", "draft", "b"]);
    }
}
