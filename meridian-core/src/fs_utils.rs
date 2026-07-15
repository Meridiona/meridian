//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Small filesystem helpers shared across the daemon and the tray.
//!
//! The one thing here today is [`atomic_write_json`] — the crash-safe
//! "serialise → temp file → atomic rename" idiom. It was hand-rolled in at
//! least three places ([`crate::settings::write_settings_value`], the tray's
//! `analytics::save_state`, and `commands::account::save_account_email`); this
//! is the single tested implementation they now all call, so the
//! crash-safety-sensitive sequence lives in exactly one place.
//!
//! # Who calls this
//! - [`crate::settings::write_settings_value`] — `settings.json`.
//! - `tray/src-tauri/src/analytics.rs::save_state` — the PostHog state file.
//! - `tray/src-tauri/src/commands/account.rs::save_account_email` — the
//!   signed-in account file (via `spawn_blocking`, since it's a sync fn called
//!   from an async command).

use anyhow::Context;
use serde::Serialize;
use std::io::Write;
use std::path::Path;

/// Crash-safely persist `value` as pretty (2-space) JSON to `path`.
///
/// Creates the parent directory if needed, serialises to a temp file **in the
/// same directory** (so the rename stays on one filesystem and is therefore
/// atomic), then renames it over `path`. A crash mid-write can only ever leave
/// the stale `path` or the temp file — never a truncated `path`.
pub fn atomic_write_json<T: Serialize>(path: &Path, value: &T) -> anyhow::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating dir {}", dir.display()))?;
    }
    let json = serde_json::to_string_pretty(value).context("serialising JSON")?;
    // Temp file in the SAME directory so the rename is atomic (same filesystem).
    let tmp = path.with_extension("json.tmp");
    {
        let mut f = std::fs::File::create(&tmp)
            .with_context(|| format!("creating temp file {}", tmp.display()))?;
        f.write_all(json.as_bytes())
            .with_context(|| format!("writing temp file {}", tmp.display()))?;
        // fsync the data to disk BEFORE the rename. `rename` only makes the
        // directory entry swap atomic; without this flush, a crash/power-loss
        // right after the rename could persist the new dir entry while the data
        // blocks are still buffered, leaving `path` empty or torn.
        f.sync_all()
            .with_context(|| format!("flushing temp file {}", tmp.display()))?;
    }
    std::fs::rename(&tmp, path)
        .with_context(|| format!("atomically replacing {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Serialize, Deserialize, PartialEq, Debug)]
    struct Sample {
        name: String,
        n: u32,
    }

    #[test]
    fn writes_and_creates_parent_dir() {
        let dir =
            std::env::temp_dir().join(format!("meridian-fs-utils-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // Nested path whose parent does not exist yet.
        let path = dir.join("nested").join("sample.json");

        let value = Sample {
            name: "meridian".into(),
            n: 42,
        };
        atomic_write_json(&path, &value).unwrap();

        let round: Sample = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(round, value);
        // No temp file left behind.
        assert!(!path.with_extension("json.tmp").exists());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn overwrites_existing_atomically() {
        let dir = std::env::temp_dir().join(format!(
            "meridian-fs-utils-test-overwrite-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("sample.json");

        atomic_write_json(
            &path,
            &Sample {
                name: "a".into(),
                n: 1,
            },
        )
        .unwrap();
        atomic_write_json(
            &path,
            &Sample {
                name: "b".into(),
                n: 2,
            },
        )
        .unwrap();

        let round: Sample = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            round,
            Sample {
                name: "b".into(),
                n: 2
            }
        );

        std::fs::remove_dir_all(&dir).ok();
    }
}
