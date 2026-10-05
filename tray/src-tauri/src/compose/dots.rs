//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! The "..." that types itself into the box while a draft is being written.
//!
//! Real full stops are typed at the caret one by one (one, two, three) and taken back out again
//! (Backspace), in a loop, so it looks like someone typing. They are removed, and the box
//! checked to hold exactly what it did before, *before* the draft is written.
//!
//! # Why this is guarded so heavily
//! The box belongs to the user, and these are real keystrokes in their app. So:
//! - **Only a bare caret that was really read.** A highlight would be replaced by the first
//!   full stop, and a guessed caret could put a dot in the wrong place.
//! - **Every step is verified.** Before each key the box is read; it must be exactly the
//!   original text with nothing but full stops (or the single "…" that smart punctuation turns
//!   three into) at the caret. Anything else - the user typed, the app changed it - ends the
//!   animation and nothing more is deleted.
//! - **The caret is re-checked before every key.** It must sit exactly after the dots typed so
//!   far; if the user moved it, nothing more is typed or deleted.
//! - **Only while the field's app is in front.** Keys go to whatever has focus, so the
//!   animation stops the moment another app takes over.
//! - **Cleanup is verify-driven, not counted.** Backspace is sent only while the box still
//!   differs from the original by those dots, so it can never delete the user's own text.
//! - **If cleanup cannot be proven** the caller is told and does not write; the draft goes to
//!   the clipboard instead.
//!
//! # Lifetime
//! [`TypedDots::finish`] stops the animation and cleans up. Dropping the value without calling
//! it (an early return, a panic) stops the animation and cleans up too, so keystrokes can never
//! outlive the press.
//!
//! # Who calls this
//! [`super::controller`]: [`TypedDots::start`] once a press is known to be writable,
//! [`TypedDots::finish`] the moment the model answers.
//!
//! # Related
//! - [`super::keys`] - the key events.
//! - [`super::typing`] - the sound that plays alongside.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use super::ax::Element;
use super::delivery::{canonical_blanks, same_text};
use super::keys;
use super::ranges::{self, Utf16Range};
use super::reader::{self, FieldHandle};

/// Pause between steps of the animation.
const STEP: Duration = Duration::from_millis(380);
/// How often a waiting thread checks whether it has been told to stop.
const POLL: Duration = Duration::from_millis(15);
/// Backspaces tried during cleanup before giving up.
const CLEANUP_STEPS: usize = 8;
/// Time given to an app to apply one key before the box is read again.
const APPLY: Duration = Duration::from_millis(45);

/// What sits between the original text before the caret and the original text after it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Typed {
    /// Characters to Backspace to remove them.
    pub chars: usize,
    /// How many full stops they read as ("…" counts as three).
    pub stops: usize,
}

/// What the box holds beyond `original`, if that is only full stops at the caret.
///
/// `None` means the box differs from the original in any other way, which is the signal to
/// stop touching it. Blank space at the join between the text before the caret and the dots is
/// (or between the dots and the text after the caret) is ignored, because editors normalise it.
pub(crate) fn typed_between(value: &str, original: &str, caret: usize) -> Option<Typed> {
    let (_, before, after) = ranges::split(
        original,
        Utf16Range {
            location: 0,
            length: caret,
        },
    )?;
    let value = canonical_blanks(value);
    let before = canonical_blanks(&before);
    let after = canonical_blanks(&after);
    let middle = value
        .strip_prefix(before.trim_end())?
        .strip_suffix(after.trim_start())?;
    let mut chars = 0;
    let mut stops = 0;
    for c in middle.trim().chars() {
        match c {
            '.' => stops += 1,
            '\u{2026}' => stops += 3,
            _ => return None,
        }
        chars += 1;
    }
    Some(Typed { chars, stops })
}

/// A running animation. Call [`TypedDots::finish`] to stop it and clean up; dropping it does the
/// same, without reporting the result.
pub struct TypedDots {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    element: Element,
    original: String,
    caret: usize,
    pid: i32,
}

/// Wait up to `total`, returning early (true) if told to stop.
fn sleep_unless_stopped(stop: &AtomicBool, total: Duration) -> bool {
    let end = Instant::now() + total;
    while Instant::now() < end {
        if stop.load(Ordering::SeqCst) {
            return true;
        }
        std::thread::sleep(POLL);
    }
    stop.load(Ordering::SeqCst)
}

fn value_of(element: &Element) -> Option<String> {
    reader::visible_value(element)
}

/// True when the box holds only full stops beyond `original` and the caret sits right after
/// them. Returns what was typed. Anything else means the box is no longer ours to touch.
fn typed_and_caret_ok(
    element: &Element,
    original: &str,
    caret: usize,
) -> Result<Typed, &'static str> {
    let value = value_of(element).ok_or("the box stopped reporting its text")?;
    let typed = typed_between(&value, original, caret).ok_or("the box changed in another way")?;
    let expected = Utf16Range {
        location: caret + typed.chars,
        length: 0,
    };
    if element.range("AXSelectedTextRange") != Some(expected) {
        return Err("the caret moved");
    }
    Ok(typed)
}

fn run(stop: Arc<AtomicBool>, element: Element, original: String, caret: usize, pid: i32) {
    while !sleep_unless_stopped(&stop, STEP) {
        // Keys go to whatever has focus; never send them anywhere but the field's own app.
        if reader::frontmost_pid() != Some(pid) {
            return;
        }
        let typed = match typed_and_caret_ok(&element, &original, caret) {
            Ok(t) => t,
            Err(reason) => {
                tracing::info!(reason, "compose: typing dots stopped");
                return;
            }
        };
        if typed.stops >= 3 {
            for _ in 0..typed.chars {
                if stop.load(Ordering::SeqCst) || !keys::backspace() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(25));
            }
        } else if !keys::type_period() {
            return;
        }
    }
}

impl TypedDots {
    /// Start the animation, or `None` when it would not be safe: a highlight, a caret that was
    /// not actually read, an unreadable box, or a box that no longer matches what was read.
    pub fn start(handle: &FieldHandle) -> Option<TypedDots> {
        if handle.selection.length > 0 || !handle.selection_known {
            return None;
        }
        let now = value_of(&handle.element)?;
        if !same_text(&now, &handle.value) || reader::frontmost_pid() != Some(handle.pid) {
            return None;
        }
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let (stop, element, original) =
                (stop.clone(), handle.element.clone(), handle.value.clone());
            let (caret, pid) = (handle.selection.location, handle.pid);
            std::thread::Builder::new()
                .name("compose-typing-dots".into())
                .spawn(move || run(stop, element, original, caret, pid))
                .ok()?
        };
        Some(TypedDots {
            stop,
            thread: Some(thread),
            element: handle.element.clone(),
            original: handle.value.clone(),
            caret: handle.selection.location,
            pid: handle.pid,
        })
    }

    /// Stop the animation and take the dots back out. True when the box is verified to hold
    /// exactly its original text again; false when that could not be proven (the caller must
    /// not write over it).
    pub fn finish(mut self) -> bool {
        self.settle()
    }

    /// Idempotent: stop the thread, then remove whatever dots are in the box.
    fn settle(&mut self) -> bool {
        self.stop.store(true, Ordering::SeqCst);
        let Some(thread) = self.thread.take() else {
            return true;
        };
        let _ = thread.join();
        for _ in 0..CLEANUP_STEPS {
            let typed = match typed_and_caret_ok(&self.element, &self.original, self.caret) {
                Ok(t) => t,
                Err(reason) => {
                    // A box that already equals the original needs nothing, whatever the caret.
                    let clean = value_of(&self.element)
                        .and_then(|v| typed_between(&v, &self.original, self.caret))
                        .is_some_and(|t| t.chars == 0);
                    if !clean {
                        tracing::warn!(reason, "compose: cannot clear the typing dots");
                    }
                    return clean;
                }
            };
            if typed.chars == 0 {
                return true;
            }
            if reader::frontmost_pid() != Some(self.pid) || !keys::backspace() {
                return false;
            }
            std::thread::sleep(APPLY);
        }
        value_of(&self.element)
            .and_then(|v| typed_between(&v, &self.original, self.caret))
            .is_some_and(|t| t.chars == 0)
    }
}

impl Drop for TypedDots {
    fn drop(&mut self) {
        let _ = self.settle();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_untouched_box_has_nothing_typed() {
        assert_eq!(
            typed_between("hello world", "hello world", 5),
            Some(Typed { chars: 0, stops: 0 })
        );
    }

    #[test]
    fn full_stops_at_the_caret_are_counted() {
        assert_eq!(
            typed_between("hello.. world", "hello world", 5),
            Some(Typed { chars: 2, stops: 2 })
        );
    }

    #[test]
    fn dots_after_the_text_in_an_empty_box() {
        assert_eq!(
            typed_between("...", "", 0),
            Some(Typed { chars: 3, stops: 3 })
        );
    }

    #[test]
    fn smart_punctuation_turns_three_dots_into_one_character_worth_three() {
        assert_eq!(
            typed_between("hi\u{2026}", "hi", 2),
            Some(Typed { chars: 1, stops: 3 })
        );
    }

    #[test]
    fn anything_else_between_means_hands_off() {
        assert_eq!(typed_between("hello x world", "hello world", 5), None);
        assert_eq!(typed_between("hello wor", "hello world", 5), None);
        assert_eq!(typed_between("changed", "hello world", 5), None);
    }

    #[test]
    fn dots_in_the_original_text_are_not_counted_as_typed() {
        assert_eq!(
            typed_between("wait... ok", "wait... ok", 7),
            Some(Typed { chars: 0, stops: 0 })
        );
    }

    #[test]
    fn works_with_text_outside_the_basic_plane() {
        // "caf\u{e9} " is 5 UTF-16 units; the caret sits at its end.
        assert_eq!(
            typed_between("caf\u{e9} ..", "caf\u{e9} ", 5),
            Some(Typed { chars: 2, stops: 2 })
        );
    }

    #[test]
    fn a_trailing_space_that_becomes_a_non_breaking_space_is_the_same_text() {
        // The rich-text editor case: "Ann " turns into "Ann\u{a0}." once a dot follows.
        assert_eq!(
            typed_between("Ann\u{a0}.", "Ann ", 4),
            Some(Typed { chars: 1, stops: 1 })
        );
        assert_eq!(
            typed_between("Ann\u{a0}", "Ann ", 4),
            Some(Typed { chars: 0, stops: 0 })
        );
    }

    #[test]
    fn invisible_marks_around_a_tag_are_ignored() {
        assert_eq!(
            typed_between("\u{200b}Ann\u{200b} ..", "Ann ", 4),
            Some(Typed { chars: 2, stops: 2 })
        );
    }

    #[test]
    fn a_space_the_user_typed_after_the_dots_is_not_treated_as_ours() {
        assert_eq!(typed_between("Ann .x", "Ann ", 4), None);
    }
}
