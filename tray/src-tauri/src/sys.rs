//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Small shared runtime helpers used across the tray: the current uid, the
//! dashboard base URL, and the native-notification shim.
//!
//! These each had 2–3 copy-pasted definitions scattered across `lib.rs`,
//! `commands`, `poll`, and `health` before this module consolidated them.
//!
//! # Related
//! - [`crate::poll`] — the loop that toasts via [`notify`].
//! - [`crate::install`] — install-mode + path resolution (a separate concern from these).

use tauri_plugin_notification::NotificationExt;

/// The current user's numeric uid as a string (for `launchctl gui/<uid>/…`
/// domain targets). Falls back to `"501"` (the first macOS user) if `id -u`
/// can't be read — better than failing the whole launchctl call.
pub fn uid_str() -> String {
    std::process::Command::new("id")
        .arg("-u")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "501".to_string())
}

/// Show a native macOS toast (title + body).
///
/// v1 shows title + body only. Producers populate a `deep_link` (e.g. /plan,
/// /worklogs) and the in-app banner channel renders it as an "Open →" link, but
/// click-to-navigate on a native toast needs Tauri notification actions + a
/// focus/navigate handler — deferred. The two channels are intentionally
/// asymmetric here; the banner carries the link.
pub fn notify(app: &tauri::AppHandle, title: &str, body: &str) {
    let _ = app.notification().builder().title(title).body(body).show();
}
