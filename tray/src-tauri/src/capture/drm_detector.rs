//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Streaming video content detection (Netflix, Disney+, Hulu, Prime Video, etc.)
//!
//! When enabled, pauses screen capture when a known DRM streaming service is
//! focused. This prevents capturing black/protected screens that DRM services
//! blank when ScreenCaptureKit is active.
//!
//! Detection works on two levels:
//! 1. **App name** — native streaming apps (Netflix, Disney+, etc.)
//! 2. **Browser URL** — when streaming sites are open in Chrome/Safari/etc.

/// Streaming services with native apps that show black when captured
const STREAMING_APPS: &[&str] = &[
    "netflix",
    "disney+",
    "hulu",
    "prime video",
    "apple tv",
    "peacock",
    "paramount+",
    "hbo max",
    "max",
    "crunchyroll",
    "dazn",
];

/// Domains hosting DRM-protected streaming content
const STREAMING_DOMAINS: &[&str] = &[
    "netflix.com",
    "disneyplus.com",
    "hulu.com",
    "primevideo.com",
    "tv.apple.com",
    "peacocktv.com",
    "paramountplus.com",
    "play.max.com",
    "crunchyroll.com",
    "dazn.com",
];

/// Special path-based matches (e.g., Amazon's `/gp/video/` prefix)
const STREAMING_URL_PATHS: &[(&str, &str)] = &[("amazon.com", "/gp/video/")];

/// Check if an app name matches a known streaming service
pub fn is_streaming_app(app_name: &str) -> bool {
    let lower = app_name.to_lowercase();
    for &service in STREAMING_APPS {
        if service == "max" {
            // Exact match for "Max" to avoid false positives like "Maximum"
            if lower == "max" {
                return true;
            }
        } else if lower.contains(service) {
            return true;
        }
    }
    false
}

/// Check if a URL points to a streaming service
pub fn is_streaming_url(url: &str) -> bool {
    let lower = url.to_lowercase();
    let host_and_path = lower
        .strip_prefix("https://")
        .or_else(|| lower.strip_prefix("http://"))
        .unwrap_or(&lower);
    let normalized = host_and_path.strip_prefix("www.").unwrap_or(host_and_path);

    // Extract host (before the first `/`)
    let host = normalized.split('/').next().unwrap_or(normalized);

    // Check against known domains (including subdomains)
    for &domain in STREAMING_DOMAINS {
        if host == domain || host.strip_suffix(domain).is_some_and(|s| s.ends_with('.')) {
            return true;
        }
    }

    // Check special path patterns
    for &(domain, path) in STREAMING_URL_PATHS {
        if normalized.starts_with(domain) {
            if let Some(url_path) = normalized.strip_prefix(domain) {
                if url_path.starts_with(path) {
                    return true;
                }
            }
        }
    }

    false
}

/// Combined check: is this content from a streaming service?
pub fn is_streaming_content(app_name: &str, url: Option<&str>) -> bool {
    if is_streaming_app(app_name) {
        return true;
    }
    if let Some(u) = url {
        if is_streaming_url(u) {
            return true;
        }
    }
    false
}

/// Safari uses "current tab" terminology in its AppleScript dictionary.
const SAFARI_STYLE_BROWSERS: &[&str] = &["Safari"];

/// Chromium-based browsers share the same "active tab of front window"
/// AppleScript terminology (same underlying scripting dictionary lineage).
/// Verified live for Google Chrome and Arc; the rest follow the identical,
/// well-documented Chromium convention but weren't installed on this machine
/// to verify directly. Firefox is deliberately absent — it has no reliable
/// AppleScript command for the active tab's URL.
const CHROMIUM_STYLE_BROWSERS: &[&str] = &[
    "Google Chrome",
    "Arc",
    "Brave Browser",
    "Microsoft Edge",
    "Vivaldi",
    "Opera",
    "Chromium",
];

/// Best-effort fallback: query a scriptable browser's current-tab URL
/// directly via AppleScript when the AX tree didn't expose one this tick.
///
/// Detection itself stays fully URL/domain-based ([`is_streaming_url`]) and
/// therefore site-specific, not browser-specific — this function only covers
/// the OS-level mechanism to *retrieve* that URL when accessibility fails to
/// expose it. Once playback focus drifts into a video player's subtree (e.g.
/// Netflix's closed-caption overlay), `AXDocument` stops being re-exposed on
/// the window entirely — not just intermittently, but for the rest of the
/// session — so `StreamingGate`'s sticky same-app carry-over has nothing to
/// latch onto after a fresh process start. Asking the browser directly closes
/// that gap, for any browser whose scripting dictionary supports it (mirrors
/// the same technique upstream screenpipe's DRM detector uses for Safari/Arc,
/// generalized here to the wider Chromium family).
///
/// Returns `None` immediately (no process spawn) for any app that isn't a
/// known scriptable browser (e.g. Firefox, or a non-browser app).
#[cfg(target_os = "macos")]
pub fn resolve_url_via_applescript(app_name: &str) -> Option<String> {
    let script = current_tab_url_script(app_name)?;
    run_osascript(&script)
}

#[cfg(not(target_os = "macos"))]
pub fn resolve_url_via_applescript(_app_name: &str) -> Option<String> {
    None
}

/// Pure helper: build the AppleScript source to query `app_name`'s current-tab
/// URL, or `None` if it isn't a known scriptable browser. Split out from
/// [`resolve_url_via_applescript`] so the browser-family dispatch logic is
/// unit-testable without spawning a real process.
fn current_tab_url_script(app_name: &str) -> Option<String> {
    if SAFARI_STYLE_BROWSERS
        .iter()
        .any(|&b| app_name.eq_ignore_ascii_case(b))
    {
        return Some(format!(
            r#"tell application "{app_name}" to return URL of current tab of front window"#
        ));
    }
    if CHROMIUM_STYLE_BROWSERS
        .iter()
        .any(|&b| app_name.eq_ignore_ascii_case(b))
    {
        return Some(format!(
            r#"tell application "{app_name}" to return URL of active tab of front window"#
        ));
    }
    None
}

#[cfg(target_os = "macos")]
fn run_osascript(script: &str) -> Option<String> {
    let output = std::process::Command::new("osascript")
        .arg("-e")
        .arg(script)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let url = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if url.starts_with("http://") || url.starts_with("https://") {
        Some(url)
    } else {
        None
    }
}

/// All scriptable browsers we can ask directly for their current-tab URL via
/// AppleScript (see [`resolve_url_via_applescript`]) — the union of both
/// dictionary styles.
fn scriptable_browsers() -> impl Iterator<Item = &'static str> {
    SAFARI_STYLE_BROWSERS
        .iter()
        .chain(CHROMIUM_STYLE_BROWSERS.iter())
        .copied()
}

/// Best-effort process-name hints for native streaming apps, used only to
/// decide whether it's worth checking further — NOT full detection (that's
/// [`is_streaming_app`], matched against the AX/OCR-observed `app_name`).
const NATIVE_APP_PROCESS_HINTS: &[&str] = &[
    "Netflix",
    "Disney+",
    "Hulu",
    "Prime Video",
    "TV", // Apple TV.app's process name on macOS
    "Peacock",
    "Paramount+",
    "HBO Max",
    "Crunchyroll",
];

/// True if a process with this exact name (case-insensitive) is currently
/// running. Cheap existence check — no window/visibility info.
#[cfg(target_os = "macos")]
fn is_process_running(name: &str) -> bool {
    std::process::Command::new("pgrep")
        .args(["-ix", name])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Is any streaming content plausibly on screen *right now*, checked using
/// ONLY process-existence + AppleScript (never ScreenCaptureKit)?
///
/// This is the pre-capture gate: macOS blanks/flickers DRM-protected video
/// (Netflix, Disney+, etc.) whenever ANY active ScreenCaptureKit session
/// exists on the same display — regardless of which specific window that
/// session targets. Discarding an already-captured frame (what
/// [`StreamingGate`] does) is NOT enough on its own: the OCR fallback's SCK
/// call for a completely unrelated window still triggers the blank/flicker
/// as long as it fires while a streaming window is anywhere on screen. This
/// check must run — and return true — BEFORE that OCR/SCK call, so the
/// engine can skip it entirely for the tick.
///
/// Deliberately conservative: a running native streaming app or an open
/// scriptable browser is treated as "maybe visible" even if backgrounded —
/// a false-positive here just means one skipped OCR tick, not incorrect
/// captured data.
#[cfg(target_os = "macos")]
pub fn any_streaming_content_visible() -> bool {
    if NATIVE_APP_PROCESS_HINTS
        .iter()
        .any(|&app| is_process_running(app))
    {
        return true;
    }
    for browser in scriptable_browsers() {
        if is_process_running(browser) {
            if let Some(url) = resolve_url_via_applescript(browser) {
                if is_streaming_url(&url) {
                    return true;
                }
            }
        }
    }
    false
}

#[cfg(not(target_os = "macos"))]
pub fn any_streaming_content_visible() -> bool {
    false
}

/// Sticky per-app streaming state across capture ticks.
///
/// Safari (and other browsers) don't reliably expose `AXDocument` /
/// `browser_url` on every tick — once the AX focus moves into a video
/// player's subtree (e.g. Netflix's closed-caption overlay), the window
/// title and document URL both come back empty, even though the same tab
/// is still playing the same streaming content. A pure per-frame check
/// would therefore let those frames slip through even with the setting
/// enabled.
///
/// This gate remembers the last-classified app + streaming state and
/// carries it forward when a later tick can't reconfirm the URL, as long
/// as the focused app hasn't changed. It resets as soon as either the app
/// changes or a fresh URL affirmatively proves we've navigated away.
#[derive(Default)]
pub struct StreamingGate {
    last_app: Option<String>,
    is_streaming: bool,
}

impl StreamingGate {
    pub fn new() -> Self {
        Self::default()
    }

    /// Decide whether this frame should be treated as streaming content.
    pub fn should_skip(&mut self, app_name: Option<&str>, url: Option<&str>) -> bool {
        let app = app_name.unwrap_or("");

        if is_streaming_content(app, url) {
            self.last_app = Some(app.to_string());
            self.is_streaming = true;
            return true;
        }

        // A real URL this tick that didn't match — an affirmative "navigated
        // away" signal within the same app, so clear the sticky flag.
        if url.is_some() {
            if self.last_app.as_deref() == Some(app) {
                self.is_streaming = false;
            }
            self.last_app = Some(app.to_string());
            return false;
        }

        // No URL this tick (extraction failed). Stay sticky only if the same
        // app is still focused — otherwise this is a genuine app switch.
        if self.is_streaming && self.last_app.as_deref() == Some(app) {
            return true;
        }

        self.last_app = Some(app.to_string());
        self.is_streaming = false;
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_streaming_app_detection() {
        assert!(is_streaming_app("Netflix"));
        assert!(is_streaming_app("netflix"));
        assert!(is_streaming_app("Disney+"));
        assert!(is_streaming_app("Hulu"));
        assert!(is_streaming_app("Prime Video"));
        assert!(is_streaming_app("Apple TV"));
        assert!(is_streaming_app("Peacock"));
        assert!(is_streaming_app("Paramount+"));
        assert!(is_streaming_app("HBO Max"));
        assert!(is_streaming_app("Max"));
        assert!(is_streaming_app("max"));

        // Negative tests
        assert!(!is_streaming_app("Finder"));
        assert!(!is_streaming_app("Safari"));
        assert!(!is_streaming_app("Chrome"));
        assert!(!is_streaming_app("Maximize")); // Should not match "max"
        assert!(!is_streaming_app("3ds Max")); // Graphics software
    }

    #[test]
    fn test_streaming_url_detection() {
        assert!(is_streaming_url("https://netflix.com/watch/12345"));
        assert!(is_streaming_url("https://www.netflix.com/browse"));
        assert!(is_streaming_url("https://disneyplus.com/video/abc"));
        assert!(is_streaming_url("https://hulu.com/watch"));
        assert!(is_streaming_url("https://primevideo.com/detail"));
        assert!(is_streaming_url("https://tv.apple.com/show/123"));
        assert!(is_streaming_url("https://peacocktv.com/watch"));
        assert!(is_streaming_url("https://paramountplus.com/shows"));
        assert!(is_streaming_url("https://play.max.com/movie/abc"));
        assert!(is_streaming_url("https://crunchyroll.com/watch"));
        assert!(is_streaming_url("http://netflix.com/watch/12345"));

        // Amazon special path
        assert!(is_streaming_url(
            "https://www.amazon.com/gp/video/detail/B0CXGTK4HY/ref=atv_hm"
        ));
        assert!(is_streaming_url(
            "https://amazon.com/gp/video/detail/something"
        ));

        // Subdomains
        assert!(is_streaming_url(
            "https://apps.disneyplus.com/il/shows/scrubs/123/watch"
        ));
        assert!(is_streaming_url("https://watch.hulu.com/show/abc"));

        // Negative tests
        assert!(!is_streaming_url("https://google.com"));
        assert!(!is_streaming_url("https://github.com"));
        assert!(!is_streaming_url("https://max.com")); // "max.com" ≠ "play.max.com"
        assert!(!is_streaming_url("https://amazon.com/dp/B09V3KXJPB")); // Not /gp/video/
        assert!(!is_streaming_url("https://amazon.com/s?k=headphones")); // Search, not video
    }

    #[test]
    fn test_combined_detection() {
        assert!(is_streaming_content("Netflix", None));
        assert!(is_streaming_content(
            "Google Chrome",
            Some("https://netflix.com/watch/123")
        ));
        assert!(!is_streaming_content("Finder", Some("https://google.com")));
        assert!(!is_streaming_content("Finder", None));
    }

    #[test]
    fn test_streaming_gate_basic_match() {
        let mut gate = StreamingGate::new();
        assert!(gate.should_skip(Some("Safari"), Some("https://www.netflix.com/watch/123")));
    }

    #[test]
    fn test_streaming_gate_sticky_when_url_missing_same_app() {
        // Simulates Safari's AX focus drifting into Netflix's caption overlay:
        // browser_url/window_name come back empty on later ticks, but it's
        // still the same app that was just confirmed as streaming.
        let mut gate = StreamingGate::new();
        assert!(gate.should_skip(Some("Safari"), Some("https://www.netflix.com/watch/123")));
        assert!(gate.should_skip(Some("Safari"), None));
        assert!(gate.should_skip(Some("Safari"), None));
    }

    #[test]
    fn test_streaming_gate_clears_on_app_switch() {
        let mut gate = StreamingGate::new();
        assert!(gate.should_skip(Some("Safari"), Some("https://www.netflix.com/watch/123")));
        // Switched to a different app entirely — no sticky carry-over.
        assert!(!gate.should_skip(Some("Code"), None));
        // Switching back to Safari without a reconfirmed URL should NOT
        // resurrect the old streaming state (different tick, no evidence).
        assert!(!gate.should_skip(Some("Safari"), None));
    }

    #[test]
    fn test_streaming_gate_clears_on_navigation_away() {
        let mut gate = StreamingGate::new();
        assert!(gate.should_skip(Some("Safari"), Some("https://www.netflix.com/watch/123")));
        // Same app, but a fresh URL proves we've navigated away.
        assert!(!gate.should_skip(Some("Safari"), Some("https://github.com")));
        // Sticky state should now be cleared for subsequent empty-URL ticks.
        assert!(!gate.should_skip(Some("Safari"), None));
    }

    #[test]
    fn test_streaming_gate_native_app_no_url_needed() {
        let mut gate = StreamingGate::new();
        assert!(gate.should_skip(Some("Netflix"), None));
        assert!(gate.should_skip(Some("Netflix"), None));
    }

    #[test]
    fn test_current_tab_url_script_safari_dialect() {
        let script = current_tab_url_script("Safari").unwrap();
        assert!(script.contains("current tab"));
        assert!(script.contains("Safari"));
    }

    #[test]
    fn test_current_tab_url_script_chromium_family() {
        for app in [
            "Google Chrome",
            "Arc",
            "Brave Browser",
            "Microsoft Edge",
            "Vivaldi",
            "Opera",
            "Chromium",
        ] {
            let script = current_tab_url_script(app)
                .unwrap_or_else(|| panic!("{app} should be a recognized Chromium-style browser"));
            assert!(script.contains("active tab"), "app={app}");
            assert!(script.contains(app), "app={app}");
        }
    }

    #[test]
    fn test_current_tab_url_script_unknown_browser() {
        // Firefox has no reliable AppleScript URL command — must return None,
        // not guess a syntax that would fail silently at runtime.
        assert!(current_tab_url_script("Firefox").is_none());
        assert!(current_tab_url_script("Finder").is_none());
    }

    #[test]
    fn test_current_tab_url_script_case_insensitive() {
        assert!(current_tab_url_script("safari").is_some());
        assert!(current_tab_url_script("google chrome").is_some());
    }
}
