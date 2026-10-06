//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! The two commands the writing key's on-screen icon calls.
//!
//! Kept here rather than in `compose` because `generate_handler!` has to see them on every
//! platform; on anything but macOS they do nothing.
//!
//! # Who calls this
//! `tray/src/badge.html`, the page inside the `compose-badge` window.
//!
//! # Related
//! - [`crate::compose`] - the macOS writing key these forward to.

/// The icon was clicked. Same as tapping the writing key.
#[tauri::command]
pub fn compose_badge_click() {
    #[cfg(target_os = "macos")]
    {
        tracing::info!("compose: badge clicked");
        crate::compose::press();
    }
}

/// The icon's page reports what it saw, so a click that never arrives can be told apart from
/// one that arrives and fails. The text is one of the page's own fixed notes.
#[tauri::command]
pub fn compose_badge_log(detail: String) {
    tracing::info!(
        detail = %detail.chars().take(120).collect::<String>(),
        "compose: badge page"
    );
}
