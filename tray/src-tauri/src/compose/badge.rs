//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! A small Meridian icon beside the focused text box, whose click does what the writing key does.
//!
//! # How it stays out of the way
//! The icon lives in its own non-activating panel (the `compose-badge` window), so clicking
//! it never takes focus from the text box: the box keeps its caret and the draft still has
//! somewhere to land. A poller follows the focused field every [`POLL`], shows the icon at
//! the field's left edge, and hides it again when focus is not in a text box, in a
//! password field, in Meridian itself, or when the writing key is switched off.
//!
//! # Who calls this
//! [`super::controller::start`] starts the poller; the badge's webview calls the commands in
//! [`crate::commands::compose_badge`] on mouse down.
//!
//! # Related
//! - [`super::controller::press`] - the shared entry point, identical to a key tap.
//! - [`crate::window_panel`] - makes the window a non-activating panel over full-screen apps.

use std::time::{Duration, Instant};

use tauri::{Emitter, LogicalPosition, Manager};

use super::ax::{self, Element};
use super::element_tree::TEXT_ROLES;
use super::reader;

/// How often the focused field is checked.
const POLL: Duration = Duration::from_millis(350);
/// How often the on/off setting is re-read.
const SETTINGS_POLL: Duration = Duration::from_secs(2);
/// Edge of the square badge window, in points.
const SIZE: f64 = 22.0;
/// Gap kept between the badge and the field's edge.
const MARGIN: f64 = 6.0;
/// Fields smaller than this are not worth marking (a tiny search or rename box).
const MIN_WIDTH: f64 = 120.0;
const MIN_HEIGHT: f64 = 20.0;
/// A field bigger than this is no real text box, whatever the window (a huge display).
const MAX_AREA: f64 = 3_000_000.0;
/// A field covering more than this share of its window is the window's editor surface
/// (a document or code editor), not a box to write a message in.
const MAX_WINDOW_SHARE: f64 = 0.6;

/// A field's frame on screen: `(x, y, width, height)` in points, top-left origin.
pub type Frame = (f64, f64, f64, f64);

/// True when a field is a sensible place to show the badge.
///
/// `window` is the frame of the window holding the field, when it could be read: a field that
/// fills most of it is the editor surface itself and is not marked.
pub fn worth_marking((_, _, w, h): Frame, window: Option<Frame>) -> bool {
    let fills_window = window
        .is_some_and(|(_, _, ww, wh)| ww > 0.0 && wh > 0.0 && w * h > ww * wh * MAX_WINDOW_SHARE);
    w >= MIN_WIDTH && h >= MIN_HEIGHT && w * h <= MAX_AREA && !fills_window
}

/// The visible box around a field. The Accessibility frame of a text field is often only its
/// inner input, with the rounded border, the attach button and the emoji button drawn by the
/// ancestors, so the badge would land on top of them. This walks up through the ancestors that
/// still wrap the field closely (a little taller, a little wider) and returns the outermost.
pub fn visible_box(field: Frame, ancestors: &[Frame]) -> Frame {
    let (fx, fy, fw, fh) = field;
    let mut best = field;
    for &(x, y, w, h) in ancestors {
        let contains =
            x <= fx + 1.0 && y <= fy + 1.0 && x + w >= fx + fw - 1.0 && y + h >= fy + fh - 1.0;
        if contains && h <= fh * 2.0 + 16.0 && w <= fw * 1.8 + 120.0 {
            best = (x, y, w, h);
        } else {
            break;
        }
    }
    best
}

/// Top-left of the badge for a box: just above its top-left corner, in the free space over
/// the box. Beside the box it would land on a sidebar divider, an attach button or the text
/// next to a form field, and for a tall field such as a mail body the bottom can be off
/// screen; the strip above the corner is clear in all of them.
pub fn badge_position((x, y, _w, _h): Frame) -> (f64, f64) {
    (x.max(0.0), (y - SIZE - MARGIN).max(0.0))
}

/// Start following the focused field. Does nothing until the writing key is switched on.
pub fn start(app: tauri::AppHandle) {
    if let Err(e) = std::thread::Builder::new()
        .name("compose-badge".into())
        .spawn(move || run(app))
    {
        tracing::warn!(error = %e, "compose: could not start the badge thread");
    }
}

fn run(app: tauri::AppHandle) {
    let own_pid = std::process::id() as i32;
    let mut enabled = false;
    let mut enabled_checked: Option<Instant> = None;
    let mut shown_at: Option<(i64, i64)> = None;
    let mut was_working = false;
    loop {
        std::thread::sleep(POLL);
        // Tell the icon's page to spin while a draft is being written.
        let working = super::controller::is_working();
        if working != was_working {
            was_working = working;
            let _ = app.emit_to("compose-badge", "compose-busy", working);
        }
        if enabled_checked.is_none_or(|t| t.elapsed() >= SETTINGS_POLL) {
            enabled = meridian_core::settings::load_runtime_settings().compose_enabled;
            enabled_checked = Some(Instant::now());
        }
        let target = if enabled { target_for(own_pid) } else { None };
        let place = target.map(badge_position);
        let key = place.map(|(x, y)| (x.round() as i64, y.round() as i64));
        if key == shown_at {
            continue;
        }
        shown_at = key;
        let handle = app.clone();
        let _ = app.run_on_main_thread(move || apply(&handle, place));
    }
}

/// The focused text field's frame, when it should carry the badge.
fn target_for(own_pid: i32) -> Option<Frame> {
    if !ax::is_trusted() || reader::secure_input_active() {
        return None;
    }
    let pid = reader::frontmost_pid().filter(|pid| *pid != own_pid)?;
    let app = Element::application(pid)?;
    let element = reader::focused_element(pid, &app)?;
    let role = element.string("AXRole")?;
    if !TEXT_ROLES.contains(&role.as_str()) {
        return None;
    }
    if element.string("AXSubrole").as_deref() == Some("AXSecureTextField") {
        return None;
    }
    let window = app.element("AXFocusedWindow").and_then(|w| w.frame());
    let field = element.frame().filter(|f| worth_marking(*f, window))?;
    let mut ancestors = Vec::new();
    let mut current = element;
    for _ in 0..6 {
        let Some(parent) = current.element("AXParent") else {
            break;
        };
        match parent.frame() {
            Some(f) => ancestors.push(f),
            None => break,
        }
        current = parent;
    }
    Some(visible_box(field, &ancestors))
}

/// Move and show the badge, or hide it. Runs on the main thread.
fn apply(app: &tauri::AppHandle, place: Option<(f64, f64)>) {
    let Some(win) = app.get_webview_window("compose-badge") else {
        return;
    };
    match place {
        Some((x, y)) => {
            let _ = win.set_position(LogicalPosition::new(x, y));
            crate::window_panel::show_no_focus(&win);
        }
        None => {
            let _ = win.hide();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_badge_sits_above_the_boxs_top_left_corner() {
        assert_eq!(
            badge_position((100.0, 200.0, 400.0, 120.0)),
            (100.0, 200.0 - 22.0 - 6.0)
        );
        assert_eq!(
            badge_position((100.0, 200.0, 400.0, 32.0)),
            (100.0, 200.0 - 22.0 - 6.0)
        );
    }

    #[test]
    fn the_badge_never_leaves_the_screen() {
        assert_eq!(badge_position((-5.0, 10.0, 400.0, 32.0)), (0.0, 0.0));
    }

    #[test]
    fn the_badge_goes_around_the_visible_box_not_the_inner_input() {
        // Messages: the input sits inside a wider row holding the + and emoji buttons.
        let field = (386.0, 963.0, 1447.0, 31.0);
        let row = (328.0, 963.0, 1592.0, 42.0);
        assert_eq!(visible_box(field, &[row]), row);
    }

    #[test]
    fn the_walk_up_stops_at_an_ancestor_that_is_much_bigger() {
        let field = (386.0, 963.0, 600.0, 31.0);
        let row = (360.0, 955.0, 700.0, 44.0);
        let pane = (0.0, 30.0, 1920.0, 975.0);
        assert_eq!(visible_box(field, &[row, pane]), row);
        assert_eq!(visible_box(field, &[pane]), field);
    }

    #[test]
    fn tiny_and_enormous_fields_are_not_marked() {
        assert!(!worth_marking((0.0, 0.0, 60.0, 24.0), None));
        assert!(!worth_marking((0.0, 0.0, 300.0, 10.0), None));
        assert!(!worth_marking((0.0, 0.0, 2500.0, 1400.0), None));
        assert!(worth_marking((0.0, 0.0, 600.0, 80.0), None));
    }

    #[test]
    fn a_field_filling_its_window_is_not_marked() {
        let window = Some((0.0, 0.0, 1440.0, 900.0));
        // A 1440 x 900 editor is under the absolute cap but is the whole window.
        assert!(!worth_marking((0.0, 0.0, 1440.0, 900.0), window));
        assert!(!worth_marking((0.0, 40.0, 1200.0, 800.0), window));
        // A message box in the same window is fine.
        assert!(worth_marking((300.0, 800.0, 900.0, 60.0), window));
    }
}
