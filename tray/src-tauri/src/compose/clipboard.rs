//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Borrowing the clipboard for a paste without losing what the user had on it.
//!
//! The paste path puts the draft on the system clipboard and sends a paste keystroke. That
//! must be invisible: afterwards the clipboard holds exactly what it held before, every
//! type of every item. A user who had copied an image must get the image back, not its alt
//! text, so this saves and restores the raw data of every type rather than just the string.
//!
//! The restore is skipped when the clipboard has changed since our write (the user copied
//! something in the meantime): their newer copy always wins over our tidy-up.
//!
//! # Who calls this
//! [`super::writer`]'s paste path, and the controller when a draft must be left on the
//! clipboard because it could not be written safely.
//!
//! # Related
//! - [`super::writer`] - decides when a paste is needed.

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_app_kit::{NSPasteboard, NSPasteboardItem, NSPasteboardTypeString, NSPasteboardWriting};
use objc2_foundation::{NSArray, NSData, NSString};

/// One clipboard item: each of its types with the raw data under it.
type SavedItem = Vec<(Retained<NSString>, Retained<NSData>)>;

/// A snapshot of the clipboard taken before we borrowed it.
pub struct Saved {
    items: Vec<SavedItem>,
}

// SAFETY: the saved values are immutable Foundation objects, only ever touched from the one
// worker thread that created and restores them.
unsafe impl Send for Saved {}

/// Save every type of every item currently on the clipboard.
pub fn save() -> Saved {
    let board = NSPasteboard::generalPasteboard();
    let mut items = Vec::new();
    if let Some(existing) = board.pasteboardItems() {
        for item in existing.iter() {
            let mut saved: SavedItem = Vec::new();
            for ty in item.types().iter() {
                if let Some(data) = item.dataForType(&ty) {
                    saved.push((ty, data));
                }
            }
            if !saved.is_empty() {
                items.push(saved);
            }
        }
    }
    Saved { items }
}

/// Marks clipboard content that clipboard managers should not record (the convention from
/// nspasteboard.org, honoured by the common managers).
const TRANSIENT_TYPE: &str = "org.nspasteboard.TransientType";

/// The clipboard's current change count.
pub fn change_count() -> isize {
    NSPasteboard::generalPasteboard().changeCount()
}

/// Put plain text on the clipboard, marked transient so clipboard managers skip it. Returns the
/// change count right after, which [`restore`] uses to tell whether anyone else has written
/// since, or `None` when the clipboard refused the text (the caller must then not paste).
pub fn set_text(text: &str) -> Option<isize> {
    let board = NSPasteboard::generalPasteboard();
    board.clearContents();
    let value = NSString::from_str(text);
    // SAFETY: NSPasteboardTypeString is a valid, immortal constant.
    let ty = unsafe { NSPasteboardTypeString };
    if !board.setString_forType(&value, ty) {
        return None;
    }
    // Best effort: a manager that ignores the marker simply records the draft.
    board.setString_forType(&NSString::new(), &NSString::from_str(TRANSIENT_TYPE));
    Some(board.changeCount())
}

/// Put the saved items back, unless the clipboard has changed since `expected_change_count`.
/// Returns whether anything was restored.
pub fn restore(saved: Saved, expected_change_count: isize) -> bool {
    let board = NSPasteboard::generalPasteboard();
    if board.changeCount() != expected_change_count {
        return false;
    }
    board.clearContents();
    if saved.items.is_empty() {
        return true;
    }
    let mut objects: Vec<Retained<ProtocolObject<dyn NSPasteboardWriting>>> = Vec::new();
    for item in saved.items {
        let fresh = NSPasteboardItem::new();
        for (ty, data) in item {
            fresh.setData_forType(&data, &ty);
        }
        objects.push(ProtocolObject::from_retained(fresh));
    }
    let array = NSArray::from_retained_slice(&objects);
    let restored = board.writeObjects(&array);
    if !restored {
        tracing::warn!("compose: could not give the clipboard back");
    }
    restored
}
