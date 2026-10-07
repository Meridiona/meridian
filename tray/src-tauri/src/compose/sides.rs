//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Telling the user's own messages from the other person's when the chat prints no names.
//!
//! Chat apps such as Messages, WhatsApp and Telegram show your messages on the right and
//! theirs on the left, and say nothing about who wrote what. The text alone then reads as one
//! anonymous list, and the model answers the user's own question as if it had been asked of
//! them. The one thing the screen does carry is where each message sits, so that is what is
//! used: a message that starts in the right half of the window and runs to its right edge is
//! the user's.
//!
//! # Only when the layout shows it
//! Labels are added only if at least one message sits on the user's side. In a left-aligned
//! layout (Slack, email) nothing is labelled and the text goes through unchanged.
//!
//! # Who calls this
//! [`super::walk`] labels the text above the box; [`super::element_tree`] measures each line.
//!
//! # Related
//! - `assets/prompts/compose/core.md` - tells the model what "You:" and "Them:" mean.

/// Which side of a conversation a message sits on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    User,
    Other,
}

/// A rectangle `(x, y, width, height)` in points.
pub type Frame = (f64, f64, f64, f64);

/// A line wider than this share of the window is a divider or a full-width row, not a bubble.
const BUBBLE_MAX_SHARE: f64 = 0.6;
/// A user's bubble ends within this share of the window's width from its right edge.
const RIGHT_EDGE_SHARE: f64 = 0.9;

/// Which side one line of text sits on, or `None` when it is not a chat bubble.
pub fn side_of(text: Frame, pane: Frame) -> Option<Side> {
    let (tx, _, tw, th) = text;
    let (px, _, pw, _) = pane;
    if pw <= 0.0 || tw <= 0.0 || th <= 0.0 || tw > pw * BUBBLE_MAX_SHARE {
        return None;
    }
    let right = tx + tw;
    if tx > px + pw * 0.5 && right >= px + pw * RIGHT_EDGE_SHARE {
        Some(Side::User)
    } else {
        Some(Side::Other)
    }
}

/// True when `description` marks `text` as the user's own message: it starts with "Your"
/// (Messages writes "Your iMessage, <text>, <time>") and contains the text.
pub fn is_own_description(description: &str, text: &str) -> bool {
    let d = description.trim_start();
    d.get(..5).is_some_and(|p| p.eq_ignore_ascii_case("your "))
        && !text.is_empty()
        && d.contains(text)
}

/// The horizontal extent of the conversation, taken from the composer row: the ancestors of
/// the text box that are still only about one box tall (the row holding the box and its
/// buttons). It spans the chat column even when that column is narrower than the window, or
/// centred in it. Falls back to `window` when no wider row is found.
pub fn conversation_pane(
    field: Frame,
    ancestors: &[Frame],
    window: Option<Frame>,
) -> Option<Frame> {
    let (_, _, fw, fh) = field;
    let mut best: Option<Frame> = None;
    for &(x, y, w, h) in ancestors {
        let wraps = x <= field.0 + 1.0 && x + w >= field.0 + fw - 1.0;
        if wraps && h <= fh * 2.0 + 16.0 {
            if best.is_none_or(|(_, _, bw, _)| w > bw) {
                best = Some((x, y, w, h));
            }
        } else {
            break;
        }
    }
    match (best, window) {
        (Some(row), _) if row.2 > fw * 1.1 => Some(row),
        (_, window) => window,
    }
}

/// Where one element sits relative to the conversation: its side, and whether it lies wholly
/// outside the conversation column (a sidebar, a list of other chats).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Placement {
    pub side: Option<Side>,
    pub outside: bool,
}

/// The conversation column must be at least this wide to be trusted as a filter.
const MIN_PANE_WIDTH: f64 = 240.0;
/// A line counts as outside only if it ends this far before the column starts, or starts this
/// far after it ends. A line that overlaps the column even slightly stays.
const OUTSIDE_SLACK: f64 = 4.0;

/// True when `text` lies wholly to the left or right of the conversation column `pane`. An
/// element with no size, or a pane too narrow to be a real column, is never outside.
pub fn is_outside(text: Frame, pane: Frame) -> bool {
    let (tx, _, tw, th) = text;
    let (px, _, pw, _) = pane;
    if tw <= 0.0 || th <= 0.0 || pw < MIN_PANE_WIDTH {
        return false;
    }
    tx + tw <= px + OUTSIDE_SLACK || tx >= px + pw - OUTSIDE_SLACK
}

/// At least this much text must be left after dropping what lies outside the column;
/// otherwise the column was probably misjudged and nothing is dropped.
const MIN_KEPT_CHARS: usize = 40;

/// Drop the lines that lie wholly outside the conversation column. Falls back to every line
/// when dropping them would leave next to nothing, so a wrongly found column cannot empty the
/// context.
fn keep_in_pane(lines: Vec<Line>) -> Vec<Line> {
    let kept: usize = lines
        .iter()
        .filter(|l| !l.outside)
        .map(|l| l.text.len())
        .sum();
    if kept >= MIN_KEPT_CHARS || !lines.iter().any(|l| l.outside) {
        lines.into_iter().filter(|l| !l.outside).collect()
    } else {
        lines
    }
}

/// A line of text and the side it sits on, if known.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub text: String,
    pub side: Option<Side>,
    /// A short label from an element that is not text (a button, a group). Kept only when a
    /// timestamp follows it, which is how a message header (sender, then time) reads.
    pub candidate: bool,
    /// True when the line lies wholly outside the conversation column.
    pub outside: bool,
}

/// True for a short line that is a clock time with at most a few words around it ("3:09 PM",
/// "Yesterday at 3:09:04 PM", "14:32 Oct 6"). The test is structural, so it holds in any
/// language: a clock, and almost no words. A message that mentions a time ("Sure, 3:30 works
/// for me") has too many words to count.
fn looks_like_time(text: &str) -> bool {
    let t = text.trim();
    if t.len() > 40 {
        return false;
    }
    let has_clock = t.as_bytes().windows(4).any(|w| {
        w[0].is_ascii_digit() && w[1] == b':' && w[2].is_ascii_digit() && w[3].is_ascii_digit()
    });
    let words = t
        .split_whitespace()
        .filter(|w| w.chars().any(char::is_alphabetic) && !w.chars().any(|c| c.is_ascii_digit()))
        .count();
    has_clock && words <= 3
}

/// Keep a candidate label only when the very next line is a timestamp. Chat apps put the
/// sender's name in a button or link and print the time right after it; every other button on
/// the page ("Reply", "Add reaction", "Send now") is dropped.
fn resolve_candidates(lines: Vec<Line>) -> Vec<Line> {
    let is_time: Vec<bool> = lines
        .iter()
        .map(|l| !l.candidate && looks_like_time(&l.text))
        .collect();
    lines
        .into_iter()
        .enumerate()
        .filter_map(|(i, l)| {
            if !l.candidate {
                Some(l)
            } else if is_time.get(i + 1).copied().unwrap_or(false) {
                Some(Line {
                    candidate: false,
                    ..l
                })
            } else {
                None
            }
        })
        .collect()
}

/// The lines as plain text, with "You: " and "Them: " added where the speaker changes - but
/// only if at least one line is the user's, so a layout that shows no sides is left alone.
pub fn label(lines: Vec<Line>) -> Vec<String> {
    let lines = resolve_candidates(keep_in_pane(lines));
    if !lines.iter().any(|l| l.side == Some(Side::User)) {
        return lines.into_iter().map(|l| l.text).collect();
    }
    let mut current = None;
    lines
        .into_iter()
        .map(|l| match l.side {
            Some(side) if Some(side) != current => {
                current = Some(side);
                let who = if side == Side::User { "You" } else { "Them" };
                format!("{who}: {}", l.text)
            }
            _ => l.text,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const WINDOW: Frame = (0.0, 30.0, 1920.0, 975.0);

    fn line(text: &str, side: Option<Side>) -> Line {
        Line {
            text: text.into(),
            side,
            candidate: false,
            outside: false,
        }
    }

    #[test]
    fn a_right_aligned_bubble_is_the_users() {
        // The real Messages bubble: starts at 1692, ends at 1893 in a 1920 wide window.
        assert_eq!(
            side_of((1692.0, 146.0, 201.0, 30.0), WINDOW),
            Some(Side::User)
        );
    }

    #[test]
    fn a_left_aligned_bubble_is_the_other_persons() {
        assert_eq!(
            side_of((360.0, 200.0, 240.0, 30.0), WINDOW),
            Some(Side::Other)
        );
    }

    #[test]
    fn a_full_width_divider_is_not_a_bubble() {
        assert_eq!(side_of((348.0, 128.0, 1552.0, 13.0), WINDOW), None);
    }

    #[test]
    fn a_short_line_in_the_middle_is_the_other_side() {
        assert_eq!(
            side_of((900.0, 300.0, 200.0, 20.0), WINDOW),
            Some(Side::Other)
        );
    }

    #[test]
    fn a_your_description_that_repeats_the_text_is_the_users() {
        let d = "Your iMessage, Hey, how have you been doing?, 5:42\u{202f}PM";
        assert!(is_own_description(d, "Hey, how have you been doing?"));
        assert!(!is_own_description(d, "something else"));
        assert!(!is_own_description(
            "Akarsh, Hey, how have you been doing?, 5:42 PM",
            "Hey, how have you been doing?"
        ));
        assert!(!is_own_description("Yours truly, hi", "hi"));
    }

    #[test]
    fn the_pane_is_the_composer_row_not_the_whole_window() {
        // A chat column from 400 to 1200 in a 1920 wide window, with a one-line composer.
        let field = (440.0, 900.0, 700.0, 30.0);
        let row = (400.0, 895.0, 800.0, 40.0);
        let window = (0.0, 30.0, 1920.0, 975.0);
        assert_eq!(
            conversation_pane(field, &[row, window], Some(window)),
            Some(row)
        );
        // A bubble at the right end of that column is the user's.
        assert_eq!(side_of((1000.0, 300.0, 190.0, 30.0), row), Some(Side::User));
    }

    #[test]
    fn with_no_wider_row_the_window_is_used() {
        let field = (440.0, 900.0, 700.0, 30.0);
        let window = (0.0, 30.0, 1920.0, 975.0);
        assert_eq!(conversation_pane(field, &[], Some(window)), Some(window));
    }

    #[test]
    fn labels_appear_where_the_speaker_changes() {
        let out = label(vec![
            line("Today 5:42 PM", None),
            line("how are you?", Some(Side::Other)),
            line("fine", Some(Side::User)),
            line("Delivered", Some(Side::User)),
            line("great", Some(Side::Other)),
        ]);
        assert_eq!(
            out,
            [
                "Today 5:42 PM",
                "Them: how are you?",
                "You: fine",
                "Delivered",
                "Them: great"
            ]
        );
    }

    #[test]
    fn a_layout_with_no_user_side_is_left_alone() {
        let out = label(vec![
            line("Akarsh", Some(Side::Other)),
            line("hello", Some(Side::Other)),
        ]);
        assert_eq!(out, ["Akarsh", "hello"]);
    }

    fn candidate(text: &str) -> Line {
        Line {
            text: text.into(),
            side: None,
            candidate: true,
            outside: false,
        }
    }

    #[test]
    fn a_button_label_followed_by_a_time_is_a_sender() {
        let lines = vec![
            candidate("Adithya Harish"),
            line("Yesterday at 3:09:04 PM", None),
            line("joined Slack", None),
            candidate("Akarsh Hegde"),
            line("3:15 PM", None),
            line("Can you look at the plan?", None),
        ];
        assert_eq!(
            label(lines),
            vec![
                "Adithya Harish",
                "Yesterday at 3:09:04 PM",
                "joined Slack",
                "Akarsh Hegde",
                "3:15 PM",
                "Can you look at the plan?"
            ]
        );
    }

    #[test]
    fn other_buttons_are_dropped() {
        let lines = vec![
            candidate("Add reaction"),
            line("Sure, 3:30 works for me", None),
            candidate("Send now"),
            line("Reply in thread", None),
        ];
        assert_eq!(
            label(lines),
            vec!["Sure, 3:30 works for me", "Reply in thread"]
        );
    }

    #[test]
    fn a_long_line_that_mentions_a_time_is_not_a_timestamp() {
        assert!(looks_like_time("3:09 PM"));
        assert!(looks_like_time("Today at 14:32"));
        assert!(looks_like_time("Gestern um 15:09"));
        assert!(!looks_like_time("Sure, 3:30 works for me"));
        assert!(!looks_like_time("Yesterday"));
    }

    fn outside(text: &str) -> Line {
        Line {
            outside: true,
            ..line(text, None)
        }
    }

    const COLUMN: Frame = (400.0, 0.0, 800.0, 900.0);

    #[test]
    fn a_line_wholly_left_of_the_column_is_outside() {
        // A sidebar chat row: x 0..380 against a column starting at 400.
        assert!(is_outside((0.0, 100.0, 380.0, 20.0), COLUMN));
        assert!(is_outside((1210.0, 100.0, 150.0, 20.0), COLUMN));
    }

    #[test]
    fn a_line_that_overlaps_the_column_stays() {
        assert!(!is_outside((350.0, 100.0, 120.0, 20.0), COLUMN));
        assert!(!is_outside((500.0, 100.0, 300.0, 20.0), COLUMN));
        assert!(!is_outside((0.0, 0.0, 0.0, 0.0), COLUMN));
    }

    #[test]
    fn a_narrow_pane_is_never_trusted_as_a_filter() {
        assert!(!is_outside(
            (0.0, 0.0, 50.0, 20.0),
            (400.0, 0.0, 120.0, 900.0)
        ));
    }

    #[test]
    fn sidebar_lines_are_dropped_and_the_conversation_kept() {
        let lines = vec![
            outside("Mum"),
            outside("Yesterday"),
            line(
                "Are we still on for Thursday evening at the usual place?",
                None,
            ),
            line("11:50 am", None),
        ];
        assert_eq!(
            label(lines),
            vec![
                "Are we still on for Thursday evening at the usual place?",
                "11:50 am"
            ]
        );
    }

    #[test]
    fn nothing_is_dropped_when_it_would_leave_next_to_nothing() {
        let lines = vec![outside("Mum"), outside("Daddy"), line("ok", None)];
        assert_eq!(label(lines), vec!["Mum", "Daddy", "ok"]);
    }
}
