//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Tests for [`super`] (the surrounding-text walk), against a fake element tree.

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
    /// A short label a non-text leaf carries (a sender's name in a button).
    label: Option<String>,
    /// Lies wholly outside the conversation column.
    outside: bool,
    children: std::cell::RefCell<Vec<Fake>>,
    parent: std::cell::RefCell<Option<std::rc::Weak<Data>>>,
}

impl Fake {
    fn build(
        role: &'static str,
        text: Option<&str>,
        header: Option<HeaderField>,
        label: Option<&str>,
        outside: bool,
    ) -> Fake {
        Fake(Rc::new(Data {
            role,
            text: text.map(str::to_string),
            header,
            label: label.map(str::to_string),
            outside,
            children: Default::default(),
            parent: Default::default(),
        }))
    }

    fn new(role: &'static str, text: Option<&str>) -> Fake {
        Fake::build(role, text, None, None, false)
    }

    fn text(t: &str) -> Fake {
        Fake::new("AXStaticText", Some(t))
    }

    /// A leaf that is not text but carries a label, like a name in a button.
    fn button(label: &str) -> Fake {
        Fake::build("AXButton", None, None, Some(label), false)
    }

    /// Text that lies wholly outside the conversation column (a sidebar row).
    fn sidebar(t: &str) -> Fake {
        Fake::build("AXStaticText", Some(t), None, None, true)
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
        let header = HeaderField {
            label: label.into(),
            value: value.into(),
        };
        Fake::build("AXTextField", None, Some(header), None, false)
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
    fn label_text(&self, _role: &str) -> Option<String> {
        self.0.label.clone()
    }
    fn placement(&self) -> Placement {
        Placement {
            side: None,
            outside: self.0.outside,
        }
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

/// A message list the way Slack lays it out: a name, its time, then the text.
fn thread() -> (Fake, Fake) {
    let list = Fake::group(vec![
        Fake::group(vec![
            Fake::button("Adithya Harish"),
            Fake::text("3:09 PM"),
            Fake::text("joined Slack"),
        ]),
        Fake::group(vec![
            Fake::button("Akarsh Hegde"),
            Fake::text("3:15 PM"),
            Fake::text("Can you look at the plan?"),
        ]),
    ]);
    let field = Fake::new("AXTextArea", None);
    let root = Fake::group(vec![list, field.clone()]);
    (field, root)
}

#[test]
fn a_name_in_a_button_before_a_time_reaches_the_prompt_text() {
    let (field, _root) = thread();
    let s = surrounding(&field, limits());
    assert_eq!(
        s.above,
        [
            "Adithya Harish",
            "3:09 PM",
            "joined Slack",
            "Akarsh Hegde",
            "3:15 PM",
            "Can you look at the plan?"
        ]
    );
}

#[test]
fn a_button_that_is_not_followed_by_a_time_is_left_out() {
    let list = Fake::group(vec![
        Fake::text("Can you look at the plan?"),
        Fake::button("Reply in thread"),
        Fake::button("Add reaction"),
    ]);
    let field = Fake::new("AXTextArea", None);
    let _root = Fake::group(vec![list, field.clone()]);
    let s = surrounding(&field, limits());
    assert_eq!(s.above, ["Can you look at the plan?"]);
}

#[test]
fn a_button_with_children_is_not_taken_as_a_label() {
    // Only a leaf can offer its label; a container's own title is not a sender name.
    let named = Fake::button("Akarsh Hegde");
    let container = Fake::group(vec![Fake::text("hello there"), named]);
    let times = Fake::group(vec![container, Fake::text("3:15 PM")]);
    let field = Fake::new("AXTextArea", None);
    let _root = Fake::group(vec![times, field.clone()]);
    let s = surrounding(&field, limits());
    // The leaf button sits before "3:15 PM" at the end of the container, so it counts; the
    // container group itself (which has children and no label) must not add a line.
    assert_eq!(s.above, ["hello there", "Akarsh Hegde", "3:15 PM"]);
}

#[test]
fn sidebar_text_is_left_out_of_the_text_above() {
    let sidebar = Fake::group(vec![Fake::sidebar("Mum"), Fake::sidebar("Yesterday")]);
    let chat = Fake::group(vec![Fake::text(
        "Are we still on for Thursday evening at the usual place?",
    )]);
    let field = Fake::new("AXTextArea", None);
    let _root = Fake::group(vec![sidebar, chat, field.clone()]);
    let s = surrounding(&field, limits());
    assert_eq!(
        s.above,
        ["Are we still on for Thursday evening at the usual place?"]
    );
}

#[test]
fn sidebar_text_is_left_out_of_the_text_below_too() {
    let field = Fake::new("AXTextArea", None);
    let below = Fake::group(vec![Fake::sidebar("Daddy"), Fake::text("Send")]);
    let _root = Fake::group(vec![field.clone(), below]);
    let s = surrounding(&field, limits());
    assert_eq!(s.below, ["Send"]);
}
