//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! `screenpipe-screen` capture engine (Gap-2 Bucket 2, slice 2).
//!
//! Implements [`super::CaptureEngine`] using the forked `screenpipe-screen`
//! crate (`Meridiona/screenpipe-fork` @ last-MIT `892199f74`): enumerate
//! monitors, grab the primary monitor image via ScreenCaptureKit, run **Apple
//! Vision OCR in-process**, and emit the extracted text as a [`CapturedFrame`].
//! No pixels are persisted — text only.
//!
//! Running the frame grab + OCR in *this* process is what yields the single
//! "Meridian" Screen-Recording TCC entry (vs the external screenpipe daemon's
//! own entry). Slice 2 captures monitor-level OCR text only; app/window/url
//! metadata + a11y-tree text + `ui_events` arrive in slice 3.
//!
//! # Related
//! - [`super`] — the engine-agnostic boundary ([`CaptureEngine`], [`CapturedFrame`]).

use std::time::Duration;

use chrono::Utc;
use screenpipe_screen::capture_screenshot_by_window::{capture_all_visible_windows, WindowFilters};
use tracing::{debug, info, warn};

use super::{CaptureEngine, CapturedFrame, FrameTx, TextSource};

/// Seconds between capture+OCR passes. Conservative fixed cadence for v1;
/// screenpipe's idle-adaptive frame rate is a later refinement.
const CAPTURE_INTERVAL: Duration = Duration::from_secs(2);

/// In-process capture backed by the forked `screenpipe-screen`.
#[derive(Default)]
pub struct ScreenpipeEngine;

impl CaptureEngine for ScreenpipeEngine {
    async fn run(self, tx: FrameTx) -> anyhow::Result<()> {
        // Register with TCC + drive the Screen-Recording prompt ourselves. The
        // capture lib's monitor enumeration only PREFLIGHTS (returns empty on
        // denial) and never prompts — the one non-obvious spike finding.
        request_screen_capture_access();

        info!(
            interval_s = CAPTURE_INTERVAL.as_secs(),
            "capture: screenpipe-screen engine started"
        );
        loop {
            if tx.is_closed() {
                info!("capture: consumer gone — stopping engine");
                return Ok(());
            }
            if let Err(e) = capture_once(&tx).await {
                warn!(error = %e, "capture: tick failed (will retry)");
            }
            tokio::time::sleep(CAPTURE_INTERVAL).await;
        }
    }
}

/// One capture + OCR pass over the **focused window(s)** of the primary monitor.
/// Per-window (not monitor-level) so each frame carries the app/window/url the
/// classifier keys on — matching meridian's per-app ETL model. Best-effort: a
/// failed tick is logged and retried, never fatal (capture must not crash the tray).
async fn capture_once(tx: &FrameTx) -> anyhow::Result<()> {
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
        // Non-blocking send: under backpressure drop the frame and end this tick
        // rather than stall the capture loop (frames are sampled, not transactional).
        if let Err(e) = tx.try_send(frame) {
            debug!(error = %e, "capture: frame dropped (consumer backpressure / gone)");
            break;
        }
    }
    Ok(())
}

/// CoreGraphics: register for + prompt Screen Recording. Idempotent; returns
/// `true` if already/now granted. Without it the capture lib silently sees zero
/// monitors on first run.
fn request_screen_capture_access() -> bool {
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGRequestScreenCaptureAccess() -> bool;
    }
    // Safety: a pure CoreGraphics TCC status/prompt call — no args, no UB.
    let granted = unsafe { CGRequestScreenCaptureAccess() };
    if granted {
        debug!("capture: screen-recording access granted");
    } else {
        warn!("capture: screen-recording not granted yet — frames stay empty until the user grants it");
    }
    granted
}
