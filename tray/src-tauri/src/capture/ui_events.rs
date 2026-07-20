//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! In-process input recorder (Gap-2 Bucket 2, slice 3c).
//!
//! Runs `screenpipe-a11y`'s [`UiRecorder`] in-process and persists the input
//! events meridian's ETL reads — clicks/keys/text (for the "last interaction"
//! timestamp that refines session ends), app switches, clipboard, and window
//! focus. Together with the [`super::screenpipe`] frame engine this is the
//! second half of replacing screenpipe's `ui_events` feed with an in-process
//! one, so slice 4b can repoint the daemon at `meridian.db` and drop screenpipe.
//!
//! **Tree walking is OFF here.** The recorder's config *can* walk the AX tree,
//! but [`super::screenpipe::ScreenpipeEngine`] already owns that — leaving it on
//! would double-walk every focused window. We use the recorder for input events
//! only.
//!
//! **Privacy.** Only `clipboard` events carry text (`text_content`); click/key/
//! text/app_switch/window_focus persist their type + timestamp + app only. The
//! upstream recorder already skips password fields, secure input, and applies
//! PII removal (config defaults left on).
//!
//! **Threading.** `UiRecorder` spawns its own `CGEventTap` + clipboard threads
//! and hands back a crossbeam `Receiver`; we drain it on a dedicated OS thread
//! (blocking `recv_timeout`) and forward mapped rows over a tokio channel to the
//! async DB consumer in `lib.rs`.
//!
//! # Related
//! - [`meridian_core::insert_capture_ui_event`] — the writer this feeds.
//! - migration `047_capture_ui_events.sql` — the table shape.

use std::time::Duration;

use meridian_core::CaptureUiEventInsert;
use screenpipe_a11y::{EventData, UiCaptureConfig, UiEvent, UiRecorder};
use tracing::{info, warn};

/// Channel the recorder thread pushes mapped events onto (drained by the async
/// consumer that writes them to `capture_ui_events`).
pub type UiEventTx = tokio::sync::mpsc::Sender<CaptureUiEventInsert>;

/// Map a recorder [`UiEvent`] to a persistable row, or `None` for event types
/// the daemon never reads (mouse move / scroll). Only `clipboard` keeps text —
/// every other type carries just its timestamp + app (see the module privacy
/// note). `event_type` strings are the literals the daemon's `WHERE event_type
/// IN (...)` filters match — kept as an explicit match, not serde derivation.
pub(crate) fn map_event(ev: UiEvent) -> Option<CaptureUiEventInsert> {
    let (event_type, text_content, app_name) = match ev.data {
        EventData::Click { .. } => ("click", None, ev.app_name),
        EventData::Key { .. } => ("key", None, ev.app_name),
        // Drop the typed text — the daemon uses only the timestamp of `text`.
        EventData::Text { .. } => ("text", None, ev.app_name),
        // app_switch's app is the *activated* app, carried in the variant.
        EventData::AppSwitch { name, .. } => ("app_switch", None, Some(name)),
        EventData::WindowFocus { app, .. } => ("window_focus", None, Some(app)),
        EventData::Clipboard { content, .. } => ("clipboard", content, ev.app_name),
        EventData::Move { .. } | EventData::Scroll { .. } => return None,
    };
    Some(CaptureUiEventInsert {
        timestamp: ev.timestamp,
        event_type: event_type.to_string(),
        app_name,
        text_content,
    })
}

/// Input-events-only recorder config: the documented defaults (clicks/text/
/// app_switch/clipboard on; keystrokes/scroll/mouse-move off; password/PII/
/// secure-input skipped) minus tree walking, which the frame engine owns.
fn recorder_config() -> UiCaptureConfig {
    let mut c = UiCaptureConfig::new();
    c.capture_tree = false;
    c.enable_tree_walker = false;
    c.capture_context = false; // element context isn't persisted
    c
}

/// Whether this process already holds macOS Input Monitoring — a pure status read
/// (`IOHIDCheckAccess`, no prompt) used to skip re-requesting an existing grant.
#[cfg(target_os = "macos")]
fn input_monitoring_granted() -> bool {
    #[link(name = "IOKit", kind = "framework")]
    extern "C" {
        fn IOHIDCheckAccess(request_type: u32) -> u32;
    }
    // kIOHIDRequestTypeListenEvent = 1 (Input Monitoring); kIOHIDAccessTypeGranted = 0.
    const LISTEN_EVENT: u32 = 1;
    const GRANTED: u32 = 0;
    // Safety: IOHIDCheckAccess is a pure status read — no prompt, no side effects.
    unsafe { IOHIDCheckAccess(LISTEN_EVENT) == GRANTED }
}

/// Windows counterpart of [`input_monitoring_granted`] — always `true`.
///
/// Same reasoning as the capture engine's `request_screen_capture_access`:
/// Windows has no TCC, so there is no Input Monitoring grant to hold and no
/// reduced mode to fall back to. The low-level input hooks the recorder installs
/// need no per-app permission — the fork's own
/// `screenpipe_a11y::platform::windows` `check_permissions()` hardcodes the same,
/// noting that "Windows doesn't require explicit permissions for hooks". `true`
/// is therefore the accurate answer, not a placeholder.
#[cfg(target_os = "windows")]
fn input_monitoring_granted() -> bool {
    true
}

/// Run the input recorder until `tx` closes. **Blocking** — call on a dedicated
/// OS thread. **Never prompts for Input Monitoring:** the setup wizard dropped it
/// as a permission because the signals the daemon actually consumes
/// (`clipboard` and `app_switch`, via `get_signals`) come from the
/// Accessibility-only workspace observer and clipboard poller — see
/// `screenpipe-a11y` `macos.rs`. When Input Monitoring isn't already granted the
/// recorder runs in **reduced mode** (clipboard and app/window events, no
/// click/key/text CGEventTap) rather than surfacing a cold, wizard-less TCC
/// dialog; the click/key stream feeds only the minor Option-C `ended_at`
/// refinement. `start()` degrades to a no-op recorder (emits nothing) if even
/// reduced mode can't attach, rather than failing.
pub(crate) fn run_ui_event_recorder(tx: UiEventTx) {
    let recorder = UiRecorder::new(recorder_config());
    // Do NOT request Input Monitoring here — prompting would raise a cold TCC
    // dialog the wizard no longer explains. Just log the current status for
    // observability; `recorder.start()` selects full vs reduced mode itself.
    if input_monitoring_granted() {
        info!("capture: input monitoring granted — full ui-event capture (incl. click/key/text)");
    } else {
        info!(
            "capture: input monitoring not granted — reduced mode \
             (clipboard + app/window events; no click/key/text tap)"
        );
    }
    let handle = match recorder.start() {
        Ok(h) => h,
        Err(e) => {
            warn!(error = %e, "capture: ui-event recorder failed to start");
            return;
        }
    };
    info!("capture: ui-event recorder started");
    loop {
        if tx.is_closed() {
            info!("capture: ui consumer gone — stopping recorder");
            handle.stop();
            return;
        }
        // Block up to 500ms for the next event, then re-check tx liveness.
        if let Some(ev) = handle.recv_timeout(Duration::from_millis(500)) {
            if let Some(row) = map_event(ev) {
                // blocking_send is correct here: this is a plain OS thread, not
                // a tokio task. Errors only when the consumer is gone — stop.
                if tx.blocking_send(row).is_err() {
                    handle.stop();
                    return;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};

    fn ev(data: EventData, app: Option<&str>) -> UiEvent {
        UiEvent {
            id: None,
            timestamp: Utc.timestamp_opt(1_700_000_000, 0).unwrap(),
            relative_ms: 0,
            data,
            app_name: app.map(Into::into),
            window_title: None,
            browser_url: None,
            element: None,
            frame_id: None,
        }
    }

    #[test]
    fn click_maps_to_click_with_app_no_text() {
        let row = map_event(ev(
            EventData::Click {
                x: 1,
                y: 2,
                button: 0,
                click_count: 1,
                modifiers: 0,
            },
            Some("Code"),
        ))
        .unwrap();
        assert_eq!(row.event_type, "click");
        assert_eq!(row.app_name.as_deref(), Some("Code"));
        assert_eq!(row.text_content, None);
    }

    #[test]
    fn text_drops_typed_content() {
        let row = map_event(ev(
            EventData::Text {
                content: "secret typing".into(),
                char_count: Some(13),
            },
            Some("Code"),
        ))
        .unwrap();
        assert_eq!(row.event_type, "text");
        assert_eq!(row.text_content, None, "typed text must not be persisted");
    }

    #[test]
    fn app_switch_uses_activated_app_name() {
        // ev.app_name deliberately differs from the activated app to prove we
        // take the variant's `name`, not the event's ambient app.
        let row = map_event(ev(
            EventData::AppSwitch {
                name: "Slack".into(),
                pid: 42,
            },
            Some("Code"),
        ))
        .unwrap();
        assert_eq!(row.event_type, "app_switch");
        assert_eq!(row.app_name.as_deref(), Some("Slack"));
    }

    #[test]
    fn clipboard_keeps_content() {
        let row = map_event(ev(
            EventData::Clipboard {
                operation: 'c',
                content: Some("copied".into()),
            },
            Some("Code"),
        ))
        .unwrap();
        assert_eq!(row.event_type, "clipboard");
        assert_eq!(row.text_content.as_deref(), Some("copied"));
    }

    #[test]
    fn move_and_scroll_are_dropped() {
        assert!(map_event(ev(EventData::Move { x: 1, y: 2 }, Some("Code"))).is_none());
        assert!(map_event(ev(
            EventData::Scroll {
                x: 1,
                y: 2,
                delta_x: 0,
                delta_y: 1
            },
            Some("Code")
        ))
        .is_none());
    }
}
