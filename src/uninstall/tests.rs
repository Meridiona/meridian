//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Unit tests for `meridian uninstall`, split out of [`super`] to keep that
//! file under the 500-line guideline.

use super::*;

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
    let dir = std::env::temp_dir().join("meridian-uninstall-test-agents");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for f in [
        "com.meridiona.daemon.plist",
        "com.meridiona.a11y-helper.plist",
        "com.apple.something.plist",
        "random.txt",
    ] {
        std::fs::write(dir.join(f), "x").unwrap();
    }
    let found: Vec<String> = meridiona_agent_plists(&dir)
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
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn missing_launch_agents_dir_is_empty() {
    let dir = std::env::temp_dir().join("meridian-uninstall-test-nope-xyz");
    let _ = std::fs::remove_dir_all(&dir);
    assert!(meridiona_agent_plists(&dir).is_empty());
}

/// `data_items`/`runtime_items` must only list entries that actually exist
/// (a wizard checkbox should never advertise removing nothing), and must
/// stay disjoint from each other so the two checkboxes don't double-count.
#[test]
fn data_and_runtime_items_only_list_existing_entries_and_stay_disjoint() {
    let dir = std::env::temp_dir().join(format!(
        "meridian-uninstall-test-items-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(".env"), "x").unwrap();
    std::fs::write(dir.join("meridian.db"), "x").unwrap();
    std::fs::create_dir_all(dir.join("runtime")).unwrap();
    // "settings.json" and "mlx-server-venv" deliberately absent.

    let data = data_items(&dir);
    let runtime = runtime_items(&dir);

    assert!(data.contains(&dir.join(".env")));
    assert!(data.contains(&dir.join("meridian.db")));
    assert!(!data.iter().any(|p| p.ends_with("settings.json")));
    assert!(runtime.contains(&dir.join("runtime")));
    assert!(!runtime.iter().any(|p| p.ends_with("mlx-server-venv")));

    for p in &data {
        assert!(
            !runtime.contains(p),
            "data and runtime item lists must be disjoint: {p:?}"
        );
    }

    std::fs::remove_dir_all(&dir).ok();
}

/// `model_items` only returns catalog entries that exist on disk, and never
/// invents a path for a model that isn't in [`MODEL_CATALOG`].
#[test]
fn model_items_filters_to_existing_catalog_dirs() {
    let home = std::env::temp_dir().join(format!(
        "meridian-uninstall-test-models-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&home);
    let hub = home.join(".cache/huggingface/hub");
    std::fs::create_dir_all(&hub).unwrap();
    std::fs::create_dir_all(hub.join("models--mlx-community--Qwen3.5-2B-OptiQ-4bit")).unwrap();
    // A non-Meridian model the user downloaded for another tool — must be ignored.
    std::fs::create_dir_all(hub.join("models--some-other-org--unrelated-model")).unwrap();

    let found = model_items(&home);
    assert_eq!(found.len(), 1);
    assert!(found[0].ends_with("models--mlx-community--Qwen3.5-2B-OptiQ-4bit"));

    std::fs::remove_dir_all(&home).ok();
}

#[test]
fn remove_path_handles_files_and_directories() {
    let dir = std::env::temp_dir().join(format!(
        "meridian-uninstall-test-remove-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

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

    std::fs::remove_dir_all(&dir).ok();
}
