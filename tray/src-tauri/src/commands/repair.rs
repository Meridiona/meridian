//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! The user-facing half of database repair: ask for one, and find out whether
//! one is needed.
//!
//! Before this existed, a corrupt `meridian.db` left the user a banner telling
//! them to quit the app and run `meridian db repair` in a terminal. On a signed
//! DMG shipped to people who did not install a CLI, that is a dead end dressed
//! up as instructions - most users would simply watch tracking stop forever.
//!
//! # How a repair actually happens
//!
//! This command does **not** repair anything. It writes the marker and
//! relaunches; [`crate::repair_boot`] does the work on the way back up, at the
//! one moment when nothing holds the database open. That module's header
//! explains why an in-session repair is not workable.
//!
//! # Who calls this
//!
//! The dashboard, when the user confirms the repair offered against a live
//! `db.corrupt` notice.
//!
//! # Related
//!
//! - [`crate::repair_boot`] - performs the repair on the next launch.
//! - [`meridian::db::repair::marker`] - the daemon handshake.
//! - [`meridian::db::integrity`] - what decides a database is damaged.

use crate::install;

/// What `db check` would say, without repairing. Drives the confirmation copy
/// so the user is told what a repair will cost *before* agreeing to it, rather
/// than discovering it afterwards.
#[derive(serde::Serialize)]
pub struct RepairPreview {
    /// Whether anything is actually damaged.
    pub damaged: bool,
    /// Tables that cannot be read end to end.
    pub corrupt_tables: Vec<String>,
    /// Of those, the ones holding real user data rather than capture scratch -
    /// the distinction that decides whether this is a shrug or a loss.
    pub product_tables: Vec<String>,
}

/// Scans the database and reports what a repair would face.
///
/// Read-only, and safe to call while everything is running: it opens its own
/// read-only pool (see `repair::inspect`) rather than borrowing the tray's.
#[tauri::command]
#[tracing::instrument]
pub async fn preview_repair() -> Result<RepairPreview, String> {
    let db_path = install::meridian_db_path();
    let key = install::detect_install_mode()
        .env_path()
        .and_then(|p| install::env_key_from_path(p, "MERIDIAN_DB_KEY"));

    let (_problems, scan) =
        meridian::db::repair::inspect(std::path::Path::new(&db_path), key.as_deref())
            .await
            .map_err(|e| {
                tracing::warn!(error = %e, "repair preview failed");
                format!("{e:#}")
            })?;

    Ok(RepairPreview {
        damaged: !scan.is_healthy(),
        product_tables: scan
            .corrupt_product_tables()
            .into_iter()
            .map(str::to_string)
            .collect(),
        corrupt_tables: scan.corrupt,
    })
}

/// Requests a repair: marks the database, asks the daemon to stop, and
/// relaunches the app into [`crate::repair_boot`].
///
/// The app WILL restart - that is not a side effect to be surprised by, it is
/// the mechanism, and the caller should have said so before invoking this.
///
/// Ordering is load-bearing. The marker goes down **first** so that a daemon
/// relaunching under launchd's `KeepAlive` between here and the restart finds
/// it and stands down. Writing it after the stop would leave a window where a
/// respawned daemon opens the database we are about to rebuild.
#[tauri::command]
#[tracing::instrument(skip(app))]
pub async fn request_repair(app: tauri::AppHandle) -> Result<(), String> {
    let db_path = install::meridian_db_path();
    let db_path = std::path::Path::new(&db_path);

    meridian::db::repair::marker::request(db_path).map_err(|e| {
        tracing::error!(error = %e, "could not write the repair marker");
        format!("{e:#}")
    })?;

    // Best-effort. `launchctl stop` does not hold a KeepAlive job down (see
    // the marker module), so this only shortens the wait - the marker is what
    // actually keeps the daemon out, and `repair_boot` waits for it regardless.
    if let Err(e) = super::daemon_control::set_running(false).await {
        tracing::warn!(error = %e, "could not stop the daemon before repairing - the marker will hold it off instead");
    }

    tracing::info!("restarting to repair the database");
    app.restart();
}
