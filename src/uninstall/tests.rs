//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Unit tests for `meridian uninstall`, split out of [`super`] to keep that
//! file under the 500-line guideline.
//!
//! Each test uses a `tempfile::TempDir` for its scratch space so cleanup runs
//! via `Drop` — including during panic unwinding, so a failing assertion never
//! leaks a directory under `/tmp`.

use super::*;
use tempfile::TempDir;

#[test]
fn label_matches_only_meridiona_plists() {
    assert_eq!(
        meridiona_label(Path::new("/x/com.meridiona.daemon.plist")).as_deref(),
        Some("com.meridiona.daemon")
    );
    assert_eq!(
        meridiona_label(Path::new("/x/com.meridiona.a11y-helper.plist")).as_deref(),
        Some("com.meridiona.a11y-helper")
    );
    assert_eq!(meridiona_label(Path::new("/x/com.apple.thing.plist")), None);
    assert_eq!(meridiona_label(Path::new("/x/com.meridiona.daemon")), None); // not a plist
    assert_eq!(meridiona_label(Path::new("/x/notes.txt")), None);
}

/// `--purge` widens all three scopes; `--remove-data --remove-runtime`
/// (without `--purge`) must NOT set `purge` — that is the guard behind the
/// full `rm -rf ~/.meridian` step, which is scoped to `--purge` only.
#[test]
fn purge_flag_is_not_implied_by_remove_data_plus_remove_runtime() {
    let s =
        |args: &[&str]| Flags::from_args(&args.iter().map(|a| a.to_string()).collect::<Vec<_>>());

    let purge = s(&["--purge"]);
    assert!(purge.purge);
    assert!(purge.remove_data && purge.remove_runtime && purge.remove_models);

    let both = s(&["--remove-data", "--remove-runtime"]);
    assert!(
        !both.purge,
        "--remove-data --remove-runtime must not imply --purge"
    );
    assert!(both.remove_data && both.remove_runtime);
    assert!(!both.remove_models);

    let neither = s(&["--dry-run"]);
    assert!(!neither.purge);
    assert!(!neither.remove_data && !neither.remove_runtime && !neither.remove_models);
}

#[test]
fn enumerates_only_meridiona_agents() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    for f in [
        "com.meridiona.daemon.plist",
        "com.meridiona.a11y-helper.plist",
        "com.apple.something.plist",
        "random.txt",
    ] {
        std::fs::write(dir.join(f), "x").unwrap();
    }
    let found: Vec<String> = meridiona_agent_plists(dir)
        .into_iter()
        .map(|(l, _)| l)
        .collect();
    assert_eq!(
        found,
        vec![
            "com.meridiona.a11y-helper".to_string(),
            "com.meridiona.daemon".to_string()
        ]
    );
}

#[test]
fn missing_launch_agents_dir_is_empty() {
    let tmp = TempDir::new().unwrap();
    // A path *inside* the temp dir that was never created.
    let missing = tmp.path().join("does-not-exist");
    assert!(meridiona_agent_plists(&missing).is_empty());
}

/// `data_items`/`runtime_items` must only list entries that actually exist
/// (a wizard checkbox should never advertise removing nothing), and must
/// stay disjoint from each other so the two checkboxes don't double-count.
#[test]
fn data_and_runtime_items_only_list_existing_entries_and_stay_disjoint() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path();
    let meridian = home.join(".meridian");
    std::fs::create_dir_all(&meridian).unwrap();
    std::fs::write(meridian.join(".env"), "x").unwrap();
    std::fs::write(meridian.join("meridian.db"), "x").unwrap();
    std::fs::create_dir_all(meridian.join("runtime")).unwrap();
    // "settings.json" and "mlx-server-venv" deliberately absent.

    let data = data_items(home);
    let runtime = runtime_items(&meridian);

    assert!(data.contains(&meridian.join(".env")));
    assert!(data.contains(&meridian.join("meridian.db")));
    assert!(!data.iter().any(|p| p.ends_with("settings.json")));
    assert!(runtime.contains(&meridian.join("runtime")));
    assert!(!runtime.iter().any(|p| p.ends_with("mlx-server-venv")));

    for p in &data {
        assert!(
            !runtime.contains(p),
            "data and runtime item lists must be disjoint: {p:?}"
        );
    }
}

/// On macOS, `data_items` also picks up the tray's OS-managed WebKit/AppKit
/// caches (keyed off `com.meridiona.tray`) when present, and ignores them when
/// absent — mirrors the on-disk existence filter every other item list uses.
#[cfg(target_os = "macos")]
#[test]
fn data_items_includes_app_caches_when_present() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path();
    let cache = home.join("Library/Caches/com.meridiona.tray");
    std::fs::create_dir_all(&cache).unwrap();
    // Saved Application State deliberately absent — must not appear.

    let data = data_items(home);
    assert!(data.contains(&cache));
    assert!(!data
        .iter()
        .any(|p| p.ends_with("com.meridiona.tray.savedState")));
}

/// The launch-at-login marker must be removed with the rest of the user data.
/// Left behind, a reinstall reads it, believes autostart is already configured,
/// and never re-registers the login item — so the app silently stops starting
/// at login and nothing reports an error.
#[test]
fn data_items_includes_the_autostart_marker() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path();
    let meridian = home.join(".meridian");
    std::fs::create_dir_all(&meridian).unwrap();
    std::fs::write(meridian.join("autostart_configured"), "").unwrap();

    assert!(data_items(home).contains(&meridian.join("autostart_configured")));
}

/// The Windows branch of [`app_cache_items`]: `%LOCALAPPDATA%`/`%APPDATA%`'s
/// `com.meridiona.tray` directories are where WebView2 keeps the cookies and
/// localStorage the signed-in session lives in, so an uninstall that skipped
/// them would come back still logged in. Exercised through `bundle_dirs_under`
/// on every platform rather than through the env vars, which are process-global
/// and would race the other tests.
#[test]
fn bundle_dirs_are_listed_only_when_they_exist() {
    let tmp = TempDir::new().unwrap();
    let local = tmp.path().join("Local");
    let roaming = tmp.path().join("Roaming");
    std::fs::create_dir_all(local.join("com.meridiona.tray")).unwrap();
    // `roaming` deliberately has no bundle dir — must not be listed.
    std::fs::create_dir_all(&roaming).unwrap();

    let found = bundle_dirs_under([local.clone(), roaming.clone()]);
    assert_eq!(found, vec![local.join("com.meridiona.tray")]);
}

/// `model_items` only returns catalog entries that exist on disk, and never
/// invents a path for a model that isn't in [`MODEL_CATALOG`].
#[test]
fn model_items_filters_to_existing_catalog_dirs() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path();
    let hub = home.join(".cache/huggingface/hub");
    std::fs::create_dir_all(&hub).unwrap();
    std::fs::create_dir_all(hub.join("models--mlx-community--Qwen3.5-2B-OptiQ-4bit")).unwrap();
    // A non-Meridian model the user downloaded for another tool — must be ignored.
    std::fs::create_dir_all(hub.join("models--some-other-org--unrelated-model")).unwrap();

    let found = model_items(home);
    assert_eq!(found.len(), 1);
    assert!(found[0].ends_with("models--mlx-community--Qwen3.5-2B-OptiQ-4bit"));
}

#[test]
fn remove_path_handles_files_and_directories() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();

    let file = dir.join("a-file");
    std::fs::write(&file, "x").unwrap();
    remove_path(&file).unwrap();
    assert!(!file.exists());

    let subdir = dir.join("a-dir");
    std::fs::create_dir_all(subdir.join("nested")).unwrap();
    remove_path(&subdir).unwrap();
    assert!(!subdir.exists());

    // A path that doesn't exist reports NotFound rather than panicking.
    assert_eq!(
        remove_path(&dir.join("missing")).unwrap_err().kind(),
        std::io::ErrorKind::NotFound
    );
}
