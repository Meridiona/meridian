//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! In-process capture boundary (Gap-2 Bucket 2).
//!
//! Defines the engine-agnostic [`CaptureEngine`] trait and the [`CapturedFrame`]
//! shape so the capture **backend** is a swappable internal detail: today the
//! forked `screenpipe-screen` ([`screenpipe`], behind the `capture` feature),
//! a native `scap` engine later — same boundary, no architecture change.
//!
//! **Text-only by design.** Capture OCRs each frame in memory and keeps only the
//! extracted text + window metadata — never pixels or video. This is why no
//! ffmpeg/video-encode dependency is pulled, and it's the "we store only text,
//! never screenshots" privacy property.
//!
//! Items flow over an mpsc channel ([`FrameTx`] of [`CaptureItem`]) so the
//! consumer can be swapped without touching the engine: a logger now (slice 2),
//! a `meridian.db` writer later (slice 4). Mirrors the columns meridian's ETL
//! reads (`src/db/screenpipe.rs`). Two item shapes travel the same channel:
//! [`CapturedFrame`] (the per-tick primary window — defines session
//! boundaries) and [`SecondaryScreenSample`] (OCR text from other connected
//! monitors — context folded onto whichever session is open, never a
//! boundary of its own; multi-screen capture).
//!
//! # Who calls this
//! `lib.rs`'s setup hook spawns the engine (behind the `capture` feature) + a
//! consumer task; the engine sends [`CaptureItem`]s, the consumer drains them.
//!
//! # Related
//! - Obsidian `Decisions/Bucket 2 implementation plan - in-process capture.md` — the slice plan.
//! - [`crate::backend_install`] — the other half of the self-contained `.app`.

// The whole `capture` module is gated behind the `capture` feature at the
// crate root (`lib.rs`), so everything here is already in a capture-enabled
// build — no per-item cfg needed.
pub mod drm_detector;
pub mod screenpipe;
pub mod ui_events;

use chrono::{DateTime, Utc};

/// Provenance of a frame's text — mirrors `frames.text_source` in the schema
/// meridian's ETL reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextSource {
    /// Apple Vision OCR of the screen image.
    Ocr,
    /// Accessibility-tree text (primary signal for Electron/Chromium apps).
    /// Constructed by the a11y-tree walker (slice 3b) — preferred over OCR
    /// whenever the focused window exposes a usable tree.
    Accessibility,
}

impl TextSource {
    /// The string form stored in `text_source` ("ocr" | "accessibility").
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ocr => "ocr",
            Self::Accessibility => "accessibility",
        }
    }
}

/// One captured frame's extracted signal. Mirrors the columns meridian's ETL
/// reads (app/window/url/timestamp + on-screen text); carries **no pixels**.
#[derive(Debug, Clone)]
pub struct CapturedFrame {
    /// When the frame was captured.
    pub timestamp: DateTime<Utc>,
    /// Foreground application name (e.g. "Code", "Safari"), if known.
    pub app_name: Option<String>,
    /// Foreground window title, if known.
    pub window_name: Option<String>,
    /// Active browser URL when the foreground app is a browser, if detected.
    pub browser_url: Option<String>,
    /// Extracted text — a11y-tree text when the focused window exposes one,
    /// else Apple Vision OCR. See [`text_source`](Self::text_source).
    pub text: String,
    /// Provenance of `text`.
    pub text_source: TextSource,
}

/// One OCR sample from a monitor OTHER than the one holding the
/// currently-focused window (multi-screen capture). Context, not activity:
/// it never defines a session boundary on its own — the ETL folds it onto
/// whichever session is open when the block containing `timestamp` closes
/// (see `meridian_core::CaptureSecondaryScreenInsert` / migration 068).
#[derive(Debug, Clone)]
pub struct SecondaryScreenSample {
    /// When the sample was captured.
    pub timestamp: DateTime<Utc>,
    /// Display name (`SafeMonitor::name()`, falling back to `stable_id()` when empty).
    pub monitor_name: String,
    /// Foreground app on that monitor's topmost window, if known.
    pub app_name: Option<String>,
    pub window_name: Option<String>,
    /// OCR text of that window.
    pub text: String,
}

/// One item the engine can push: the primary per-tick frame (defines session
/// boundaries) or a secondary-monitor context sample (never does).
#[derive(Debug, Clone)]
pub enum CaptureItem {
    Frame(CapturedFrame),
    Secondary(SecondaryScreenSample),
}

/// Channel the engine pushes captured items onto. A bounded mpsc so a slow
/// consumer applies backpressure to the capture loop rather than growing
/// unbounded.
pub type FrameTx = tokio::sync::mpsc::Sender<CaptureItem>;

/// An in-process capture backend. [`run`](CaptureEngine::run) drives a capture
/// loop until the process exits, sending each extracted frame on `tx`. Returns
/// `Err` for the supervisor to log + restart — **capture must never crash the
/// tray** (it runs in-process now, without the screenpipe daemon's isolation).
#[allow(async_fn_in_trait)] // used concretely (no dyn), so Send-bound erasure is a non-issue
pub trait CaptureEngine {
    /// Run until shutdown, sending frames on `tx`.
    async fn run(self, tx: FrameTx) -> anyhow::Result<()>;
}
