//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Loads the menu-bar tray icon: a dedicated Meridian glyph rendered as a
//! macOS **template image** (RGB ignored; macOS tints it to the menu-bar
//! foreground and modulates by alpha), the same convention system icons and
//! well-behaved third-party menu-bar apps use.
//!
//! `icons/tray-template.png` is a pre-baked 44×44 (22 pt @2x) silhouette
//! derived from `icons/tray-icon-source.png` by `create-icons.sh` — a
//! purpose-built bold/opaque redraw of the brand mark, not a crop of the
//! full-color app icon. `isTemplate` rendering is structurally monochrome
//! (color and gradients don't survive it), and the gradient app icon's
//! hairline strokes wash out to near-nothing at menu-bar size, so — same as
//! every app with a detailed icon — the menu bar gets its own glyph.
//! Regenerate via `create-icons.sh` if either source changes; this module
//! only decodes and hands off the baked asset.
//!
//! # Who calls this
//! `lib.rs`'s tray setup → [`logo_image`] → `TrayIconBuilder::icon`.

use tauri::image::Image;

static TRAY_TEMPLATE_PNG: &[u8] = include_bytes!("../icons/tray-template.png");

/// Decode the baked menu-bar template icon.
pub fn logo_image() -> Image<'static> {
    Image::from_bytes(TRAY_TEMPLATE_PNG)
        .expect("icons/tray-template.png must be a valid PNG (regenerate via create-icons.sh)")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_to_expected_size() {
        let img = logo_image();
        assert_eq!(img.width(), 44);
        assert_eq!(img.height(), 44);
        assert_eq!(img.rgba().len(), (44 * 44 * 4) as usize);
    }

    #[test]
    fn has_both_opaque_and_transparent_pixels() {
        let img = logo_image();
        let rgba = img.rgba();
        let any_opaque = rgba.chunks_exact(4).any(|px| px[3] > 0);
        let any_transparent = rgba.chunks_exact(4).any(|px| px[3] == 0);
        assert!(any_opaque, "template icon should have a visible silhouette");
        assert!(
            any_transparent,
            "template icon should punch out the glyph/background"
        );
    }
}
