//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Popover window placement and macOS NSPanel behaviour.
//!
//! Two things live here, both split out of `lib.rs`:
//!
//! - **Placement** - [`tray_anchor_position`] and [`monitor_work_area`] work out where a
//!   tray-anchored window goes. `tray_anchor_position` is deliberately pure and
//!   platform-independent so the macOS-anchors-below / Windows-anchors-above split is
//!   unit-testable without a live window.
//! - **NSPanel behaviour** (macOS only) - making the popover render over another app's
//!   full-screen Space, showing it without stealing focus, promoting it to a panel, and
//!   dismissing it on an outside click.
//!
//! # Who calls this
//! [`crate::run`]'s tray-event handlers position and show the popover and tooltip;
//! [`crate::commands::system`] re-arms the click-outside monitor after reopening an
//! already-open window; [`crate::capture::screenpipe`] relies on the process display
//! name this sets.
//!
//! # Related
//! - [`crate::tray`] - builds the tray menu whose click rect anchors these windows.
//! - [`crate::reopen`] - the other pure, testable decision split out of `lib.rs`.

/// Top-left position for a window anchored to the tray icon's click rect —
/// centred horizontally on the icon, and placed either below it or above it.
///
/// macOS keeps the tray icon in the menu bar at the *top* of the screen, so
/// anchoring below (`icon_pos.y + icon_size.height`) lands the popover/tooltip
/// on-screen. Windows puts the tray icon in the notification area at the
/// *bottom* of the taskbar, so that same "below" math pushes the window off
/// the bottom of the screen — invisible, not just misplaced. `anchor_above`
/// picks which side; callers pass `cfg!(target_os = "windows")`.
///
/// `monitor_bounds` is `(left, top, right, bottom)` of the target monitor's
/// work area (see [`monitor_work_area`]) — clamping against it, not just
/// zero, is what keeps a right-edge tray icon (common on Windows multi-monitor
/// setups) from placing the window past the monitor's right or bottom edge.
///
/// Pure and platform-independent so it's unit-testable without a live window —
/// same rationale as [`crate::reopen::is_onboarded`] / [`crate::reopen::reopen_target`],
/// which moved to their own module in the same split that created this one.
pub(crate) fn tray_anchor_position(
    icon_pos: (i32, i32),
    icon_size: (i32, i32),
    win_size: (i32, i32),
    anchor_above: bool,
    monitor_bounds: (i32, i32, i32, i32),
) -> (i32, i32) {
    let (mon_left, mon_top, mon_right, mon_bottom) = monitor_bounds;
    let x = (icon_pos.0 + icon_size.0 / 2 - win_size.0 / 2)
        .max(mon_left)
        .min(mon_right - win_size.0);
    let y = if anchor_above {
        icon_pos.1 - win_size.1
    } else {
        icon_pos.1 + icon_size.1
    };
    let y = y.max(mon_top).min(mon_bottom - win_size.1);
    (x, y)
}

/// The window's current monitor's work area as `(left, top, right, bottom)` in
/// physical pixels — the work area excludes the taskbar/menu bar, so clamping
/// against it can't still place a window behind either.
///
/// Falls back to an effectively unbounded rect when Tauri can't resolve a
/// monitor (headless CI, a monitor unplugged mid-call) so callers degrade to
/// the old zero-only clamp instead of failing the whole positioning step.
pub(crate) fn monitor_work_area(window: &tauri::WebviewWindow) -> (i32, i32, i32, i32) {
    match window.current_monitor() {
        Ok(Some(monitor)) => {
            let area = monitor.work_area();
            (
                area.position.x,
                area.position.y,
                area.position.x + area.size.width as i32,
                area.position.y + area.size.height as i32,
            )
        }
        _ => (0, 0, i32::MAX, i32::MAX),
    }
}

/// Set the window's macOS `collectionBehavior` and level so it renders over
/// another app's full-screen Space, not just normal Spaces.
///
/// `WebviewWindow::set_visible_on_all_workspaces(true)` (tao) only OR-s in
/// `NSWindowCollectionBehaviorCanJoinAllSpaces`. A window over a full-screen app
/// also needs `NSWindowCollectionBehaviorFullScreenAuxiliary`, which tao never
/// sets — so the popover/tooltip silently fail to appear when a full-screen app
/// owns the active Space. We send `setCollectionBehavior:` directly, OR-ing both
/// flags onto whatever is already there, and raise the window level to
/// `NSPopUpMenuWindowLevel` (101) so it sits above full-screen app content.
/// `NSStatusWindowLevel` (25) is above the menu bar on normal Spaces but can sit
/// *below* a full-screen app's compositor layer — pop-up menu level is the safe
/// choice and is what Spotlight / Alfred / 1Password mini use.
/// Must run on the main thread (the `setup` hook and tray-event handlers do).
#[cfg(target_os = "macos")]
pub(crate) fn make_visible_over_fullscreen(win: &tauri::WebviewWindow) {
    use objc2::{msg_send, runtime::AnyObject};

    // AppKit NSWindowCollectionBehavior bit flags (stable since 10.x).
    const CAN_JOIN_ALL_SPACES: usize = 1 << 0;
    const FULL_SCREEN_AUXILIARY: usize = 1 << 8;
    // NSPopUpMenuWindowLevel (101): above all normal app content and above
    // full-screen app compositor layers. NSStatusWindowLevel (25) is not
    // reliably above full-screen content on macOS 14+.
    const NS_POPUP_MENU_WINDOW_LEVEL: isize = 101;

    let ptr = match win.ns_window() {
        Ok(p) if !p.is_null() => p as *const AnyObject,
        _ => {
            tracing::warn!(label = %win.label(), "make_visible_over_fullscreen: ns_window unavailable");
            return;
        }
    };
    // Safety: `ptr` is a live NSWindow for the lifetime of this call (we hold
    // `win`), and we are on the main thread. `collectionBehavior` /
    // `setCollectionBehavior:` / `setLevel:` are NSUInteger/NSInteger get/sets
    // with no ownership transfer.
    unsafe {
        let ns = &*ptr;
        let current: usize = msg_send![ns, collectionBehavior];
        let next = current | CAN_JOIN_ALL_SPACES | FULL_SCREEN_AUXILIARY;
        let _: () = msg_send![ns, setCollectionBehavior: next];
        let _: () = msg_send![ns, setLevel: NS_POPUP_MENU_WINDOW_LEVEL];
        tracing::info!(
            label = %win.label(),
            behavior_before = current,
            behavior_after = next,
            level = NS_POPUP_MENU_WINDOW_LEVEL,
            "make_visible_over_fullscreen: applied"
        );
    }
}

/// Show a window without stealing focus from the currently active app.
///
/// `WebviewWindow::show()` calls `makeKeyAndOrderFront:` which signals the
/// active app to deactivate — if the active app is in full-screen this can
/// cause macOS to switch away from its Space before our window appears.
/// `orderFrontRegardless` shows the window at its current level without
/// changing the key-window status, so the full-screen app stays active.
/// For the NSPanel popover this is complementary to the non-activating mask
/// (belt-and-suspenders): the mask prevents key-window steal, and this call
/// prevents the ordering operation from triggering a Space switch.
#[cfg(target_os = "macos")]
pub(crate) fn show_no_focus(win: &tauri::WebviewWindow) {
    use objc2::{msg_send, runtime::AnyObject};
    match win.ns_window() {
        Ok(p) if !p.is_null() => unsafe {
            let ns = &*(p as *const AnyObject);
            let _: () = msg_send![ns, orderFrontRegardless];
        },
        _ => {
            let _ = win.show();
        }
    }
}

/// Convert an NSWindow to a non-activating NSPanel for fullscreen Space support.
///
/// A plain NSWindow shown via `makeKeyAndOrderFront:` activates the app, which
/// macOS interprets as "switch to this app's Space" — causing a Space switch
/// away from a fullscreen app. `NSWindowStyleMaskNonactivatingPanel` (bit 7)
/// prevents the panel from ever becoming key or activating the app, so the
/// fullscreen app's Space stays active and the panel appears within it when
/// we call `orderFrontRegardless` (see `show_no_focus`).
///
/// NSPanel is a direct `NSWindow` subclass with identical ivar layout;
/// `object_setClass` between them is safe (same technique as `tauri-nspanel`).
/// The WKWebView IPC bridge is unaffected — it lives inside the view, not the
/// window class.
///
/// `hidesOnDeactivate: NO` keeps the panel visible when the Accessory-policy
/// process briefly backgrounds (otherwise it flickers).
///
/// Since a non-activating panel never becomes key, `Focused(false)` never fires.
/// Click-outside dismiss is handled instead by a global `NSEvent` monitor
/// installed via `install_click_outside_monitor`.
///
/// Must be called in the `setup` hook before the window is ever shown.
#[cfg(target_os = "macos")]
pub(crate) fn init_as_nspanel(win: &tauri::WebviewWindow) {
    use objc2::{class, msg_send, runtime::AnyObject};

    extern "C" {
        fn object_setClass(
            obj: *mut AnyObject,
            cls: *const objc2::runtime::AnyClass,
        ) -> *const objc2::runtime::AnyClass;
    }

    let ptr = match win.ns_window() {
        Ok(p) if !p.is_null() => p as *mut AnyObject,
        _ => {
            tracing::warn!(label = %win.label(), "init_as_nspanel: ns_window unavailable");
            return;
        }
    };
    // Safety: NSPanel is a direct NSWindow subclass with identical ivar layout.
    // object_setClass between them is safe before the window is first shown.
    // We are on the main thread (setup hook). No ownership transfer.
    unsafe {
        object_setClass(ptr, class!(NSPanel));
        let ns = &*ptr;
        // NSWindowStyleMaskNonactivatingPanel = 1 << 7.
        let current_mask: usize = msg_send![ns, styleMask];
        let _: () = msg_send![ns, setStyleMask: current_mask | (1usize << 7)];
        let _: () = msg_send![ns, setHidesOnDeactivate: false];
        tracing::info!(
            label = %win.label(),
            new_mask = current_mask | (1usize << 7),
            "init_as_nspanel: converted to non-activating NSPanel"
        );
    }
}

/// Clip the popover's native content view to a rounded rect matching the
/// card's own CSS `border-radius`, so the window's 4 corners are genuinely
/// transparent instead of relying on WKWebView's private `drawsBackground`
/// hack to be pixel-perfect there.
///
/// The CSS card (`.pop` / `.signin-lock`, `--radius: 18px`) is rounded, but
/// the native window and its WKWebView are a plain rectangle — CSS
/// `overflow: hidden` only clips content *inside* the div, not the
/// webview's own backing layer. `app.js`'s `resizeToContent` already works
/// around the same underlying gap on the bottom edge (fitting window height
/// to the card so no unclipped strip shows below it), but the 4 corners
/// carved out by `border-radius` were never addressed - wry only asks
/// WKWebView to stop painting its default background via a private
/// `drawsBackground` KVC key, which is not always pixel-accurate right at
/// an anti-aliased curve, so those corner triangles can render as a faint
/// white/light patch over the desktop instead of true transparency. Masking
/// `contentView`'s `CALayer` removes them at the compositor level - nothing
/// paints there at all - independent of whatever WKWebView itself does.
///
/// Only the popover ("main") uses this: the tooltip window is deliberately
/// *larger* than its own (differently-radiused) card, with transparent
/// padding on all sides for tail placement (see `tooltip.css`), so clipping
/// its whole window to a rounded rect would cut off that intentional
/// margin.
///
/// Must be called after the window has a native handle (`setup` hook, same
/// site as [`init_as_nspanel`]).
#[cfg(target_os = "macos")]
pub(crate) fn apply_corner_mask(win: &tauri::WebviewWindow, radius: f64) {
    use objc2::{msg_send, runtime::AnyObject};

    let ptr = match win.ns_window() {
        Ok(p) if !p.is_null() => p as *mut AnyObject,
        _ => {
            tracing::warn!(label = %win.label(), "apply_corner_mask: ns_window unavailable");
            return;
        }
    };
    // Safety: `ptr` is a live NSWindow handle for the lifetime of this call
    // (owned by the still-running app); we only send read/write property
    // selectors that exist on every NSWindow/NSView/CALayer. Main thread
    // (setup hook), no ownership transfer.
    unsafe {
        let ns = &*ptr;
        let content_view: *mut AnyObject = msg_send![ns, contentView];
        let Some(cv) = content_view.as_ref() else {
            tracing::warn!(label = %win.label(), "apply_corner_mask: contentView unavailable");
            return;
        };
        let _: () = msg_send![cv, setWantsLayer: true];
        let layer: *mut AnyObject = msg_send![cv, layer];
        let Some(l) = layer.as_ref() else {
            tracing::warn!(label = %win.label(), "apply_corner_mask: layer unavailable");
            return;
        };
        let _: () = msg_send![l, setCornerRadius: radius];
        let _: () = msg_send![l, setMasksToBounds: true];
    }
    tracing::info!(label = %win.label(), radius, "apply_corner_mask: clipped content view to rounded rect");
}

/// Install a global `NSEvent` monitor that hides the popover when the user
/// clicks outside it (in any other app or window).
///
/// `addGlobalMonitorForEventsMatchingMask:handler:` fires for mouse-down events
/// delivered to OTHER processes — it does NOT fire for clicks inside our own
/// windows. This makes it a clean "click outside" detector: any click that
/// doesn't land in the popover will hide it.
///
/// This replaces `Focused(false)` / `windowDidResignKey` which never fires for
/// a non-activating NSPanel (the panel never becomes key in the first place).
///
/// The block is leaked and the monitor runs for the app's lifetime. CPU impact
/// is negligible: the closure body only runs when the popover is visible.
#[cfg(target_os = "macos")]
pub(crate) fn install_click_outside_monitor(win: tauri::WebviewWindow) {
    use block2::RcBlock;
    use objc2::{class, msg_send, runtime::AnyObject};

    // NSLeftMouseDown (1<<1) | NSRightMouseDown (1<<3)
    let mask: u64 = (1u64 << 1) | (1u64 << 3);

    let block = RcBlock::new(move |_event: *mut AnyObject| {
        if win.is_visible().unwrap_or(false) {
            let _ = win.hide();
        }
    });

    // Leak the RcBlock so it lives for the app's lifetime.
    // RcBlock<Dyn>: Deref<Target = Block<Dyn>>; the coercion gives &Block<Dyn>
    // which implements RefEncode (encoding: @? = block pointer).
    let block_ref: &'static block2::Block<dyn Fn(*mut AnyObject)> = Box::leak(Box::new(block));

    unsafe {
        let _monitor: *mut AnyObject = msg_send![
            class!(NSEvent),
            addGlobalMonitorForEventsMatchingMask: mask,
            handler: block_ref,
        ];
        tracing::info!("install_click_outside_monitor: global NSEvent monitor installed");
    }
}

/// Set the macOS process display name used by the dock, Activity Monitor, and
/// the AX tree. Must be called early in the setup hook — before the first
/// window is shown — so that every subsequent AX snapshot already sees the
/// corrected name. Without this, `tauri dev` shows "meridian-tray" (the Cargo
/// binary name); production `.app` bundles already use `productName = "Meridian"`
/// from `tauri.conf.json`, but setting it here makes both modes identical.
#[cfg(target_os = "macos")]
pub(crate) fn set_process_display_name(name: &str) {
    use objc2::{class, msg_send, runtime::AnyObject};
    use std::ffi::CString;

    let Ok(c_name) = CString::new(name) else {
        return;
    };
    // Safety: NSProcessInfo is a process-lifetime singleton; setProcessName:
    // copies the NSString immediately. Called once on the main thread (setup hook).
    unsafe {
        let ns_name: *const AnyObject =
            msg_send![class!(NSString), stringWithUTF8String: c_name.as_ptr()];
        if ns_name.is_null() {
            return;
        }
        let info: *const AnyObject = msg_send![class!(NSProcessInfo), processInfo];
        if info.is_null() {
            return;
        }
        let _: () = msg_send![&*info, setProcessName: ns_name];
    }
    tracing::debug!(name, "set_process_display_name: applied");
}

#[cfg(test)]
mod tray_anchor_position_tests {
    use super::tray_anchor_position;

    const UNBOUNDED: (i32, i32, i32, i32) = (0, 0, i32::MAX, i32::MAX);
    // A 1920x1080 monitor at the origin, work area only (no taskbar cutout —
    // irrelevant to these tests since they probe the left/top/right clamps).
    const SCREEN_1080P: (i32, i32, i32, i32) = (0, 0, 1920, 1080);

    #[test]
    fn below_places_top_edge_at_icon_bottom() {
        // macOS-style: icon near the top of the screen, popover grows downward.
        let (x, y) = tray_anchor_position((1500, 0), (24, 24), (344, 400), false, UNBOUNDED);
        assert_eq!(x, 1500 + 12 - 172); // centred under the icon
        assert_eq!(y, 24); // flush with the icon's bottom edge
    }

    #[test]
    fn above_places_bottom_edge_at_icon_top() {
        // Windows-style: icon near the bottom of the screen, popover grows upward.
        let (x, y) = tray_anchor_position((1500, 1040), (24, 24), (344, 400), true, UNBOUNDED);
        assert_eq!(x, 1500 + 12 - 172); // centred over the icon, same as below
        assert_eq!(y, 1040 - 400); // flush with the icon's top edge
    }

    #[test]
    fn x_never_goes_negative_when_icon_is_near_the_left_edge() {
        let (x, _) = tray_anchor_position((5, 0), (24, 24), (344, 400), false, UNBOUNDED);
        assert_eq!(x, 0);
    }

    #[test]
    fn above_never_goes_negative_when_window_is_taller_than_the_icons_offset() {
        // A pathologically tall window anchored above a near-top icon must clamp
        // to 0 rather than requesting a negative y (off the top of the screen).
        let (_, y) = tray_anchor_position((100, 50), (24, 24), (344, 2000), true, UNBOUNDED);
        assert_eq!(y, 0);
    }

    #[test]
    fn x_clamps_to_the_monitors_right_edge() {
        // A tray icon hard against the right edge of a 1920px-wide monitor —
        // centring the 344px popover under it would push its right edge past
        // 1920, common on Windows multi-monitor taskbars.
        let (x, _) = tray_anchor_position((1900, 1040), (24, 24), (344, 400), true, SCREEN_1080P);
        assert_eq!(x, 1920 - 344); // flush with the monitor's right edge, not overflowing it
    }

    #[test]
    fn y_clamps_to_the_monitors_bottom_edge() {
        // macOS-style anchor-below near the bottom of a short/rotated monitor:
        // the popover must not extend past the work area's bottom edge.
        let (_, y) = tray_anchor_position((100, 700), (24, 24), (344, 400), false, SCREEN_1080P);
        assert_eq!(y, 1080 - 400);
    }
}
