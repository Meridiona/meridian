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
//! [`super::controller::start`] starts the poller; the badge's webview calls
//! [`badge_click`] on mouse down.
//!
//! # Related
//! - [`super::controller::press`] - the shared entry point, identical to a key tap.
//! - [`crate::window_panel`] - makes the window a non-activating panel over full-screen apps.

use std::time::{Duration, Instant};

use tauri::{LogicalPosition, Manager};

use super::ax::{self, Element};
use super::element_tree::TEXT_ROLES;
use super::reader;

/// How often the focused field is checked.
const POLL: Duration = Duration::from_millis(350);
/// How often the on/off setting is re-read.
const SETTINGS_POLL: Duration = Duration::from_secs(2);
/// Edge of the square badge window, in points.
const SIZE: f64 = 28.0;
/// Gap kept between the badge and the field's edge.
const MARGIN: f64 = 4.0;
/// Fields smaller than this are not worth marking (a tiny search or rename box).
const MIN_WIDTH: f64 = 120.0;
const MIN_HEIGHT: f64 = 20.0;
/// Where a field is bigger than any real text box (a whole-window editor surface).
const MAX_AREA: f64 = 3_000_000.0;

/// A field's frame on screen: `(x, y, width, height)` in points, top-left origin.
pub type Frame = (f64, f64, f64, f64);

/// True when a field is a sensible place to show the badge.
pub fn worth_marking((_, _, w, h): Frame) -> bool {
    w >= MIN_WIDTH && h >= MIN_HEIGHT && w * h <= MAX_AREA
}

/// Top-left of the badge for a field: just outside its left edge, level with the bottom line
/// of the field (centred when the field is only one line tall). A field flush against the
/// left edge of the screen has no room outside, so the badge goes above its top-left corner.
pub fn badge_position((x, y, _w, h): Frame) -> (f64, f64) {
    let left = x - SIZE - MARGIN;
    if left < 0.0 {
        return (x.max(0.0), (y - SIZE - MARGIN).max(0.0));
    }
    let top = if h < SIZE * 2.0 {
        y + (h - SIZE) / 2.0
    } else {
        y + h - SIZE
    };
    (left, top)
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

/// Tauri command: the badge was clicked. Same as tapping the writing key.
#[tauri::command]
pub fn badge_click() {
    tracing::info!("compose: badge clicked");
    super::controller::press();
}

fn run(app: tauri::AppHandle) {
    let own_pid = std::process::id() as i32;
    let mut enabled = false;
    let mut enabled_checked: Option<Instant> = None;
    let mut shown_at: Option<(i64, i64)> = None;
    loop {
        std::thread::sleep(POLL);
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
    element.frame().filter(|f| worth_marking(*f))
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
    fn a_tall_field_gets_the_badge_outside_its_left_edge_at_the_bottom() {
        let (x, y) = badge_position((100.0, 200.0, 400.0, 120.0));
        assert_eq!((x, y), (100.0 - 28.0 - 4.0, 200.0 + 120.0 - 28.0));
    }

    #[test]
    fn a_one_line_field_gets_it_centred_beside_its_left_edge() {
        let (x, y) = badge_position((100.0, 200.0, 400.0, 32.0));
        assert_eq!((x, y), (68.0, 202.0));
    }

    #[test]
    fn a_field_at_the_screen_edge_gets_it_above_the_corner() {
        let (x, y) = badge_position((10.0, 200.0, 400.0, 120.0));
        assert_eq!((x, y), (10.0, 200.0 - 28.0 - 4.0));
    }

    #[test]
    fn tiny_and_enormous_fields_are_not_marked() {
        assert!(!worth_marking((0.0, 0.0, 60.0, 24.0)));
        assert!(!worth_marking((0.0, 0.0, 300.0, 10.0)));
        assert!(!worth_marking((0.0, 0.0, 2500.0, 1400.0)));
        assert!(worth_marking((0.0, 0.0, 600.0, 80.0)));
    }
}
