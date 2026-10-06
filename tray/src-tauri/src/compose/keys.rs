//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Synthetic key presses sent to the app that has focus.
//!
//! Three uses: Command+V and Command+A for the paste rungs of [`super::writer`], and plain
//! full stops and Backspace for the typing dots of [`super::dots`]. Events are posted at the
//! HID level with only the flags given, so a modifier the user is still holding never leaks into
//! them.
//!
//! # Related
//! - [`super::writer`] - the paste rungs.
//! - [`super::dots`] - the typing animation.

use core_graphics::event::{CGEvent, CGEventFlags, CGEventTapLocation};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};

/// Virtual key code for A.
pub const KEY_A: u16 = 0;
/// Virtual key code for V.
pub const KEY_V: u16 = 9;
/// Virtual key code for the period key.
pub const KEY_PERIOD: u16 = 47;
/// Virtual key code for Backspace (the delete key above Return).
pub const KEY_BACKSPACE: u16 = 51;

fn post(key: u16, flags: CGEventFlags, text: Option<&str>) -> bool {
    let Ok(source) = CGEventSource::new(CGEventSourceStateID::HIDSystemState) else {
        return false;
    };
    let (Ok(down), Ok(up)) = (
        CGEvent::new_keyboard_event(source.clone(), key, true),
        CGEvent::new_keyboard_event(source, key, false),
    ) else {
        return false;
    };
    if let Some(text) = text {
        down.set_string(text);
        up.set_string(text);
    }
    down.set_flags(flags);
    up.set_flags(flags);
    down.post(CGEventTapLocation::HID);
    up.post(CGEventTapLocation::HID);
    true
}

/// Post Command + `key`.
pub fn press_command(key: u16) -> bool {
    post(key, CGEventFlags::CGEventFlagCommand, None)
}

/// Type a full stop as if it came from the keyboard (the period key, with the character
/// attached so apps that read either the key code or the text agree).
pub fn type_period() -> bool {
    post(KEY_PERIOD, CGEventFlags::empty(), Some("."))
}

/// Press Backspace once.
pub fn backspace() -> bool {
    post(KEY_BACKSPACE, CGEventFlags::empty(), None)
}
