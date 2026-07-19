//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! `screenpipe-screen` + `screenpipe-a11y` capture engine (Gap-2 Bucket 2, slices 2–3b).
//!
//! Implements [`super::CaptureEngine`] using two sibling crates from the
//! `Meridiona/screenpipe-fork` (@ last-MIT `892199f74`):
//!
//! - **`screenpipe-a11y`** — the accessibility-tree walker. Each tick walks the
//!   focused window's AX tree → structured text. This is the **preferred**
//!   signal: for Electron/Chromium apps (VS Code, Slack, Claude) the walker
//!   enables `AXManualAccessibility`/`AXEnhancedUserInterface` on the focused
//!   pid itself, so it gets the full text content — not just whatever pixels
//!   happen to be on screen. Emitted with [`TextSource::Accessibility`].
//! - **`screenpipe-screen`** — the OCR **fallback**. When the focused window
//!   exposes no usable AX tree (some native apps, or an Electron renderer still
//!   building its tree on first focus), we grab the window image and run **Apple
//!   Vision OCR in-process**. Emitted with [`TextSource::Ocr`].
//!
//! Privacy: windows the a11y walker reports as `Skipped` (incognito / private
//! browsing / excluded apps) are dropped entirely — we do **not** fall back to
//! OCR on them, so an incognito window is never captured by either path. No
//! pixels are ever persisted — text only.
//!
//! Running the frame grab + OCR + AX walk in *this* process is what yields the
//! single "Meridian" Screen-Recording / Accessibility TCC entries (vs the
//! external screenpipe daemon's own entries).
//!
//! # Related
//! - [`super`] — the engine-agnostic boundary ([`CaptureEngine`], [`CapturedFrame`]).

use std::time::Duration;

use chrono::Utc;
use screenpipe_a11y::tree::{
    create_tree_walker, TreeSnapshot, TreeWalkResult, TreeWalkerConfig, TreeWalkerPlatform,
};
use screenpipe_screen::capture_screenshot_by_window::{
    capture_all_visible_windows, CapturedWindow, WindowFilters,
};
use tracing::{debug, info, warn};

use super::drm_detector::{self, StreamingGate};
use super::{CaptureEngine, CapturedFrame, FrameTx, TextSource};

/// Seconds between capture passes. Conservative fixed cadence for v1;
/// screenpipe's idle-adaptive frame rate is a later refinement.
const CAPTURE_INTERVAL: Duration = Duration::from_secs(2);

/// Outcome of the per-tick accessibility-tree walk — decides what (if anything)
/// the OCR fallback does this tick.
enum A11yOutcome {
    /// The walk produced usable text → emit this `Accessibility` frame, skip OCR.
    Frame(Box<CapturedFrame>),
    /// The walker deliberately skipped this window (incognito / excluded app) →
    /// capture nothing this tick. **Must not** fall through to OCR (privacy).
    Skip,
    /// No a11y tree available (no focused window, empty warm-up walk, or error)
    /// → fall back to OCR for this tick.
    FallBackToOcr,
}

/// In-process capture backed by the forked `screenpipe-screen` + `screenpipe-a11y`.
#[derive(Default)]
pub struct ScreenpipeEngine {
    /// Whether to pause capture when streaming video is detected
    pub pause_on_streaming_video: bool,
}

impl CaptureEngine for ScreenpipeEngine {
    async fn run(self, tx: FrameTx) -> anyhow::Result<()> {
        let pause_on_streaming = self.pause_on_streaming_video;
        // Register with TCC + drive both prompts ourselves. Neither library
        // prompts on its own: screen-capture enumeration only PREFLIGHTS, and
        // the AX tree reads nothing until this process is a trusted AX client.
        request_screen_capture_access();
        request_accessibility_access();

        // Build the tree walker ONCE: it owns an internal enhanced-mode cache
        // (one AX-enable poke per pid, not per walk) + node caches reused across
        // walks. Rebuilding per tick would defeat that de-thrash logic. The
        // `Box<dyn TreeWalkerPlatform>` is `Send` (supertrait bound), so owning
        // it across the loop's awaits is sound.
        let walker = create_tree_walker(TreeWalkerConfig::default());

        // Sticky per-app streaming state — carries a streaming classification
        // across ticks where the AX tree fails to re-expose the URL/title
        // (see `StreamingGate` docs: Safari's caption overlay is the
        // motivating case). Lives for the whole engine run, single-threaded.
        let mut streaming_gate = StreamingGate::new();

        info!(
            interval_s = CAPTURE_INTERVAL.as_secs(),
            "capture: screenpipe engine started (a11y-tree + OCR fallback)"
        );
        loop {
            if tx.is_closed() {
                info!("capture: consumer gone — stopping engine");
                return Ok(());
            }
            // The AX walk is synchronous (bounded by the walker's walk_timeout)
            // and MUST finish before any await: `&dyn TreeWalkerPlatform` is
            // `Send` but not `Sync`, so the reference cannot cross an await
            // point. Returning an owned outcome keeps the borrow local to here.
            // Pooled: the AX calls autorelease bridged values into the current
            // thread's pool, and tokio worker threads never drain one — see
            // `with_autorelease_pool`.
            let outcome = with_autorelease_pool(|| try_walk_a11y(walker.as_ref()));
            // Pre-capture gate: macOS blanks/flickers DRM video whenever ANY
            // ScreenCaptureKit session is active on the display — regardless
            // of which window it targets — so the OCR fallback's SCK call
            // must never fire while streaming content is on screen, even if
            // it's for a completely unrelated app. Checked with process
            // existence + AppleScript only (never SCK) — see
            // `drm_detector::any_streaming_content_visible`.
            // `any_streaming_content_visible` shells out to `pgrep`/`osascript`
            // (up to 9 process spawns) — run it on the blocking pool rather
            // than stalling this tokio worker thread every tick.
            let skip_ocr_for_streaming = pause_on_streaming
                && tokio::task::spawn_blocking(drm_detector::any_streaming_content_visible)
                    .await
                    .unwrap_or(false);
            if let Err(e) = dispatch(
                &tx,
                outcome,
                pause_on_streaming,
                &mut streaming_gate,
                skip_ocr_for_streaming,
            )
            .await
            {
                warn!(error = %e, "capture: tick failed (will retry)");
            }
            tokio::time::sleep(CAPTURE_INTERVAL).await;
        }
    }
}

/// Walk the focused window's accessibility tree and classify the result into an
/// [`A11yOutcome`]. Synchronous on purpose (see the call site): holds only a
/// `&dyn TreeWalkerPlatform`, which must not outlive the borrow across an await.
fn try_walk_a11y(walker: &dyn TreeWalkerPlatform) -> A11yOutcome {
    match walker.walk_focused_window() {
        Ok(TreeWalkResult::Found(snap)) => {
            if snap.text_content.trim().is_empty() {
                // Chromium builds its AX tree asynchronously after the enable
                // poke, so the first walk on a freshly-focused Electron window
                // can come back empty — OCR covers that tick, a11y the next.
                debug!(app = %snap.app_name, "capture: a11y tree empty (warm-up) — OCR fallback");
                return A11yOutcome::FallBackToOcr;
            }
            let app_lower = snap.app_name.to_lowercase();
            // Never record Meridian capturing its own dashboard windows.
            if is_self_app(&app_lower) {
                debug!(app = %snap.app_name, "capture: skipping self (Meridian window)");
                return A11yOutcome::Skip;
            }
            // Canvas-rendered apps (Figma, design tools): content is GPU pixels,
            // not in the AX tree. The a11y snapshot gives only toolbar/menu text;
            // OCR is the only path to actual canvas content.
            if is_canvas_app(&app_lower) {
                debug!(
                    app = %snap.app_name,
                    "capture: canvas app — a11y can't reach GPU content, OCR fallback"
                );
                return A11yOutcome::FallBackToOcr;
            }
            // Browsers return non-empty a11y text even when the page hasn't been
            // exposed: the tab strip (AXTab/AXButton nodes) alone gives ~695 chars
            // of tab titles. Gate on is_browser so we don't apply the density
            // heuristic to non-browser apps (sparse dialogs, preferences panes, etc.)
            // where thin a11y text is expected and OCR adds no value.
            if is_browser(&app_lower) && a11y_content_is_thin(&snap) {
                debug!(
                    app = %snap.app_name,
                    chars = snap.text_content.len(),
                    "capture: a11y tree is browser-chrome-only — OCR fallback for page content"
                );
                return A11yOutcome::FallBackToOcr;
            }
            debug!(
                app = %snap.app_name,
                chars = snap.text_content.len(),
                "capture: a11y tree walked"
            );
            A11yOutcome::Frame(Box::new(CapturedFrame {
                timestamp: Utc::now(),
                app_name: Some(snap.app_name),
                window_name: Some(snap.window_name),
                browser_url: snap.browser_url,
                text: snap.text_content,
                text_source: TextSource::Accessibility,
            }))
        }
        // Incognito / excluded app / user-ignored: the walker's privacy decision.
        // Honour it — do NOT OCR these windows.
        Ok(TreeWalkResult::Skipped(reason)) => {
            debug!(%reason, "capture: a11y walk skipped (privacy) — capturing nothing");
            A11yOutcome::Skip
        }
        // No focused window or no text extracted — try OCR.
        Ok(TreeWalkResult::NotFound) => A11yOutcome::FallBackToOcr,
        Err(e) => {
            debug!(error = %e, "capture: a11y walk errored — OCR fallback");
            A11yOutcome::FallBackToOcr
        }
    }
}

/// Returns `true` when `app_lower` is Meridian's own tray process.
///
/// The AX tree reports `SELF_BINARY_NAME` ("meridian-tray") in dev / inside
/// the bundle before `setProcessName:` fires, and `SELF_PRODUCT_NAME_LOWER`
/// ("meridian") once it has. Both constants are declared in `crate::lib` next
/// to the `set_process_display_name` call so a rename there is visible here.
fn is_self_app(app_lower: &str) -> bool {
    app_lower == crate::SELF_PRODUCT_NAME_LOWER || app_lower == crate::SELF_BINARY_NAME
}

/// Returns `true` when `app_lower` is a known browser.
fn is_browser(app_lower: &str) -> bool {
    [
        "chrome", "safari", "firefox", "arc", "edge", "brave", "opera", "vivaldi",
    ]
    .iter()
    .any(|b| app_lower.contains(b))
}

/// Returns `true` when `app_lower` is a canvas-rendering design tool whose
/// content is drawn to a GPU surface and is therefore absent from the AX tree.
fn is_canvas_app(app_lower: &str) -> bool {
    [
        "figma",
        "canva",
        "miro",
        "sketch",
        "origami",
        "principle",
        "framer",
        "penpot",
    ]
    .iter()
    .any(|a| app_lower.contains(a))
}

/// Returns `true` when the a11y snapshot contains mostly browser-chrome roles
/// (tab strip, toolbar, buttons) rather than real page content. When this is
/// the case, the tray falls back to OCR so we capture what's actually on screen.
///
/// Ported from `screenpipe-capture::paired_capture::a11y_content_is_thin` —
/// the same heuristic used by the screenpipe CLI's paired-capture path.
///
/// Thresholds:
/// - fewer than 100 total chars → always thin (no meaningful content)
/// - content-role chars < 30 % of total → thin (mostly chrome)
fn a11y_content_is_thin(snap: &TreeSnapshot) -> bool {
    // AX roles that represent browser UI chrome rather than document content.
    const CHROME_ROLES: &[&str] = &[
        "AXButton",
        "AXMenuItem",
        "AXMenuBar",
        "AXMenu",
        "AXToolbar",
        "AXTabGroup",
        "AXTab",
        "AXPopUpButton",
        "AXCheckBox",
        "AXRadioButton",
        "AXDisclosureTriangle",
        "AXSlider",
        "AXIncrementor",
        "AXComboBox",
        "AXScrollBar",
    ];

    // If the walker returned no node metadata (max-node budget exhausted and
    // all nodes were elided), we have no role data to reason about — treat as
    // non-thin so the text_content we do have is used rather than discarded.
    if snap.nodes.is_empty() {
        return false;
    }

    let mut content_chars: usize = 0;
    let mut total_chars: usize = 0;

    for node in &snap.nodes {
        let len = node.text.len();
        if len == 0 {
            continue;
        }
        total_chars += len;
        if !CHROME_ROLES.iter().any(|r| node.role == *r) {
            content_chars += len;
        }
    }

    if total_chars < 100 {
        return true;
    }

    let ratio = content_chars as f64 / total_chars as f64;
    ratio < 0.3
}

/// Route one tick's [`A11yOutcome`]: send the a11y frame, drop a privacy-skip,
/// or run the OCR fallback. Best-effort — a failed OCR pass is logged and
/// retried, never fatal (capture must not crash the tray).
///
/// `skip_ocr_for_streaming`: when true, the OCR fallback is skipped entirely
/// for this tick — never invoking ScreenCaptureKit — because streaming
/// content is on screen somewhere and any SCK call would trigger macOS's
/// DRM blank/flicker regardless of which window OCR was targeting.
async fn dispatch(
    tx: &FrameTx,
    outcome: A11yOutcome,
    pause_on_streaming: bool,
    streaming_gate: &mut StreamingGate,
    skip_ocr_for_streaming: bool,
) -> anyhow::Result<()> {
    match outcome {
        A11yOutcome::Frame(frame) => {
            send_frame(tx, *frame, pause_on_streaming, streaming_gate).await;
            Ok(())
        }
        A11yOutcome::Skip => Ok(()),
        A11yOutcome::FallBackToOcr => {
            if skip_ocr_for_streaming {
                debug!(
                    "capture: skipping OCR fallback this tick — streaming content on screen (avoiding DRM blank/flicker)"
                );
                return Ok(());
            }
            capture_once_ocr(tx, pause_on_streaming, streaming_gate).await
        }
    }
}

/// OCR fallback: one capture + Apple-Vision-OCR pass over the **focused
/// window(s)** of the primary monitor. Per-window (not monitor-level) so each
/// frame carries the app/window/url the classifier keys on — matching
/// meridian's per-app ETL model.
async fn capture_once_ocr(
    tx: &FrameTx,
    pause_on_streaming: bool,
    streaming_gate: &mut StreamingGate,
) -> anyhow::Result<()> {
    let monitors = screenpipe_screen::monitor::list_monitors().await;
    let Some(monitor) = monitors.into_iter().next() else {
        anyhow::bail!("no monitors enumerated (Screen Recording not granted yet?)");
    };

    // No app/title/url filters; focused window(s) only (`capture_unfocused = false`)
    // — we capture what the user is actively on, not every background window.
    let filters = WindowFilters::new(&[], &[], &[]);
    // DIAGNOSTIC: brackets the SCK screenshot enumeration so a crash INSIDE it
    // (no "enumerated" line follows) is distinguishable from a window being
    // dropped (a focused app missing from `apps`) or a blank image (see per-window
    // dims below). Remove once the OCR/SCK root cause is fixed.
    tracing::info!("capture.ocr.diag: enumerating windows via SCK screenshot path");
    let windows = capture_all_visible_windows(&monitor, &filters, false)
        .await
        .map_err(|e| anyhow::anyhow!("capture_all_visible_windows: {e}"))?;
    tracing::info!(
        window_count = windows.len(),
        apps = ?windows.iter().map(|w| w.app_name.as_str()).collect::<Vec<_>>(),
        "capture.ocr.diag: windows enumerated"
    );

    let now = Utc::now();
    for win in windows {
        if is_self_app(&win.app_name.to_lowercase()) {
            continue;
        }
        // DIAGNOSTIC: image dims localize a blank/zero-size capture (dead SCStream)
        // and mark the last point reached before a crash inside `perform_ocr`.
        tracing::info!(
            app = %win.app_name,
            img_w = win.image.width(),
            img_h = win.image.height(),
            "capture.ocr.diag: about to OCR window"
        );
        let Some(text) = perform_ocr(&win).await else {
            tracing::info!(app = %win.app_name, "capture.ocr.diag: perform_ocr → None (blank/illegible)");
            continue;
        };
        if text.trim().is_empty() {
            continue; // nothing legible in this window — skip it
        }
        debug!(app = %win.app_name, chars = text.len(), "capture: window OCR'd");
        let frame = CapturedFrame {
            timestamp: now,
            app_name: Some(win.app_name),
            window_name: Some(win.window_name),
            browser_url: win.browser_url,
            text,
            text_source: TextSource::Ocr,
        };
        if !send_frame(tx, frame, pause_on_streaming, streaming_gate).await {
            break; // consumer backpressure / gone — end this tick
        }
    }
    Ok(())
}

/// Non-blocking send: under backpressure drop the frame rather than stall the
/// capture loop (frames are sampled, not transactional). Returns `false` when
/// the frame was dropped (channel full or consumer gone).
///
/// When `pause_on_streaming_video` is enabled, skips frames from known streaming
/// services (Netflix, Disney+, etc.) to avoid capturing black/protected screens
/// or their caption/subtitle text. If the AX-derived `browser_url` is missing,
/// falls back to a direct AppleScript query for scriptable browsers (Safari,
/// Arc) — Safari stops re-exposing `AXDocument` for the rest of the session
/// once playback focus moves into the video subtree, so this is the only
/// reliable signal after the first few seconds of a stream. Also uses
/// `streaming_gate` to stay sticky for browsers/cases the AppleScript fallback
/// doesn't cover (see [`drm_detector::StreamingGate`]).
async fn send_frame(
    tx: &FrameTx,
    frame: CapturedFrame,
    pause_on_streaming_video: bool,
    streaming_gate: &mut StreamingGate,
) -> bool {
    if pause_on_streaming_video {
        let app_name = frame.app_name.as_deref().unwrap_or("").to_string();
        let resolved_url = match frame.browser_url.clone() {
            Some(u) => Some(u),
            // `resolve_url_via_applescript` shells out to `osascript` — a
            // blocking process spawn that can take real wall-clock time. Run
            // it on the blocking pool rather than stalling this tokio worker
            // thread (and every other task scheduled on it) for the duration.
            None => tokio::task::spawn_blocking(move || {
                drm_detector::resolve_url_via_applescript(&app_name)
            })
            .await
            .unwrap_or(None),
        };

        if streaming_gate.should_skip(frame.app_name.as_deref(), resolved_url.as_deref()) {
            debug!(
                app = ?frame.app_name,
                url = ?resolved_url,
                "capture: streaming video detected — skipping frame"
            );
            return true; // Treat as successful send (frame intentionally dropped)
        }
    }

    match tx.try_send(frame) {
        Ok(()) => true,
        Err(e) => {
            debug!(error = %e, "capture: frame dropped (consumer backpressure / gone)");
            false
        }
    }
}

/// OCR one window image, returning its text.
///
/// The fork exposes a different function per platform rather than one
/// dispatching entry point, and they do not share a signature: the Apple one
/// is synchronous and returns a bare tuple, the Windows one is `async` and
/// returns a `Result`. So this is the one place in the engine that has to know
/// which OS it is on — everything else
/// (`capture_all_visible_windows`, `list_monitors`, `create_tree_walker`)
/// already dispatches internally inside the fork.
///
/// `None` means "nothing legible", which the caller skips.
#[cfg(target_os = "macos")]
async fn perform_ocr(win: &CapturedWindow) -> Option<String> {
    // Pooled: Apple Vision autoreleases its request/observation objects and
    // intermediate CGImages into the current thread's pool on every call. This
    // is the hottest allocation site in the engine (a full retina window bitmap
    // per tick for canvas apps like Figma, which always take the OCR path), so
    // without a per-call drain the process grows without bound — see
    // `with_autorelease_pool`.
    let (text, _json, _confidence) =
        with_autorelease_pool(|| screenpipe_screen::perform_ocr_apple(&win.image, &[]));
    (!text.trim().is_empty()).then_some(text)
}

/// Windows counterpart of [`perform_ocr`] — Windows.Media.Ocr via the fork.
///
/// No autorelease pool: that is an Objective-C runtime concept with no
/// counterpart here. Unlike the Apple call this one is fallible, and a failure
/// is logged and treated as "nothing legible" rather than aborting the tick —
/// OCR is already the fallback path, so losing one window's text costs a
/// sample, not a session.
#[cfg(target_os = "windows")]
async fn perform_ocr(win: &CapturedWindow) -> Option<String> {
    match screenpipe_screen::perform_ocr_windows(&win.image, &[]).await {
        Ok((text, _json, _confidence)) => (!text.trim().is_empty()).then_some(text),
        Err(e) => {
            debug!(error = %e, "capture: windows OCR failed for this window");
            None
        }
    }
}

/// Run `f` inside a fresh Objective-C autorelease pool, draining it on return.
///
/// The capture loop runs on tokio worker threads. Unlike the main thread —
/// whose run loop drains an autorelease pool every cycle — a tokio worker
/// lives for the whole process and never drains one, so every object the
/// Apple frameworks autorelease during a tick (Vision text observations and
/// their backing CGImages, AX attribute values, bridged NSStrings) stays
/// referenced by the thread's pool forever. At one tick every 2 s that is
/// unbounded multi-GB growth over a workday — worst for canvas apps (Figma),
/// which take the image-grab + Vision-OCR path on every tick.
///
/// `objc_autoreleasePoolPush`/`Pop` is the stable libobjc pair that an
/// `@autoreleasepool` block compiles down to.
///
/// `f` must be synchronous — never hold a pool across an `.await` (the task
/// can resume on a different worker thread, unbalancing push/pop). Taking a
/// non-async closure enforces that at the call site.
#[cfg(target_os = "macos")]
fn with_autorelease_pool<R>(f: impl FnOnce() -> R) -> R {
    #[link(name = "objc")]
    extern "C" {
        fn objc_autoreleasePoolPush() -> *mut std::ffi::c_void;
        fn objc_autoreleasePoolPop(pool: *mut std::ffi::c_void);
    }
    // Drop guard so the pool is popped even if `f` panics (unwinding past a
    // pushed pool would otherwise leak it and everything in it).
    struct PoolGuard(*mut std::ffi::c_void);
    impl Drop for PoolGuard {
        fn drop(&mut self) {
            // Safety: pops the pool pushed below, on the same thread, exactly once.
            unsafe { objc_autoreleasePoolPop(self.0) }
        }
    }
    // Safety: push/pop are balanced by the guard; no other pool operations
    // interleave from this scope.
    let _pool = PoolGuard(unsafe { objc_autoreleasePoolPush() });
    f()
}

/// Register for Screen Recording, but **only prompt when it isn't already
/// granted**. Preflights with `CGPreflightScreenCaptureAccess` (a pure status
/// read) first and only calls the prompting `CGRequestScreenCaptureAccess` when
/// needed, so a granted permission never re-triggers the system prompt on launch
/// (Apple's preflight-then-request pattern). Returns `true` if already/now granted.
#[cfg(target_os = "macos")]
fn request_screen_capture_access() -> bool {
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGPreflightScreenCaptureAccess() -> bool;
        fn CGRequestScreenCaptureAccess() -> bool;
    }
    // Safety: pure CoreGraphics TCC calls — no args, no UB.
    if unsafe { CGPreflightScreenCaptureAccess() } {
        debug!("capture: screen-recording already granted — not prompting");
        return true;
    }
    let granted = unsafe { CGRequestScreenCaptureAccess() };
    if granted {
        debug!("capture: screen-recording granted after prompt");
    } else {
        warn!("capture: screen-recording not granted yet — frames stay empty until the user grants it");
    }
    granted
}

/// Accessibility (AX) trust for THIS process — the AX analogue of
/// [`request_screen_capture_access`]. Checks `is_process_trusted_with_prompt(false)`
/// (no prompt) first and only shows the system Accessibility prompt
/// (`…with_prompt(true)`) when not already trusted, so a granted permission never
/// re-prompts on launch. Not fatal when denied: OCR still works, only a11y-tree
/// text is unavailable.
#[cfg(target_os = "macos")]
fn request_accessibility_access() -> bool {
    if cidre::ax::is_process_trusted_with_prompt(false) {
        debug!("capture: accessibility (AX) already trusted — not prompting");
        return true;
    }
    let trusted = cidre::ax::is_process_trusted_with_prompt(true);
    if trusted {
        debug!("capture: accessibility (AX) trust granted after prompt");
    } else {
        warn!("capture: accessibility not granted yet — a11y-tree text unavailable until granted (OCR still works)");
    }
    trusted
}

/// Windows counterpart of [`request_screen_capture_access`] — nothing to do.
///
/// Windows has no TCC: Windows Graphics Capture needs no persistent
/// per-app grant, so there is no permission to preflight and no prompt to
/// raise. Returning `true` unconditionally is the accurate answer, not a
/// placeholder — the fork's own `screenpipe_a11y::platform::windows`
/// `check_permissions()` hardcodes the same, with the note that "Windows
/// doesn't require explicit permissions for hooks".
#[cfg(target_os = "windows")]
fn request_screen_capture_access() -> bool {
    debug!("capture: screen capture needs no explicit grant on Windows");
    true
}

/// Windows counterpart of [`request_accessibility_access`] — nothing to do.
///
/// Same reasoning as [`request_screen_capture_access`]: UI Automation has no
/// AX-trust equivalent, so there is no prompt and no state to query.
#[cfg(target_os = "windows")]
fn request_accessibility_access() -> bool {
    debug!("capture: UI Automation needs no explicit grant on Windows");
    true
}
