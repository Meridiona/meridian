//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Resolves a macOS app's *real* icon for the "Time by app" UI, instead of a
//! letter-monogram placeholder — reads the icon straight out of the app's own
//! `.app` bundle via `NSWorkspace.icon(forFile:)`, the same API every native
//! app switcher/activity tracker uses (Contexts, AltTab, Raycast). Never
//! fetches anything from the network: Meridian's ETL only stores a display
//! name (`app_name`), not a bundle identifier, so this resolves purely by
//! matching that name against the standard app-install directories — no
//! schema/capture-layer changes needed.
//!
//! # Who calls this
//! `get_app_icon` command, invoked once per distinct `app_name` by the UI's
//! `AppGlyph` (`ui/components/atoms.tsx`) via `ui/lib/app-icons.ts`.
//!
//! # Related
//! - [`crate::commands::system`] — sibling OS-facing command module this
//!   follows the pattern of.
//! - `ui/lib/brand-icons.ts` — the hand-picked wordmark fallback used while
//!   this command's result is loading, or if it returns `None`.

use std::path::PathBuf;

/// Standard install locations to search, in preference order. Per-user
/// `~/Applications` is appended at call time (needs `$HOME`).
const APP_DIRS: &[&str] = &[
    "/Applications",
    "/System/Applications",
    "/System/Applications/Utilities",
    "/Applications/Utilities",
];

/// Best-effort resolve a display name (e.g. `"Google Chrome"`) to an
/// installed `.app` bundle path. Tries the exact `<name>.app` filename in
/// each standard directory first (covers the overwhelming majority of real
/// apps), then falls back to a case-insensitive scan for a mismatched-case
/// bundle name.
///
/// Only reached from [`get_app_icon`]'s macOS branch — `allow(dead_code)`
/// off-macOS keeps that from tripping `-D warnings` in CI's Linux build.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn resolve_bundle_path(app_name: &str, home: &str) -> Option<PathBuf> {
    let mut dirs: Vec<PathBuf> = APP_DIRS.iter().map(PathBuf::from).collect();
    dirs.push(PathBuf::from(home).join("Applications"));
    resolve_bundle_path_in(&dirs, app_name)
}

/// Same lookup as [`resolve_bundle_path`] against an explicit directory list —
/// split out so tests can search a hermetic fixture dir instead of the real
/// `/Applications` (whatever happens to be installed on the dev machine).
fn resolve_bundle_path_in(dirs: &[PathBuf], app_name: &str) -> Option<PathBuf> {
    for dir in dirs {
        let guess = dir.join(format!("{app_name}.app"));
        if guess.is_dir() {
            return Some(guess);
        }
    }

    let needle = app_name.to_lowercase();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("app") {
                continue;
            }
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or_default();
            if stem.to_lowercase() == needle {
                return Some(path);
            }
        }
    }
    None
}

/// Sanitize an app name into a safe cache filename — app names can contain
/// spaces/slashes (e.g. browser profile suffixes), neither of which belong
/// in a path segment.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn cache_key(app_name: &str) -> String {
    app_name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn icon_cache_dir() -> Option<PathBuf> {
    std::env::var("HOME")
        .ok()
        .map(|h| PathBuf::from(h).join(".meridian/icon-cache"))
}

#[cfg(target_os = "macos")]
mod macos {
    use objc2::rc::Retained;
    use objc2_app_kit::{NSBitmapImageFileType, NSBitmapImageRep, NSImage, NSWorkspace};
    use objc2_foundation::{NSDictionary, NSString};
    use std::path::Path;

    /// Extract the bundle's real icon as PNG bytes via
    /// `NSWorkspace.icon(forFile:)` — the exact call every native macOS app
    /// switcher / activity tracker uses to show per-app icons.
    pub(super) fn extract_icon_png(bundle_path: &Path) -> Option<Vec<u8>> {
        let path_str = bundle_path.to_str()?;
        unsafe {
            let workspace = NSWorkspace::sharedWorkspace();
            let ns_path = NSString::from_str(path_str);
            let image: Retained<NSImage> = workspace.iconForFile(&ns_path);
            let tiff = image.TIFFRepresentation()?;
            let bitmap = NSBitmapImageRep::imageRepWithData(&tiff)?;
            let props = NSDictionary::new();
            let png =
                bitmap.representationUsingType_properties(NSBitmapImageFileType::PNG, &props)?;
            Some(png.to_vec())
        }
    }
}

/// Resolve `app_name`'s real macOS app icon, caching the PNG to
/// `~/.meridian/icon-cache/`. Returns the cached file's absolute path (the
/// frontend wraps it with Tauri's `convertFileSrc` to render it), or `None`
/// if the app can't be resolved to an installed bundle — the UI falls back
/// to a letter monogram in that case (uninstalled app, sandboxed
/// unrecognized name, non-macOS build, etc).
#[tauri::command]
#[tracing::instrument]
pub fn get_app_icon(app_name: String) -> Result<Option<String>, String> {
    if app_name.trim().is_empty() {
        return Ok(None);
    }

    #[cfg(not(target_os = "macos"))]
    {
        tracing::debug!("app icon extraction is macOS-only");
        Ok(None)
    }

    #[cfg(target_os = "macos")]
    {
        let cache_dir = icon_cache_dir().ok_or_else(|| "HOME not set".to_string())?;
        std::fs::create_dir_all(&cache_dir).map_err(|e| e.to_string())?;

        let cache_path = cache_dir.join(format!("{}.png", cache_key(&app_name)));
        if cache_path.is_file() {
            tracing::debug!(cached = true, "app icon resolved");
            return Ok(Some(cache_path.to_string_lossy().into_owned()));
        }

        let home = std::env::var("HOME").map_err(|e| e.to_string())?;
        let Some(bundle_path) = resolve_bundle_path(&app_name, &home) else {
            tracing::debug!("no installed bundle matched app_name");
            return Ok(None);
        };

        let Some(png) = macos::extract_icon_png(&bundle_path) else {
            tracing::warn!(?bundle_path, "NSWorkspace icon extraction failed");
            return Ok(None);
        };

        std::fs::write(&cache_path, png).map_err(|e| e.to_string())?;
        tracing::debug!(cached = false, ?bundle_path, "app icon resolved");
        Ok(Some(cache_path.to_string_lossy().into_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_exact_filename_match() {
        let tmp = std::env::temp_dir().join("meridian-icon-test-exact");
        std::fs::create_dir_all(tmp.join("Google Chrome.app")).unwrap();

        let found = resolve_bundle_path_in(std::slice::from_ref(&tmp), "Google Chrome");
        assert_eq!(found, Some(tmp.join("Google Chrome.app")));

        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn resolves_case_insensitive_fallback() {
        let tmp = std::env::temp_dir().join("meridian-icon-test-case");
        std::fs::create_dir_all(tmp.join("figma.app")).unwrap();

        // On APFS's default case-insensitive mode the exact-filename fast
        // path already matches "Figma.app" against the real "figma.app", so
        // this only meaningfully exercises the scan fallback on a
        // case-sensitive volume. Assert on the resolved stem rather than the
        // literal path so the test holds either way.
        let found = resolve_bundle_path_in(std::slice::from_ref(&tmp), "Figma");
        let stem = found
            .as_deref()
            .and_then(|p| p.file_stem())
            .and_then(|s| s.to_str())
            .map(str::to_lowercase);
        assert_eq!(stem.as_deref(), Some("figma"));

        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn returns_none_for_unknown_app() {
        let tmp = std::env::temp_dir().join("meridian-icon-test-missing");
        std::fs::create_dir_all(&tmp).unwrap();

        let found = resolve_bundle_path_in(std::slice::from_ref(&tmp), "Definitely Not Installed");
        assert_eq!(found, None);

        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn cache_key_sanitizes_special_chars() {
        assert_eq!(cache_key("Google Chrome"), "google_chrome");
        assert_eq!(cache_key("Visual Studio Code"), "visual_studio_code");
    }
}
