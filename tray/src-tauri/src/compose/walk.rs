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

use super::sides::{self, Line, Placement};

/// What the walk needs to know about an element, independent of how it is stored.
pub trait Node: Clone {
    /// The element's role, e.g. `AXStaticText`.
    fn role(&self) -> String;
    /// The text this element contributes, if it is the kind that carries message text.
    fn own_text(&self, role: &str) -> Option<String>;
    /// A compose-header field (`To`, `Subject`) this element represents, if it is one.
    fn header(&self, role: &str) -> Option<HeaderField>;
    /// A short label this element carries although it is not a text element (a sender's name
    /// in a button). The walk keeps it only if a timestamp follows.
    fn label_text(&self, _role: &str) -> Option<String> {
        None
    }
    fn children(&self) -> Vec<Self>;
    fn parent(&self) -> Option<Self>;
    fn same_as(&self, other: &Self) -> bool;
    /// Where this node sits relative to the conversation: its side, and whether it lies wholly
    /// outside the conversation column.
    fn placement(&self) -> Placement {
        Placement::default()
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
            let children = node.children();
            if let Some(text) = node.own_text(&role) {
                let placed = node.placement();
                lines.push(Line {
                    text,
                    side: placed.side,
                    candidate: false,
                    outside: placed.outside,
                });
            } else if children.is_empty() {
                // A leaf that is not text may still carry a label (a name in a button). It is
                // kept later only if a timestamp follows it.
                if let Some(text) = node.label_text(&role) {
                    lines.push(Line {
                        text,
                        side: None,
                        candidate: true,
                        outside: node.placement().outside,
                    });
                }
            }
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
                level_below.extend(
                    lines
                        .into_iter()
                        .filter(|l| !l.candidate && !l.outside)
                        .map(|l| l.text),
                );
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
#[path = "walk_tests.rs"]
mod tests;
