//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Putting a draft into the field it was written for.
//!
//! # The ladder
//! 1. **Accessibility selected-text write.** Select exactly the planned range, then set the
//!    selected text. Surgical, keeps the app's own undo working in native apps.
//! 2. **Paste.** Select the planned range, put the draft on the clipboard, send a paste
//!    keystroke, and give the clipboard back afterwards. This is the path Chromium and
//!    Electron apps need, because they accept the first write and ignore it.
//! 3. **Select all, then paste.** When the exact span cannot be selected but the planned range
//!    is the whole field (apart from blank space), select everything in the focused field and
//!    paste. Rich-text editors in browsers (a LinkedIn comment box) often refuse a precise
//!    selection range yet accept select-all, so this rescues the common "polish my draft" case.
//!
//! Each rung is believed only when the field is read back and shows the expected text
//! ([`super::delivery::landed`]). Chromium applies Accessibility writes a moment late, so
//! the read-back polls for a short window before declaring a rung failed; declaring it
//! failed too early is how a draft gets inserted twice.
//!
//! # What this never does
//! It never writes a partial range it could not select exactly (a paste over the wrong span
//! would duplicate or destroy text), never replaces a field's whole value through Accessibility
//! (that flattens undo and can clobber what the app holds), and never pastes without first
//! being told by [`super::delivery::guard`] that the field is unchanged.
//!
//! # Who calls this
//! [`super::controller`], after the guard has passed.
//!
//! # Related
//! - [`super::delivery`] - the planned range, the guard and the read-back check.
//! - [`super::clipboard`] - the lossless clipboard borrow used by the paste rung.

use std::time::{Duration, Instant};

use super::clipboard;
use super::delivery::{expected_value, landed, value_unchanged};
use super::keys::{self, KEY_A, KEY_V};
use super::ranges::Utf16Range;
use super::reader::{self, FieldHandle};

/// How long to wait for an app to show a write before calling the rung failed.
const LAND_WINDOW: Duration = Duration::from_millis(250);
const LAND_POLL: Duration = Duration::from_millis(25);
/// Extra time given to an Accessibility write that showed nothing in [`LAND_WINDOW`], before it
/// is taken as ignored. Electron and web apps apply these writes asynchronously; pasting while
/// one is still on its way would insert the draft twice.
const LATE_WRITE_GRACE: Duration = Duration::from_millis(450);
/// How long the paste keystroke gets to be consumed before the clipboard is restored.
const PASTE_SETTLE: Duration = Duration::from_millis(700);

/// Apps and kinds of box that were seen to accept an Accessibility write and ignore it. Once an
/// app is known to do that, later writes skip straight to pasting instead of paying the wait
/// again. Keyed by process and role; lives for the life of the app, so a restart re-learns it.
static IGNORES_AX_WRITES: std::sync::OnceLock<
    std::sync::Mutex<std::collections::HashSet<(i32, String)>>,
> = std::sync::OnceLock::new();

fn ignored_set() -> std::sync::MutexGuard<'static, std::collections::HashSet<(i32, String)>> {
    IGNORES_AX_WRITES
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn ignores_ax_writes(pid: i32, role: &str) -> bool {
    ignored_set().contains(&(pid, role.to_string()))
}

fn remember_ignores_ax_writes(pid: i32, role: &str) {
    ignored_set().insert((pid, role.to_string()));
}

/// How a draft ended up in the field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    SelectedText,
    Paste,
    /// Everything in the field was selected with select-all, then replaced by a paste.
    PasteAll,
}

impl Method {
    pub fn as_str(self) -> &'static str {
        match self {
            Method::SelectedText => "ax_selected_text",
            Method::Paste => "paste",
            Method::PasteAll => "paste_select_all",
        }
    }
}

/// Why nothing was written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteError {
    /// The planned range does not fit the text that was read.
    RangeInvalid,
    /// The exact span could not be selected, so replacing it would risk the wrong text.
    CouldNotSelect,
    /// Every rung ran and none was seen to land.
    NotConfirmed,
}

impl WriteError {
    pub fn as_str(self) -> &'static str {
        match self {
            WriteError::RangeInvalid => "range_invalid",
            WriteError::CouldNotSelect => "could_not_select",
            WriteError::NotConfirmed => "not_confirmed",
        }
    }
}

fn wait_for_landing(handle: &FieldHandle, expected: &str) -> bool {
    wait_for_landing_within(handle, expected, LAND_WINDOW)
}

fn wait_for_landing_within(handle: &FieldHandle, expected: &str, window: Duration) -> bool {
    let deadline = Instant::now() + window;
    loop {
        let observed = reader::visible_value(&handle.element);
        if landed(expected, observed.as_deref()) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(LAND_POLL);
    }
}

/// Select exactly `range`, and confirm the field reports exactly that selection.
fn select_exactly(handle: &FieldHandle, range: Utf16Range) -> bool {
    // Already there (a caret where an insert is planned): nothing to ask the app to do, and
    // some editors refuse a redundant set.
    if handle.element.range("AXSelectedTextRange") == Some(range) {
        return true;
    }
    if handle
        .element
        .set_range("AXSelectedTextRange", range)
        .is_err()
    {
        return false;
    }
    handle.element.range("AXSelectedTextRange") == Some(range)
}

/// Write `text` over `range` of the field, trying each rung in order.
#[tracing::instrument(skip_all, fields(method, range_len = range.length))]
pub fn write(
    handle: &FieldHandle,
    range: Utf16Range,
    text: &str,
    on_landed: &mut dyn FnMut(),
) -> Result<Method, WriteError> {
    let expected = expected_value(&handle.value, range, text).ok_or(WriteError::RangeInvalid)?;

    // Rung 1: select the span, set the selected text, and watch for it to land. Skipped for an
    // app already seen to ignore these writes.
    let skip_ax = ignores_ax_writes(handle.pid, &handle.identity.role);
    if !skip_ax
        && select_exactly(handle, range)
        && handle.element.set_string("AXSelectedText", text).is_ok()
    {
        // The write may land late: wait the normal window, then a grace period, before
        // deciding the app ignored it. Only then is a paste safe.
        if wait_for_landing(handle, &expected)
            || wait_for_landing_within(handle, &expected, LATE_WRITE_GRACE)
        {
            on_landed();
            tracing::Span::current().record("method", Method::SelectedText.as_str());
            return Ok(Method::SelectedText);
        }
        // Do NOT fall through if the value moved at all: a late write plus a paste would
        // insert twice.
        if !value_unchanged(
            &handle.value,
            reader::visible_value(&handle.element).as_deref(),
        ) {
            tracing::warn!(
                "compose: selected-text write changed the field but not as expected; stopping"
            );
            return Err(WriteError::NotConfirmed);
        }
        // Accepted and ignored: remember, so the next write in this app goes straight to paste.
        tracing::info!("compose: this app ignores selected-text writes; pasting from now on");
        remember_ignores_ax_writes(handle.pid, &handle.identity.role);
    }

    // Rung 2: paste over a span we can select exactly.
    if select_exactly(handle, range) {
        return paste_and_confirm(handle, text, &expected, &[KEY_V], Method::Paste, on_landed);
    }

    // Rung 3: the app refused the exact span. If the plan is the whole field, select-all is
    // an unambiguous way to say the same thing.
    // Never while a tag the app put there is protected: select-all would flatten it to text.
    if handle.protected_prefix == 0 && covers_whole_field(&handle.value, range) {
        let whole = Utf16Range {
            location: 0,
            length: handle.value.encode_utf16().count(),
        };
        if let Some(expected_all) = expected_value(&handle.value, whole, text) {
            tracing::info!("compose: exact selection refused, replacing the whole field");
            return paste_and_confirm(
                handle,
                text,
                &expected_all,
                &[KEY_A, KEY_V],
                Method::PasteAll,
                on_landed,
            );
        }
    }
    Err(WriteError::CouldNotSelect)
}

/// True when everything outside `range` is blank, so replacing `range` is the same as
/// replacing the whole field.
pub(crate) fn covers_whole_field(value: &str, range: Utf16Range) -> bool {
    expected_value(value, range, "").is_some_and(|rest| rest.trim().is_empty())
}

/// Put `text` on the clipboard, send each Command+key in `keys` in order, wait for the field
/// to show `expected`, and give the clipboard back.
fn paste_and_confirm(
    handle: &FieldHandle,
    text: &str,
    expected: &str,
    keys: &[u16],
    method: Method,
    on_landed: &mut dyn FnMut(),
) -> Result<Method, WriteError> {
    let saved = clipboard::save();
    let Some(ours) = clipboard::set_text(text) else {
        // Never paste what is not ours: the clipboard still holds whatever was there before.
        clipboard::restore(saved, clipboard::change_count());
        return Err(WriteError::NotConfirmed);
    };
    // The keys go to whatever has focus: check it is still our box, immediately before sending.
    if !reader::is_focused(&handle.element, handle.pid) {
        clipboard::restore(saved, ours);
        return Err(WriteError::CouldNotSelect);
    }
    let mut pressed = true;
    for key in keys {
        pressed = pressed && keys::press_command(*key);
        // Select-all must be taken before the paste that follows it.
        if keys.len() > 1 {
            std::thread::sleep(Duration::from_millis(60));
        }
    }
    let confirmed = pressed && wait_for_landing(handle, expected);
    // The user can see the text now. Tell the caller before the clipboard settle below, so
    // the sound and the dots stop when the text appears, not 0.7 s later.
    if confirmed {
        on_landed();
    }
    // Let the target app read the clipboard before it is given back.
    std::thread::sleep(PASTE_SETTLE);
    clipboard::restore(saved, ours);

    if confirmed {
        tracing::Span::current().record("method", method.as_str());
        Ok(method)
    } else {
        Err(WriteError::NotConfirmed)
    }
}

/// Leave the draft on the clipboard for the user to paste themselves. Used when it could
/// not be written safely; their previous clipboard is deliberately not restored, because
/// the draft is the thing they need.
pub fn leave_on_clipboard(text: &str) {
    if clipboard::set_text(text).is_none() {
        tracing::warn!("compose: could not leave the draft on the clipboard");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(location: usize, length: usize) -> Utf16Range {
        Utf16Range { location, length }
    }

    #[test]
    fn a_draft_with_a_blank_tail_covers_the_whole_field() {
        // The LinkedIn case: 17 characters read, the draft is the first 16.
        assert!(covers_whole_field("polish this one\n", r(0, 15)));
        assert!(covers_whole_field("whole thing", r(0, 11)));
    }

    #[test]
    fn a_signature_after_the_draft_is_not_covered() {
        assert!(!covers_whole_field("draft\n\nBest,\nAdithya", r(0, 5)));
    }

    #[test]
    fn text_before_the_range_is_not_covered() {
        assert!(!covers_whole_field("Notes: highlighted", r(7, 11)));
    }

    #[test]
    fn an_insert_into_an_empty_field_covers_it() {
        assert!(covers_whole_field("", r(0, 0)));
    }

    #[test]
    fn an_app_seen_ignoring_ax_writes_is_remembered_per_role() {
        // A pid no real test uses.
        let (pid, role) = (987_654, "AXTextArea");
        assert!(!ignores_ax_writes(pid, role));
        remember_ignores_ax_writes(pid, role);
        assert!(ignores_ax_writes(pid, role));
        assert!(!ignores_ax_writes(pid, "AXTextField"));
        assert!(!ignores_ax_writes(pid + 1, role));
    }
}
