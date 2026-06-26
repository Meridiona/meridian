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
use screenpipe_screen::capture_screenshot_by_window::{capture_all_visible_windows, WindowFilters};
use tracing::{debug, info, warn};

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
pub struct ScreenpipeEngine;

impl CaptureEngine for ScreenpipeEngine {
    async fn run(self, tx: FrameTx) -> anyhow::Result<()> {
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
            let outcome = try_walk_a11y(walker.as_ref());
            if let Err(e) = dispatch(&tx, outcome).await {
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
async fn dispatch(tx: &FrameTx, outcome: A11yOutcome) -> anyhow::Result<()> {
    match outcome {
        A11yOutcome::Frame(frame) => {
            send_frame(tx, *frame);
            Ok(())
        }
        A11yOutcome::Skip => Ok(()),
        A11yOutcome::FallBackToOcr => capture_once_ocr(tx).await,
    }
}

/// OCR fallback: one capture + Apple-Vision-OCR pass over the **focused
/// window(s)** of the primary monitor. Per-window (not monitor-level) so each
/// frame carries the app/window/url the classifier keys on — matching
/// meridian's per-app ETL model.
async fn capture_once_ocr(tx: &FrameTx) -> anyhow::Result<()> {
    let monitors = screenpipe_screen::monitor::list_monitors().await;
    let Some(monitor) = monitors.into_iter().next() else {
        anyhow::bail!("no monitors enumerated (Screen Recording not granted yet?)");
    };

    // No app/title/url filters; focused window(s) only (`capture_unfocused = false`)
    // — we capture what the user is actively on, not every background window.
    let filters = WindowFilters::new(&[], &[], &[]);
    let windows = capture_all_visible_windows(&monitor, &filters, false)
        .await
        .map_err(|e| anyhow::anyhow!("capture_all_visible_windows: {e}"))?;

    let now = Utc::now();
    for win in windows {
        let (text, _json, _confidence) = screenpipe_screen::perform_ocr_apple(&win.image, &[]);
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
        if !send_frame(tx, frame) {
            break; // consumer backpressure / gone — end this tick
        }
    }
    Ok(())
}

/// Non-blocking send: under backpressure drop the frame rather than stall the
/// capture loop (frames are sampled, not transactional). Returns `false` when
/// the frame was dropped (channel full or consumer gone).
fn send_frame(tx: &FrameTx, frame: CapturedFrame) -> bool {
    match tx.try_send(frame) {
        Ok(()) => true,
        Err(e) => {
            debug!(error = %e, "capture: frame dropped (consumer backpressure / gone)");
            false
        }
    }
}

/// Register for Screen Recording, but **only prompt when it isn't already
/// granted**. Preflights with `CGPreflightScreenCaptureAccess` (a pure status
/// read) first and only calls the prompting `CGRequestScreenCaptureAccess` when
/// needed, so a granted permission never re-triggers the system prompt on launch
/// (Apple's preflight-then-request pattern). Returns `true` if already/now granted.
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
