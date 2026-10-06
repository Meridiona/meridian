//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! The macOS event tap that notices the trigger key.
//!
//! # Listen-only, on purpose
//! The tap is `ListenOnly`: it can observe key and mouse events but can never delay,
//! change or swallow one. A bug here therefore cannot break typing system-wide; at worst a
//! press goes unnoticed. The callback does bookkeeping only (feed the pure detector, send
//! one message) and never touches Accessibility, because it runs in line with every
//! keystroke on the machine.
//!
//! # Permission and recovery
//! A tap needs the Input Monitoring permission. Without it creation fails, so the thread
//! asks once and keeps retrying every few seconds, which makes granting the permission take
//! effect without a restart. The system disables a tap that is slow or interrupted; the
//! thread notices and enables it again.
//!
//! # Who calls this
//! [`super::controller`] spawns it once at startup when compose is enabled.
//!
//! # Related
//! - [`super::trigger`] - the pure tap-versus-everything-else decision.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use core_foundation::runloop::{kCFRunLoopCommonModes, kCFRunLoopDefaultMode, CFRunLoop};
use core_graphics::event::{
    CGEventTap, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement, CGEventType,
    CallbackResult,
};

use super::trigger::{InputEvent, Modifiers, TapDetector, TriggerKey};

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGPreflightListenEventAccess() -> bool;
    fn CGRequestListenEventAccess() -> bool;
}

/// How long to wait before trying to create the tap again.
const RETRY_EVERY: Duration = Duration::from_secs(3);
/// How long each run-loop slice lasts, which bounds how fast stop and re-enable are noticed.
const SLICE: Duration = Duration::from_millis(500);

/// Keeps the tap thread alive; stops it when dropped.
pub struct TapHandle {
    stop: Arc<AtomicBool>,
}

impl Drop for TapHandle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

/// Start the tap thread. `on_tap` is called, on that thread, for every deliberate tap of
/// `key`; it must return immediately.
pub fn spawn(key: TriggerKey, on_tap: impl Fn() + Send + Sync + 'static) -> TapHandle {
    let stop = Arc::new(AtomicBool::new(false));
    let stop_thread = stop.clone();
    let on_tap: Arc<dyn Fn() + Send + Sync> = Arc::new(on_tap);
    let spawned = std::thread::Builder::new()
        .name("compose-event-tap".into())
        .spawn(move || {
            tracing::info!("compose: event-tap thread started");
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                run(key, on_tap, stop_thread)
            }));
            if result.is_err() {
                tracing::error!("compose: event-tap thread panicked and has stopped");
            }
        });
    if let Err(e) = spawned {
        tracing::error!(error = %e, "compose: could not start the event-tap thread");
    }
    TapHandle { stop }
}

/// The event types the tap subscribes to.
///
/// `TapDisabledByTimeout` and `TapDisabledByUserInput` are deliberately NOT here. They are
/// delivered to the callback whether or not they are in the mask, and their raw values
/// (0xFFFFFFFE, 0xFFFFFFFF) overflow the 64-bit mask shift the library builds from this
/// list - a panic in debug builds, which silently killed the tap thread the first time
/// this ran under `tauri dev`.
fn events_of_interest() -> Vec<CGEventType> {
    vec![
        CGEventType::FlagsChanged,
        CGEventType::KeyDown,
        CGEventType::LeftMouseDown,
        CGEventType::RightMouseDown,
        CGEventType::OtherMouseDown,
    ]
}

fn run(key: TriggerKey, on_tap: Arc<dyn Fn() + Send + Sync>, stop: Arc<AtomicBool>) {
    let mut asked = false;
    while !stop.load(Ordering::SeqCst) {
        // SAFETY: plain permission queries.
        if !unsafe { CGPreflightListenEventAccess() } {
            if !asked {
                asked = true;
                tracing::warn!("compose: Input Monitoring is not granted; asking for it");
                unsafe { CGRequestListenEventAccess() };
            }
            std::thread::sleep(RETRY_EVERY);
            continue;
        }
        if !run_tap(key, &on_tap, &stop) {
            std::thread::sleep(RETRY_EVERY);
        }
    }
}

/// Create and service the tap until stopped. Returns false when it could not be created.
fn run_tap(key: TriggerKey, on_tap: &Arc<dyn Fn() + Send + Sync>, stop: &Arc<AtomicBool>) -> bool {
    let started = Instant::now();
    let detector = Mutex::new(TapDetector::new(key));
    let needs_enable = Arc::new(AtomicBool::new(false));
    let flag = needs_enable.clone();
    let callback_tap = on_tap.clone();

    let tap = CGEventTap::new(
        CGEventTapLocation::Session,
        CGEventTapPlacement::HeadInsertEventTap,
        CGEventTapOptions::ListenOnly,
        events_of_interest(),
        move |_proxy, kind, event| {
            let at_ms = started.elapsed().as_millis() as u64;
            let input = match kind {
                CGEventType::FlagsChanged => Some(InputEvent::Modifiers {
                    mods: Modifiers::from_flags(event.get_flags().bits()),
                    at_ms,
                }),
                CGEventType::KeyDown => Some(InputEvent::KeyDown { at_ms }),
                CGEventType::LeftMouseDown
                | CGEventType::RightMouseDown
                | CGEventType::OtherMouseDown => Some(InputEvent::MouseDown { at_ms }),
                CGEventType::TapDisabledByTimeout | CGEventType::TapDisabledByUserInput => {
                    flag.store(true, Ordering::SeqCst);
                    None
                }
                _ => None,
            };
            if let Some(input) = input {
                if let Ok(mut d) = detector.lock() {
                    if d.feed(input).is_some() {
                        callback_tap();
                    }
                }
            }
            CallbackResult::Keep
        },
    );
    let tap = match tap {
        Ok(t) => t,
        Err(()) => {
            tracing::warn!("compose: event tap could not be created (Input Monitoring missing?)");
            return false;
        }
    };
    let Ok(source) = tap.mach_port().create_runloop_source(0) else {
        tracing::error!("compose: could not create the run-loop source for the event tap");
        return false;
    };
    // SAFETY: the run-loop mode constants are immortal statics.
    CFRunLoop::get_current().add_source(&source, unsafe { kCFRunLoopCommonModes });
    tap.enable();
    tracing::info!(key = ?key, "compose: event tap running");

    while !stop.load(Ordering::SeqCst) {
        // SAFETY: the run-loop mode constant is an immortal static.
        CFRunLoop::run_in_mode(unsafe { kCFRunLoopDefaultMode }, SLICE, false);
        if needs_enable.swap(false, Ordering::SeqCst) {
            tracing::warn!("compose: event tap was disabled by the system; enabling it again");
            tap.enable();
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The library builds `1 << event_type` into a 64-bit mask; any larger value panics in
    /// debug builds. This is the regression test for the bug that left the tap dead.
    #[test]
    fn every_subscribed_event_fits_in_the_64_bit_mask() {
        for ty in events_of_interest() {
            assert!((ty as u64) < 64, "{ty:?} would overflow the event mask");
        }
    }

    #[test]
    fn the_tap_disabled_events_are_not_subscribed_because_they_arrive_anyway() {
        let types = events_of_interest();
        assert!(!types.iter().any(|t| matches!(
            t,
            CGEventType::TapDisabledByTimeout | CGEventType::TapDisabledByUserInput
        )));
    }
}
