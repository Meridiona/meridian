//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Diagnostics export — the tray's "Export Diagnostics" Settings action.
//!
//! Bundles the local telemetry spool (`~/.meridian/telemetry/{pending,sent}`)
//! plus recent JSONL logs into a `.tar.gz` via
//! [`meridian::telemetry_spool::build_export_bundle`] — the exact same
//! implementation `meridian telemetry export` uses — and reveals it in
//! Finder. This is the shipped product's ONLY path to a developer's
//! OpenObserve: a Canonical/packaged install never ships telemetry live (see
//! `src/observability.rs`'s `resolve_otlp_target`/`is_canonical_install`), so
//! a user hands this file to support, who imports it with
//! `meridian telemetry import <bundle> --endpoint <their-oo-url> --auth <...>`.
//!
//! # Who calls this
//! `export_diagnostics_bundle` is registered in `lib.rs`'s `invoke_handler!`;
//! consumed by `ui/components/timeline/settings/AccountSection.tsx`'s
//! "Export Diagnostics" button via `mutate`/`load` in `@/lib/bridge`.
//!
//! # Related
//! - [`crate::commands::settings`] — the Advanced Settings section this lives in.
//! - `src/telemetry_spool/cli.rs` — the CLI counterpart (`meridian telemetry export`).
//! - `src/telemetry_spool/mod.rs` — `build_export_bundle`, the shared implementation.

use tauri_plugin_opener::OpenerExt;

/// Bundle local telemetry (`.otlp` spool files + recent JSONL logs) into a
/// `.tar.gz` under `~/Downloads` and reveal it in Finder. Returns the
/// resulting file path so the UI can show it to the user.
#[tracing::instrument(skip(app))]
#[tauri::command]
pub async fn export_diagnostics_bundle(app: tauri::AppHandle) -> Result<String, String> {
    let (path, file_count) = tokio::task::spawn_blocking(move || {
        let out_path = default_export_path().map_err(|e| e.to_string())?;
        meridian::telemetry_spool::build_export_bundle(Some(&out_path), None, true)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("export task panicked: {e}"))??;

    tracing::info!(
        path = %path.display(),
        file_count,
        "diagnostics bundle exported"
    );

    if let Err(e) = app.opener().reveal_item_in_dir(&path) {
        tracing::warn!(error = %e, "could not reveal diagnostics bundle in Finder");
    }

    Ok(path.to_string_lossy().into_owned())
}

/// `~/Downloads/meridian-diagnostics-<unix_micros>.tar.gz` — a location every
/// user can find without knowing about `~/.meridian/telemetry/`.
fn default_export_path() -> anyhow::Result<std::path::PathBuf> {
    let home = meridian_core::paths::home_dir()
        .ok_or_else(|| anyhow::anyhow!("home directory could not be resolved"))?;
    let dir = home.join("Downloads");
    std::fs::create_dir_all(&dir)?;
    let micros = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros();
    Ok(dir.join(format!("meridian-diagnostics-{micros}.tar.gz")))
}
