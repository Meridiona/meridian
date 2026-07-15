//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! "What's New" changelog + roadmap — curated (NOT the auto-generated
//! `CHANGELOG.md`, which is commit-level and too internal to show end users)
//! release notes surfaced in the dashboard, plus a small hand-maintained
//! roadmap list. See `tray/src-tauri/resources/whats-new.json` for the content.
//!
//! # Who calls this
//! - [`get_whats_new`] / [`mark_whats_new_seen_cmd`]: the `WhatsNewModal`
//!   (`ui/components/timeline/WhatsNewModal.tsx`), opened via the toolbar nav
//!   pill or a `/whats-new` deep link.
//! - [`has_unseen_release_notes`] / [`mark_whats_new_seen`]: the poll loop's
//!   once-per-version auto-open (`crate::poll`'s `whats_new_auto_open`).
//!
//! # Related
//! - `tray/src-tauri/resources/whats-new.json` — the content; update it per
//!   release (see CLAUDE.md's "Add a What's New entry per release").
//! - [`crate::poll::plan_auto_open`] — the sibling daily auto-open this
//!   module's marker-file pattern is copied from.

use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::OnceLock;

/// One release's user-facing notes.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ReleaseNote {
    pub version: String,
    pub date: String,
    pub highlights: Vec<String>,
    pub fixes: Vec<String>,
}

/// One roadmap item. `status` is `in-progress` | `planned` | `considering`.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RoadmapItem {
    pub title: String,
    pub status: String,
    pub description: String,
}

/// The full What's New payload — releases newest-first, plus the roadmap.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct WhatsNewData {
    pub releases: Vec<ReleaseNote>,
    pub roadmap: Vec<RoadmapItem>,
}

/// Compiled into the binary — always in sync with the build, no packaged-app
/// resource-path resolution to get wrong (unlike Tauri's `resources` bundling).
const WHATS_NEW_JSON: &str = include_str!("../../resources/whats-new.json");

static PARSED: OnceLock<WhatsNewData> = OnceLock::new();

/// Parse the embedded JSON once and cache it. A malformed file would be a
/// build-time bug — [`whats_new_json_parses`] below guards against ever
/// shipping one — so this only ever runs against content already proven valid.
fn parse_whats_new() -> &'static WhatsNewData {
    PARSED.get_or_init(|| {
        serde_json::from_str(WHATS_NEW_JSON)
            .expect("resources/whats-new.json must be valid JSON matching WhatsNewData")
    })
}

/// Full changelog + roadmap for the "What's New" modal.
#[tauri::command]
#[tracing::instrument]
pub fn get_whats_new() -> WhatsNewData {
    parse_whats_new().clone()
}

const LAST_SEEN_FILE: &str = "last_seen_version";

/// `true` when `current_version` hasn't been marked seen yet — an absent file
/// (never shown, or upgrading from a version that predates this feature) or a
/// stored version that doesn't match the running one both count as unseen.
pub fn has_unseen_release_notes(home: &Path, current_version: &str) -> bool {
    let path = home.join(".meridian").join(LAST_SEEN_FILE);
    match std::fs::read_to_string(&path) {
        Ok(seen) => seen.trim() != current_version,
        Err(_) => true,
    }
}

/// Write `current_version` to `~/.meridian/last_seen_version`, mirroring
/// `commands::setup::mark_setup_complete`'s write pattern. Idempotent.
pub fn mark_whats_new_seen(home: &Path, current_version: &str) -> std::io::Result<()> {
    let dir = home.join(".meridian");
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join(LAST_SEEN_FILE), current_version)
}

/// Frontend entry point — called when the modal closes (whether it was
/// auto-opened or nav-opened), so "seen" always reflects the version actually
/// viewed rather than only the version that triggered the auto-open.
#[tauri::command]
#[tracing::instrument(skip(app))]
pub fn mark_whats_new_seen_cmd(app: tauri::AppHandle) {
    let Ok(home) = std::env::var("HOME") else {
        return;
    };
    let current = app.package_info().version.to_string();
    if let Err(e) = mark_whats_new_seen(Path::new(&home), &current) {
        tracing::warn!(error = %e, "whats-new: failed to write last_seen_version");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whats_new_json_parses() {
        let data: WhatsNewData = serde_json::from_str(WHATS_NEW_JSON)
            .expect("resources/whats-new.json must be valid JSON");
        assert!(!data.releases.is_empty(), "seed at least one release");
    }

    #[test]
    fn unseen_when_marker_absent() {
        let dir =
            std::env::temp_dir().join(format!("meridian-whats-new-{}-absent", std::process::id()));
        assert!(has_unseen_release_notes(&dir, "1.71.0"));
    }

    #[test]
    fn unseen_when_marker_mismatches() {
        let dir = std::env::temp_dir().join(format!(
            "meridian-whats-new-{}-mismatch",
            std::process::id()
        ));
        std::fs::create_dir_all(dir.join(".meridian")).unwrap();
        std::fs::write(dir.join(".meridian").join(LAST_SEEN_FILE), "1.70.0").unwrap();
        assert!(has_unseen_release_notes(&dir, "1.71.0"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn seen_round_trips() {
        let dir =
            std::env::temp_dir().join(format!("meridian-whats-new-{}-seen", std::process::id()));
        mark_whats_new_seen(&dir, "1.71.0").unwrap();
        assert!(!has_unseen_release_notes(&dir, "1.71.0"));
        assert!(has_unseen_release_notes(&dir, "1.72.0"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
