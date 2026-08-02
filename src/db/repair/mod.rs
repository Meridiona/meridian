//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Recovery for a corrupt `meridian.db`: rebuild it into a fresh file, keeping
//! every row that can still be read.
//!
//! # Why there is no repair-in-place
//!
//! The obvious repairs do not work, and this was established by running them
//! against a real corrupt database rather than reasoned about:
//!
//! ```text
//! REINDEX idx_capture_ui_events_timestamp;  -> database disk image is malformed
//! DROP TABLE capture_frames;                -> database disk image is malformed
//! ```
//!
//! Both have to walk the damaged b-tree to do their job - `REINDEX` reads the
//! table to rebuild the index, `DROP TABLE` walks the tree to free its pages -
//! so both fail on exactly the databases that need them. `VACUUM` is worse: on
//! a malformed file it can fail part-way with the original already truncated.
//!
//! That leaves one primitive that does work: **read what is still readable
//! into a brand-new file and swap it in**. Everything below is that.
//!
//! # The three-tier copy
//!
//! Per table, cheapest first, falling back on `SQLITE_CORRUPT` **or**
//! `SQLITE_CONSTRAINT`:
//!
//! 1. One `INSERT … SELECT` for the whole table, over an explicit
//!    intersected column list (never `SELECT *` - see [`copy_columns`]).
//! 2. **Row-by-row salvage.** Tier 1 is all-or-nothing, and that difference is
//!    not marginal: on the motivating database `pm_worklog_hours` lost all 746
//!    rows to tier 1 and recovered **745** of them row by row. Only one row was
//!    genuinely unreadable.
//! 3. **Left empty**, for [`integrity::DISPOSABLE_TABLES`] only - capture
//!    scratch the ETL has largely consumed already, where row-by-row over
//!    millions of rows would cost hours to salvage data that retention deletes
//!    on a 30-day clock anyway.
//!
//! # Four things that are silently destructive if omitted
//!
//! Every one of these was found by running this against the real corrupt
//! database, not by review, and each failed in a way that looked like success
//! or like someone else's fault.
//!
//! - **`sqlite_sequence` must be carried across.** `capture_frames` is
//!   `INTEGER PRIMARY KEY AUTOINCREMENT` precisely so ids are never reused
//!   (migration 046 says so, and explains that a reused id makes the ETL
//!   cursor skip or reprocess frames). A rebuilt file that starts its
//!   sequence at 1 while `etl_cursor.last_frame_id` still reads 339257 means
//!   `get_frames_since` matches nothing, forever, with no error anywhere -
//!   capture appears to work and the timeline silently stays empty.
//! - **Foreign keys must be OFF during the copy.** sqlx enables them by
//!   default and tables are copied in name order, so a child is inserted
//!   before its parent exists: `pm_worklog_feedback` lost all 20 rows this way,
//!   counted as harmless "schema drift". A `foreign_key_check` after the copy
//!   re-verifies what enforcement would have.
//! - **Migrations seed rows, so the replacement is not empty.**
//!   `etl_cursor` and `activity_context` arrive pre-populated; copying the
//!   source on top collides on the primary key. The seeds are cleared first,
//!   but only where the source has rows to replace them with.
//! - **Constraint rejections are not corruption.** A row the current schema
//!   refuses read off disk perfectly well. Reporting it as corruption loss
//!   would overstate the damage, so [`TableOutcome`] counts `rows_rejected`
//!   separately from `rows_unreadable`. On the real database that split was
//!   1 row genuinely lost versus 39 that had NULLs in `NOT NULL` columns and
//!   could never have been stored.
//!
//! # Who calls this
//!
//! `meridian db repair` (`src/main.rs`). Never run automatically: the daemon
//! cannot do this to a live file, and cannot ask the tray to stop writing (see
//! [`ensure_no_writers`]).
//!
//! # Related
//!
//! - [`crate::db::integrity`] - detection, and the scan that decides which
//!   tables need which tier.
//! - [`meridian_core::db_crypto::swap_database_file`] - the atomic
//!   two-rename-with-rollback swap, shared with the encryption migration.

use anyhow::{bail, Context, Result};
use sqlx::SqlitePool;
use std::path::{Path, PathBuf};

use super::integrity::{self, is_corrupt_error};

/// What happened to one table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableOutcome {
    pub table: String,
    /// Rows written into the replacement.
    pub rows_copied: u64,
    /// Rows that could not be read at all - genuine, unrecoverable loss.
    pub rows_unreadable: u64,
    /// Rows that read fine but the current schema refused (a `NOT NULL` added
    /// by a later migration, say). Not corruption - see the module header.
    pub rows_rejected: u64,
    /// True when tier 1 failed and the row-by-row fallback ran.
    pub salvaged_row_by_row: bool,
    /// True when the table was deliberately left empty (disposable scratch).
    pub left_empty: bool,
}

/// Outcome of a whole repair.
#[derive(Debug, Clone, Default)]
pub struct RepairReport {
    pub tables: Vec<TableOutcome>,
    /// Where the damaged original was moved to. Never deleted.
    pub original_backup: PathBuf,
    pub before_bytes: u64,
    pub after_bytes: u64,
}

impl RepairReport {
    /// Total rows carried into the replacement.
    pub fn rows_copied(&self) -> u64 {
        self.tables.iter().map(|t| t.rows_copied).sum()
    }

    /// Rows lost to corruption. The number that matters to the user.
    pub fn rows_unreadable(&self) -> u64 {
        self.tables.iter().map(|t| t.rows_unreadable).sum()
    }

    /// Rows the current schema refused. Reported separately so it is never
    /// mistaken for corruption loss.
    pub fn rows_rejected(&self) -> u64 {
        self.tables.iter().map(|t| t.rows_rejected).sum()
    }

    /// Tables that lost at least one row, worst first - what to show a user.
    pub fn lossy_tables(&self) -> Vec<&TableOutcome> {
        let mut v: Vec<&TableOutcome> = self
            .tables
            .iter()
            .filter(|t| t.rows_unreadable > 0 || t.left_empty)
            .collect();
        v.sort_by(|a, b| b.rows_unreadable.cmp(&a.rows_unreadable));
        v
    }
}

/// Refuses to proceed while anything might still be writing the database.
///
/// Two writers touch `meridian.db` (see `CLAUDE.md`'s "Capture source"): the
/// daemon owns most tables, the tray owns the `capture_*` ones. Rebuilding
/// underneath either means copying a moving target and then swapping the file
/// out from under an open handle. The daemon has no way to *ask* the tray to
/// stop - `daemon.sock` runs the other direction - so this is a hard refusal
/// with instructions, not something to coordinate around.
pub async fn ensure_no_writers() -> Result<()> {
    if crate::platform::daemon_already_running().await {
        bail!(
            "the Meridian daemon is still running - {}, then run this again",
            stop_daemon_hint()
        );
    }
    if tray_is_running() {
        bail!(
            "the Meridian tray app is still running - quit it from the menu bar, \
             then run this again"
        );
    }
    Ok(())
}

/// How to actually stop the daemon, in words that name something real.
///
/// The first version of this message said "stop it first ('meridian stop')".
/// There is no `stop` subcommand - the binary answers `unknown subcommand
/// "stop"` - so the one instruction the user got was a dead end. That is the
/// exact failure this whole feature exists to remove, reintroduced in its own
/// error path.
///
/// Two things make the real answer non-obvious, which is why it is spelled out
/// rather than left to the reader:
/// - **Quitting the tray does not stop the daemon.** They are separate
///   processes; the daemon is its own launchd agent (`com.meridiona.daemon`) /
///   scheduled task.
/// - **Killing the process does not stop it either.** The launchd plist sets
///   `KeepAlive=true`, so it is relaunched within seconds - and a daemon that
///   respawns mid-rebuild writes to the file being copied.
///
/// Pausing from the tray menu is offered first because it is the supported
/// control (`tray/src-tauri/src/commands/daemon.rs`'s `toggle_daemon` ->
/// `daemon_control::set_running`) and it is the same on every platform.
fn stop_daemon_hint() -> &'static str {
    #[cfg(target_os = "macos")]
    {
        "pause tracking from the Meridian tray menu, or run: \
         launchctl bootout gui/$(id -u)/com.meridiona.daemon"
    }
    #[cfg(target_os = "windows")]
    {
        "pause tracking from the Meridian tray menu, or disable the Meridian \
         scheduled task"
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        "pause tracking from the Meridian tray menu"
    }
}

/// True when a `meridian-tray` process is alive.
///
/// Deliberately a process probe rather than anything the tray cooperates with:
/// a tray that is wedged (the failure mode that motivated this whole feature
/// was a tray crash-loop) would not answer a handshake, and "it did not reply"
/// must not read as "it is safe to proceed".
#[cfg(unix)]
fn tray_is_running() -> bool {
    std::process::Command::new("pgrep")
        .args(["-x", "meridian-tray"])
        .output()
        .map(|o| o.status.success() && !o.stdout.is_empty())
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn tray_is_running() -> bool {
    std::process::Command::new("tasklist")
        .args(["/FI", "IMAGENAME eq meridian-tray.exe", "/NH"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).contains("meridian-tray.exe"))
        .unwrap_or(false)
}

/// Rebuilds the database at `db_path` into a fresh file and swaps it in.
///
/// `key_hex` is the SQLCipher key, or `None` for an unencrypted database - the
/// replacement is written with the same key, so an encrypted install stays
/// encrypted.
///
/// The original is never modified. On success it is moved aside to
/// `meridian.db.corrupt-backup-<ts>` and left there deliberately: this
/// operation loses rows by design, and the only copy of what was lost is that
/// file. Deleting it is the user's call.
///
/// # Callers must call [`ensure_no_writers`] first
///
/// That guard deliberately lives in the CLI layer rather than here. It probes
/// for *any* running daemon or tray on the machine, not for writers of this
/// particular file - so calling it from inside this function would make the
/// whole repair path impossible to test on a developer machine with the tray
/// running, which is every developer machine. Keeping the operation pure and
/// the precondition at the boundary is what lets `repair_end_to_end` exercise
/// the real thing rather than a stripped-down copy of it.
#[tracing::instrument(skip(key_hex), fields(rows_copied = tracing::field::Empty, rows_lost = tracing::field::Empty))]
pub async fn repair(db_path: &Path, key_hex: Option<&str>) -> Result<RepairReport> {
    if !db_path.exists() {
        bail!("no database at {}", db_path.display());
    }

    let before_bytes = std::fs::metadata(db_path).map(|m| m.len()).unwrap_or(0);
    let tmp_path = db_path.with_extension("db.rebuilding-tmp");
    // A previous interrupted attempt leaves this behind; it is a partial
    // rebuild with no value of its own, unlike the timestamped backup.
    if tmp_path.exists() {
        tracing::warn!("removing a partial rebuild left by an interrupted repair");
        std::fs::remove_file(&tmp_path).context("removing stale rebuild file")?;
    }

    let report = build_replacement(db_path, &tmp_path, key_hex).await;
    let mut report = match report {
        Ok(r) => r,
        Err(e) => {
            // Nothing has been swapped yet, so the original is untouched.
            let _ = std::fs::remove_file(&tmp_path);
            return Err(e).context("rebuild failed - the original database is unchanged");
        }
    };

    let backup_path = db_path.with_extension(format!(
        "db.corrupt-backup-{}",
        chrono::Utc::now().format("%Y%m%d%H%M%S")
    ));
    meridian_core::db_crypto::swap_database_file(db_path, &tmp_path, &backup_path, "db repair")?;

    report.original_backup = backup_path;
    report.before_bytes = before_bytes;
    report.after_bytes = std::fs::metadata(db_path).map(|m| m.len()).unwrap_or(0);

    tracing::Span::current().record("rows_copied", report.rows_copied());
    tracing::Span::current().record("rows_lost", report.rows_unreadable());
    tracing::info!(
        rows_copied = report.rows_copied(),
        rows_unreadable = report.rows_unreadable(),
        rows_rejected = report.rows_rejected(),
        tables = report.tables.len(),
        "database repair complete"
    );
    Ok(report)
}

/// Convenience wrapper: repair the database a pool would open, given its path.
/// Reads the key from `MERIDIAN_DB_KEY`, matching `db::meridian::setup_db`.
pub async fn repair_default(db_path: &Path) -> Result<RepairReport> {
    let key = std::env::var("MERIDIAN_DB_KEY").ok();
    repair(db_path, key.as_deref()).await
}

/// Runs [`integrity::scan_tables`] against `db_path` without repairing.
/// Backs `meridian db check`.
pub async fn inspect(
    db_path: &Path,
    key_hex: Option<&str>,
) -> Result<(Vec<String>, integrity::Scan)> {
    let uri = format!("sqlite://{}", db_path.display());
    // Read-only on purpose: unlike `repair`, `check` has no reason to demand
    // that everything be stopped, so it routinely runs while the daemon still
    // owns the file. A diagnostic must not take a write lock or run the WAL
    // setup pragmas against a database another process is mid-write on.
    let pool: SqlitePool = meridian_core::db_crypto::open_pool_with_key_readonly(&uri, key_hex)
        .await
        .context("opening the database to inspect it")?;
    let problems = integrity::integrity_check(&pool, 50)
        .await
        .unwrap_or_else(|e| {
            // integrity_check itself can fail on a badly damaged header; that is a
            // finding, not a reason to abort the whole inspection.
            vec![format!("integrity_check could not complete: {e}")]
        });
    let scan = integrity::scan_tables(&pool).await;
    pool.close().await;
    match scan {
        Ok(scan) => Ok((problems, scan)),
        Err(e) if is_corrupt_error(&e) => Ok((problems, integrity::Scan::default())),
        Err(e) => Err(e),
    }
}

// Split three ways so each file stays under the repo's 500-line ceiling.
// `rebuild` builds the replacement file; `copy` moves one table into it.
mod copy;
pub mod marker;
mod rebuild;

use rebuild::build_replacement;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_corrupt::corrupt_db_fixture;

    /// The remedy must not invent a CLI. The first version told users to run
    /// `meridian stop`, which does not exist - `main.rs` dispatches only
    /// `coding-agent-hook`, `coding-agent-summarise`, `oauth-login` and `db`,
    /// and the binary answers `unknown subcommand "stop"`.
    ///
    /// A guard that hands out a dead end is worse than one that says nothing,
    /// because the user burns time believing they were told the answer. Naming
    /// no `meridian <verb>` at all is the cheap invariant that holds even as
    /// the subcommand list changes.
    #[test]
    fn stop_hint_does_not_name_a_nonexistent_subcommand() {
        let hint = stop_daemon_hint();
        assert!(
            !hint.contains("meridian stop"),
            "the `meridian stop` subcommand does not exist: {hint}"
        );
        assert!(
            !regex_lite_names_meridian_subcommand(hint),
            "hint names a `meridian <verb>` that may not exist: {hint}"
        );
        // ...and it must still tell the user something concrete to do.
        assert!(
            hint.contains("pause tracking"),
            "hint must name the supported control: {hint}"
        );
    }

    /// True if `s` contains `meridian ` followed by a bare word - i.e. it looks
    /// like a subcommand invocation. Deliberately hand-rolled: the crate has no
    /// regex dependency and this only needs to catch the one shape.
    fn regex_lite_names_meridian_subcommand(s: &str) -> bool {
        s.split("meridian ")
            .skip(1)
            .any(|rest| rest.chars().next().is_some_and(|c| c.is_ascii_alphabetic()))
    }

    #[test]
    fn report_separates_corruption_loss_from_schema_rejection() {
        let report = RepairReport {
            tables: vec![
                TableOutcome {
                    table: "pm_worklog_hours".into(),
                    rows_copied: 745,
                    rows_unreadable: 1,
                    rows_rejected: 0,
                    salvaged_row_by_row: true,
                    left_empty: false,
                },
                TableOutcome {
                    table: "day_task_worklogs".into(),
                    rows_copied: 80,
                    rows_unreadable: 0,
                    rows_rejected: 6,
                    salvaged_row_by_row: true,
                    left_empty: false,
                },
                TableOutcome {
                    table: "capture_frames".into(),
                    rows_copied: 0,
                    rows_unreadable: 0,
                    rows_rejected: 0,
                    salvaged_row_by_row: false,
                    left_empty: true,
                },
            ],
            ..Default::default()
        };
        assert_eq!(report.rows_copied(), 825);
        assert_eq!(report.rows_unreadable(), 1, "only genuine loss counts here");
        assert_eq!(report.rows_rejected(), 6);
        // capture_frames (emptied) and pm_worklog_hours (1 row lost) are the
        // lossy ones; the schema-rejected table is not corruption loss.
        let lossy: Vec<&str> = report
            .lossy_tables()
            .iter()
            .map(|t| t.table.as_str())
            .collect();
        assert_eq!(lossy, vec!["pm_worklog_hours", "capture_frames"]);
    }

    /// The whole operation against a genuinely byte-corrupt file: the
    /// replacement must end up where the original was, be readable end to end,
    /// and the damaged original must still exist afterwards.
    ///
    /// Exercises `repair()` itself rather than its parts, which is only
    /// possible because `ensure_no_writers` lives in the CLI layer - see that
    /// function's doc.
    #[tokio::test]
    async fn repair_end_to_end_replaces_the_file_and_keeps_the_original() {
        let fx = corrupt_db_fixture().await;
        // Release our own handle: repair swaps the file underneath.
        fx.pool.close().await;
        let path = fx.path.clone();
        let before = std::fs::metadata(&path).expect("stat").len();

        // The fixture's schema is not meridian's, so the migration-built
        // replacement shares no tables with it - `table_names` intersects, so
        // this exercises the full flow and simply copies nothing.
        let report = repair(&path, None).await.expect("repair must succeed");

        assert!(path.exists(), "the database must be back in place");
        assert!(
            report.original_backup.exists(),
            "the damaged original must be kept, not deleted"
        );
        assert_eq!(
            std::fs::metadata(&report.original_backup)
                .expect("stat backup")
                .len(),
            before,
            "the kept original must be the untouched damaged file"
        );
        assert!(
            !path.with_extension("db.rebuilding-tmp").exists(),
            "the scratch rebuild file must not be left behind"
        );

        // The replacement must be sound - that is the entire point.
        let uri = format!("sqlite://{}", path.display());
        let pool = meridian_core::db_crypto::open_pool_with_key(&uri, None, false, &[])
            .await
            .expect("reopen repaired db");
        let problems = integrity::quick_check(&pool, 20)
            .await
            .expect("quick_check");
        assert!(
            problems.is_empty(),
            "repaired db still reports {problems:?}"
        );
        let scan = integrity::scan_tables(&pool).await.expect("scan");
        assert!(
            scan.is_healthy(),
            "repaired db still has corrupt tables: {scan:?}"
        );
        pool.close().await;
    }

    /// A repair that cannot even build a replacement must leave the original
    /// exactly where it was - the "never lose data" invariant.
    #[tokio::test]
    async fn failed_repair_leaves_the_original_untouched() {
        let dir = tempfile::tempdir().expect("tempdir");
        let missing = dir.path().join("nope.db");
        assert!(repair(&missing, None).await.is_err());
        assert!(
            !missing.exists(),
            "must not create a database it was asked to repair"
        );
    }
}
