//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Verifies the Settings "Log Level" control actually works: the daemon builds
// its tracing filter from settings.log_level and hot-reloads it at runtime
// (the poll loop calls observability::reload_log_level when the value changes,
// no restart). This drives the real reload handle and asserts the file sink's
// verbosity changes accordingly.

use std::time::Duration;

#[test]
fn log_level_hot_reload_changes_verbosity() {
    // Isolate from the dev machine: a temp settings.json (export off so init
    // builds no OTLP exporter) + temp log dir, and no RUST_LOG (an explicit
    // override would correctly disable the settings-driven filter).
    std::env::remove_var("RUST_LOG");
    let tmp = std::env::temp_dir().join(format!("meridian-loglevel-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).unwrap();
    let settings = tmp.join("settings.json");
    std::fs::write(&settings, r#"{"log_level":"INFO","otlp_enabled":false}"#).unwrap();
    std::env::set_var("MERIDIAN_SETTINGS_PATH", &settings);
    std::env::set_var("MERIDIAN_LOG_DIR", &tmp);

    let guard = meridian::observability::init("loglevel-test").expect("init observability");

    // Default level is INFO → a debug event on the `meridian` target is dropped.
    tracing::debug!(target: "meridian", "DBG_BEFORE_RELOAD");
    tracing::info!(target: "meridian", "INFO_SANITY");

    // Flip to DEBUG at runtime — this is exactly what the poll loop does when
    // the user changes Log Level in Settings. Returns true when applied.
    assert!(
        meridian::observability::reload_log_level("DEBUG"),
        "reload_log_level should apply when RUST_LOG is unset and the handle is initialised",
    );
    tracing::debug!(target: "meridian", "DBG_AFTER_RELOAD");

    // Flush the non-blocking file writer, then read the daily-rolled jsonl.
    drop(guard);
    std::thread::sleep(Duration::from_millis(300));

    let body = std::fs::read_dir(&tmp)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("loglevel-test.jsonl"))
        })
        .map(|p| std::fs::read_to_string(p).unwrap_or_default())
        .collect::<String>();

    let _ = std::fs::remove_dir_all(&tmp);

    // INFO sanity line is always present.
    assert!(
        body.contains("INFO_SANITY"),
        "INFO event should be logged at the INFO default"
    );
    // Debug emitted BEFORE the reload (while at INFO) must be filtered out.
    assert!(
        !body.contains("DBG_BEFORE_RELOAD"),
        "a debug event at the INFO default must be filtered out",
    );
    // Debug emitted AFTER reloading to DEBUG must now appear — proving the
    // hot-reload took effect without a restart.
    assert!(
        body.contains("DBG_AFTER_RELOAD"),
        "a debug event after reloading to DEBUG must be logged",
    );
}
