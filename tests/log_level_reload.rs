//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Verifies the Settings "Log Level" control actually works: the daemon builds
// its tracing filter from settings.log_level and hot-reloads it at runtime
// (the poll loop calls observability::reload_log_level when the value changes,
// no restart). The OTel spool is the sole `tracing` sink now (see
// src/observability.rs's module doc), so this drives the real reload handle
// and asserts the spooled log records' verbosity changes accordingly by
// inspecting the raw OTLP bytes written to `~/.meridian/telemetry/pending/`.

use std::time::Duration;

#[tokio::test]
async fn log_level_hot_reload_changes_verbosity() {
    // Isolate from the dev machine: a temp settings.json + temp telemetry dir,
    // no RUST_LOG (an explicit override would correctly disable the
    // settings-driven filter). Capture is unconditional (MERIDIAN_TELEMETRY_DISABLED
    // unset), so this must run inside a Tokio runtime for the OTel batch
    // processor's background task.
    std::env::remove_var("RUST_LOG");
    let tmp = std::env::temp_dir().join(format!("meridian-loglevel-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).unwrap();
    let settings = tmp.join("settings.json");
    std::fs::write(&settings, r#"{"log_level":"INFO","otlp_enabled":false}"#).unwrap();
    std::env::set_var("MERIDIAN_SETTINGS_PATH", &settings);
    let telemetry_dir = tmp.join("telemetry");
    std::env::set_var("MERIDIAN_TELEMETRY_DIR", &telemetry_dir);

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

    // Flush the OTel log batch processor, then read the spooled files.
    guard.shutdown().await;
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Concatenate every spooled logs-*.otlp file's raw bytes — protobuf
    // encodes plain ASCII string fields verbatim, so a byte-level substring
    // check is enough without a full decode.
    let pending = telemetry_dir.join("pending");
    let body: Vec<u8> = std::fs::read_dir(&pending)
        .map(|entries| {
            entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| {
                    p.file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.starts_with("logs-"))
                })
                .flat_map(|p| std::fs::read(p).unwrap_or_default())
                .collect()
        })
        .unwrap_or_default();

    let _ = std::fs::remove_dir_all(&tmp);

    let contains = |needle: &str| body.windows(needle.len()).any(|w| w == needle.as_bytes());

    // INFO sanity line is always present.
    assert!(
        contains("INFO_SANITY"),
        "INFO event should be captured at the INFO default"
    );
    // Debug emitted BEFORE the reload (while at INFO) must be filtered out.
    assert!(
        !contains("DBG_BEFORE_RELOAD"),
        "a debug event at the INFO default must be filtered out",
    );
    // Debug emitted AFTER reloading to DEBUG must now appear — proving the
    // hot-reload took effect without a restart.
    assert!(
        contains("DBG_AFTER_RELOAD"),
        "a debug event after reloading to DEBUG must be captured",
    );
}
