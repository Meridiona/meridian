//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Rasterizes the menu-bar progress-ring icon — the `◔` glyph in the design's
//! tray pill, whose fill tracks the current task's completion.
//!
//! The image is a **template** (RGB ignored; macOS tints it to the menu-bar
//! foreground and modulates by alpha), so we encode the glyph purely in the
//! alpha channel: the completed arc at full alpha, the remaining track faint.
//! Built as raw RGBA — no PNG encoding — and handed straight to
//! [`tauri::image::Image`], which the tray's 1 s ticker swaps in via `set_icon`
//! whenever the percentage bucket changes.
//!
//! # Who calls this
//! `lib.rs`'s menu-bar ticker → [`ring_image`] → `TrayIcon::set_icon`.
//!
//! # Related
//! - [`crate::state::AppState::task_percent`] — the fill this renders.

use tauri::image::Image;

/// Icon side in pixels — 18 pt menu-bar height at @2x.
const SIZE: u32 = 36;
/// 3×3 supersampling per pixel for smooth (anti-aliased) edges.
const SS: i32 = 3;

const ALPHA_PROGRESS: f64 = 255.0; // completed arc — solid
const ALPHA_TRACK: f64 = 70.0; // remaining arc — faint
const ALPHA_UNFILLED: f64 = 165.0; // whole ring when there's no percentage

/// Render the ring for `percent` (`Some` in `[0,1]` → a filled arc from 12
/// o'clock clockwise; `None` → a uniform un-filled ring). Returns an owned
/// template [`Image`] ready for `TrayIcon::set_icon`.
pub fn ring_image(percent: Option<f64>) -> Image<'static> {
    let w = SIZE as i32;
    let h = SIZE as i32;
    let cx = w as f64 / 2.0 - 0.5;
    let cy = h as f64 / 2.0 - 0.5;
    let outer = w as f64 * 0.42;
    let inner = outer - w as f64 * 0.13; // ~4.7 px stroke at 36 px

    let p = percent.map(|p| p.clamp(0.0, 1.0));
    let mut rgba = vec![0u8; (w * h * 4) as usize];

    for y in 0..h {
        for x in 0..w {
            let mut acc = 0.0;
            for sy in 0..SS {
                for sx in 0..SS {
                    let px = x as f64 + (sx as f64 + 0.5) / SS as f64;
                    let py = y as f64 + (sy as f64 + 0.5) / SS as f64;
                    acc += sample_alpha(px - cx, py - cy, inner, outer, p);
                }
            }
            let alpha = (acc / (SS * SS) as f64).round() as u8;
            let i = ((y * w + x) * 4) as usize;
            // Black RGB; the template tint comes from macOS, shape from alpha.
            rgba[i + 3] = alpha;
        }
    }

    Image::new_owned(rgba, SIZE, SIZE)
}

/// Alpha contribution of one sub-sample at offset `(dx, dy)` from the centre.
/// Zero outside the ring band; otherwise progress vs track vs uniform per `p`.
fn sample_alpha(dx: f64, dy: f64, inner: f64, outer: f64, p: Option<f64>) -> f64 {
    let r = (dx * dx + dy * dy).sqrt();
    if r < inner || r > outer {
        return 0.0;
    }
    match p {
        None => ALPHA_UNFILLED,
        Some(p) => {
            // Angle from 12 o'clock, clockwise, normalised to [0,1).
            let mut frac = dx.atan2(-dy) / std::f64::consts::TAU;
            if frac < 0.0 {
                frac += 1.0;
            }
            if frac <= p {
                ALPHA_PROGRESS
            } else {
                ALPHA_TRACK
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn produces_rgba_buffer() {
        let img = ring_image(Some(0.7));
        // Image exposes its raw bytes; just assert the buffer is the right size.
        assert_eq!(img.rgba().len(), (SIZE * SIZE * 4) as usize);
    }

    #[test]
    fn full_ring_has_some_opaque_pixels() {
        let img = ring_image(None);
        let any_opaque = img.rgba().chunks_exact(4).any(|px| px[3] > 0);
        assert!(any_opaque);
    }
}
