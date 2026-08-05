//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Runs a database repair during tray startup, before anything opens
//! `meridian.db` - either because the user requested one last session, or
//! because the startup probe found the file damaged.
//!
//! # Why this happens at boot rather than on the button press
//!
//! Repair needs both writers quiet. The daemon stands down on its own once the
//! marker exists (`meridian::db::repair::marker`), but the tray cannot quiesce
//! *itself* mid-session in any way worth trusting: it holds a live sqlx pool
//! that commands are using, the capture engine is streaming frames into it, and
//! the rebuilt file replaces the one every existing connection is bound to. A
//! pool whose connections point at a swapped-out inode keeps happily reading
//! the file that was moved aside - silently serving stale data forever.
//!
//! Restarting sidesteps all of it. [`crate::commands::repair`] writes the
//! marker and relaunches; on the way back up this runs at the one moment when
//! the tray provably has no pool and no capture engine, and the daemon is
//! standing down. Nothing needs to be torn down because nothing has been built
//! yet.
//!
//! `update.rs`'s forced-update path already relaunches the tray this way, so
//! the restart itself is a proven move rather than a new one.
//!
//! # Why the tray also probes and repairs on its own ([`run_auto_if_needed`])
//!
//! The button depends on a chain the damage itself breaks. A corrupt database
//! raises the `db.corrupt` notice - a row in that same database. On the
//! 2026-08 fleet incident, machines whose page 1 was damaged could not open the
//! database at all, so the daemon could not raise the notice, the dashboard
//! could not read one, and the user saw a silently dead app with no repair
//! offer anywhere. Those machines stayed hard-down across an entire release
//! that fixed the corruption's cause, because the fix could not heal existing
//! damage and nothing on the machine could ask for a repair.
//!
//! So startup now probes the file directly - the one signal path corruption
//! cannot sever - and repairs unattended. Repair is safe to run without
//! consent: the original is never modified, only moved aside with every
//! damaged byte intact (`meridian::db::repair`'s contract), so the user risks
//! nothing they had not already lost.
//!
//! # Who calls this
//!
//! `lib.rs`'s `setup`, immediately after the SQLCipher key is resolved and
//! immediately before `open_existing_lazy` builds the pool. That ordering is
//! the whole point - see above. [`run_if_requested`] (the marker path) runs
//! first; [`run_auto_if_needed`] (the probe path) only when no marker was
//! pending.
//!
//! # Related
//!
//! - [`meridian::db::repair::marker`] - the handshake that holds the daemon off.
//! - [`meridian::db::repair::repair`] - the salvage this drives.
//! - [`meridian::db::integrity`] - the probe's classification helpers.
//! - [`crate::commands::repair`] - writes the marker and triggers the relaunch.

use std::path::{Path, PathBuf};

/// How long to wait for a mid-flight daemon to notice the marker and exit.
///
/// The marker stops the daemon *starting*, but one already running when the
/// button was pressed keeps going until it next exits. `set_running(false)`
/// asks it to stop; this is the grace period for that to land before the
/// repair either proceeds or gives up.
const DAEMON_QUIESCE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

/// Ceiling on the startup probe's `quick_check`. The check reads every page,
/// so a very large database could otherwise hold the whole `setup` closure -
/// and with it the tray icon - hostage. On timeout the probe reports healthy
/// (fail open): a database too large to check in this budget is being written
/// by a live daemon that would have latched `db.corrupt` itself if it were
/// damaged.
const PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// How long an unopenable database's replacement is trusted before another
/// unopenable verdict is treated as "something is eating databases" rather
/// than a fresh incident. Within this window [`run_auto_if_needed`] refuses a
/// second fresh start, so a machine with failing hardware accumulates one
/// set-aside file and a loud error, not a new empty database per launch.
const FRESH_START_GUARD: std::time::Duration = std::time::Duration::from_secs(24 * 60 * 60);

/// Outcome, kept plain so `lib.rs` can turn it into a notice once it has a pool
/// (this runs before one exists).
pub enum Outcome {
    /// Rebuilt. Carries a user-facing summary.
    Repaired { summary: String },
    /// Attempted and failed. The original database is untouched.
    Failed { error: String },
    /// The file could not be opened at all, so nothing in it was salvageable
    /// by any tool. It was moved aside (path carried here, for the notice) and
    /// a fresh database starts in its place.
    FreshStart { backup: String },
}

/// What the startup probe concluded about the on-disk `meridian.db`.
#[derive(Debug)]
enum Probe {
    /// Opens and passes `quick_check` - or is absent, or the probe could not
    /// tell (inconclusive errors and timeouts deliberately land here; see
    /// [`probe_database`]).
    Healthy,
    /// Opens, but `quick_check` reports structural damage. Repairable in
    /// place - page 1 is readable, so the salvage can ATTACH it.
    Damaged { problems: Vec<String> },
    /// The keyed open (or first read) fails with SQLITE_NOTADB (code 26) -
    /// page 1 itself is unreadable. Nothing can read this file, under any
    /// tool; a plain-corrupt failure (code 11) classifies [`Probe::Damaged`]
    /// instead, because a file that ATTACHes is still salvageable.
    Unopenable { error: String },
}

/// Repairs `meridian.db` if a repair was requested, then clears the marker.
///
/// `None` when no repair is pending, which is every normal launch - this must
/// be as close to free as possible on the hot path, and it is: one `stat`.
pub fn run_if_requested(db_path: &Path, key_hex: Option<&str>) -> Option<Outcome> {
    if !meridian::db::repair::marker::pending(db_path) {
        return None;
    }
    tracing::info!("a database repair was requested - running it before opening the database");
    Some(execute_pending_repair(db_path, key_hex))
}

/// Probes the database and, when it is damaged, repairs (or sets aside) it
/// unattended - the path that heals a machine whose user was never shown a
/// repair button. See the module header for why this exists.
///
/// `None` when the database is healthy or the probe could not tell; the caller
/// proceeds to a normal open either way. Runs only when no marker was pending,
/// so a user-requested repair is never doubled up.
///
/// # Gated on the daemon being unreachable - for cost and for safety
///
/// The probe only runs when no daemon answers, which serves two ends at once:
///
/// - **Cost.** The hot path stays near-free: one daemon probe, no file scan.
///   `quick_check` reads every page - seconds on a multi-GB database - and
///   paying that inside the sync `setup` closure on every healthy boot would
///   tax exactly the machines that need no healing. The hard-down profile this
///   path exists for is precisely "the daemon cannot start"; the latched
///   profile (daemon up, `db.corrupt` raised) still heals through the
///   existing banner + repair button, which works there because the database
///   is readable. A healthy boot where the daemon merely has not started yet
///   pays one wasted scan of a healthy file - rare, and bounded by
///   [`PROBE_TIMEOUT`].
/// - **Safety.** A live daemon is proof that *someone* can open the file, so
///   an Unopenable verdict from the tray's own key would mean the tray is the
///   one holding a wrong key (a keychain restored out of step with `.env`),
///   not that the file is corrupt. Renaming it out from under a healthy
///   writing daemon would silently strand its writes on the backup inode -
///   the one way this feature could destroy data. With the gate, an
///   unopenable verdict is only ever acted on when nothing can open the file.
pub fn run_auto_if_needed(db_path: &Path, key_hex: Option<&str>) -> Option<Outcome> {
    if tauri::async_runtime::block_on(meridian::platform::daemon_already_running()) {
        tracing::debug!(
            "daemon is up - skipping the startup integrity probe (a corrupt-but-readable database heals through the notice banner instead)"
        );
        return None;
    }
    probe_and_heal(db_path, key_hex)
}

/// The probe + heal body behind [`run_auto_if_needed`]'s daemon gate. Split
/// out so tests can exercise it without the gate consulting the development
/// machine's real daemon.
fn probe_and_heal(db_path: &Path, key_hex: Option<&str>) -> Option<Outcome> {
    match tauri::async_runtime::block_on(probe_database(db_path, key_hex)) {
        Probe::Healthy => None,
        Probe::Damaged { problems } => {
            tracing::warn!(
                problems = ?problems,
                "meridian.db failed its startup integrity probe - repairing it automatically before opening"
            );
            // The marker goes down FIRST, exactly as `commands::repair` orders
            // it: the gate proved no daemon is answering, but one relaunching
            // under KeepAlive (or leaving the watchdog's give-up window)
            // between here and the repair must find the marker and stand
            // down. If even the marker cannot be written, repairing would
            // race such a relaunch - skip, and let the daemon's own latch
            // report the corruption.
            if let Err(e) = meridian::db::repair::marker::request(db_path) {
                tracing::error!(error = %e, "could not write the repair marker - skipping the automatic repair");
                return None;
            }
            Some(execute_pending_repair(db_path, key_hex))
        }
        Probe::Unopenable { error } => {
            if key_hex.is_none() {
                // Without a keychain-verified key, "file is not a database"
                // is exactly what a healthy encrypted file looks like. This is
                // a key-resolution problem, not corruption - moving the file
                // aside here would discard data a recovered key could still
                // open.
                tracing::error!(
                    error = %error,
                    "meridian.db cannot be opened and no encryption key was resolved - leaving the file alone (a key problem, not corruption)"
                );
                return None;
            }
            if let Some(previous) = recent_unopenable_backup(db_path) {
                tracing::error!(
                    error = %error,
                    previous_backup = %previous.display(),
                    "meridian.db is unopenable again within a day of being replaced - refusing a second fresh start; something is damaging databases faster than they can be replaced"
                );
                return Some(Outcome::Failed { error });
            }
            tracing::error!(
                error = %error,
                "meridian.db cannot be opened under its own key - nothing in it is salvageable, moving it aside and starting fresh"
            );
            // Same handshake as the Damaged path, for the same reason: the
            // gate proved no daemon is answering NOW, and the marker keeps a
            // KeepAlive relaunch out of the window while the file moves.
            // (No `set_running` call - there is provably nothing to stop.)
            if let Err(e) = meridian::db::repair::marker::request(db_path) {
                tracing::error!(error = %e, "could not write the repair marker - leaving the unopenable database in place");
                return Some(Outcome::Failed { error });
            }
            let result = set_aside_unopenable(db_path);
            if let Err(e) = meridian::db::repair::marker::clear(db_path) {
                tracing::error!(error = %e, "could not clear the repair marker - the daemon will stay down until it expires");
            }
            match result {
                Ok(backup) => Some(Outcome::FreshStart {
                    backup: backup.display().to_string(),
                }),
                Err(e) => {
                    tracing::error!(error = %e, "could not move the unopenable database aside - leaving it in place");
                    Some(Outcome::Failed {
                        error: format!("{e:#}"),
                    })
                }
            }
        }
    }
}

/// The shared repair body: quiesce the daemon, salvage, clear the marker, map
/// to an [`Outcome`]. Callers have already put the marker down - the button
/// path last session, the auto path just above.
///
/// The marker is cleared on **both** outcomes. Leaving it after a failure would
/// hold the daemon down forever over an error the user can already see, and the
/// database is untouched on failure anyway, so there is nothing to protect.
fn execute_pending_repair(db_path: &Path, key_hex: Option<&str>) -> Outcome {
    let outcome = tauri::async_runtime::block_on(async move {
        wait_for_daemon_to_stand_down().await;
        meridian::db::repair::repair(db_path, key_hex).await
    });

    // Clear before reporting: a panic while formatting the summary must not
    // leave the marker behind and the daemon permanently down.
    if let Err(e) = meridian::db::repair::marker::clear(db_path) {
        tracing::error!(error = %e, "could not clear the repair marker - the daemon will stay down until it expires");
    }

    match outcome {
        Ok(report) => {
            tracing::info!(
                rows_copied = report.rows_copied(),
                rows_unreadable = report.rows_unreadable(),
                rows_rejected = report.rows_rejected(),
                "database repair finished"
            );
            Outcome::Repaired {
                summary: summarise(&report),
            }
        }
        Err(e) => {
            tracing::error!(error = %e, "database repair failed - the original is unchanged");
            Outcome::Failed {
                error: format!("{e:#}"),
            }
        }
    }
}

/// Classifies the on-disk database. Read-only and safe against a live daemon:
/// the open takes no write lock and `quick_check` reads a WAL snapshot.
///
/// Every inconclusive path deliberately reports [`Probe::Healthy`]: repairing
/// on uncertainty would stop the daemon and rebuild a database over an error
/// that may have been a transient lock. Only the two positively-identified
/// corruption signatures trigger action.
async fn probe_database(db_path: &Path, key_hex: Option<&str>) -> Probe {
    if !db_path.exists() {
        return Probe::Healthy; // fresh install - the daemon will create it
    }
    let uri = format!("sqlite://{}", db_path.display());
    let pool = match meridian_core::db_crypto::open_pool_with_key_readonly(&uri, key_hex).await {
        Ok(pool) => pool,
        // Same classification order as the query-error arms below, for the
        // same reason: `is_unopenable_error` includes the plain-corrupt
        // family, and code 11 means the file still ATTACHes - repairable, so
        // it must never route to the set-aside path. (Today the open is lazy
        // and errors surface at quick_check instead, but this arm must stay
        // correct if that ever changes.)
        Err(e) if meridian::db::integrity::is_corrupt_error(&e) => {
            return Probe::Damaged {
                problems: vec![format!("{e:#}")],
            };
        }
        Err(e) if meridian::db::integrity::is_unopenable_error(&e) => {
            return Probe::Unopenable {
                error: format!("{e:#}"),
            };
        }
        Err(e) => {
            tracing::warn!(error = %e, "startup probe could not open meridian.db for a reason that is not corruption - proceeding normally");
            return Probe::Healthy;
        }
    };
    let checked = tokio::time::timeout(
        PROBE_TIMEOUT,
        meridian::db::integrity::quick_check(&pool, 5),
    )
    .await;
    pool.close().await;
    match checked {
        Err(_elapsed) => {
            tracing::warn!(
                timeout_secs = PROBE_TIMEOUT.as_secs(),
                "startup integrity probe timed out - proceeding normally rather than blocking the tray"
            );
            Probe::Healthy
        }
        Ok(Ok(problems)) if problems.is_empty() => Probe::Healthy,
        Ok(Ok(problems)) => Probe::Damaged { problems },
        Ok(Err(e)) if meridian::db::integrity::is_corrupt_error(&e) => Probe::Damaged {
            problems: vec![format!("{e:#}")],
        },
        // The pool connects lazily, so a file whose page 1 is unreadable
        // surfaces here - at the first real query - rather than at open time.
        // `is_unopenable_error` also covers the corrupt family, so this arm
        // must come after the plain-corrupt one: code 11 means the file
        // ATTACHes and is repairable, code 26 means nothing can read it.
        Ok(Err(e)) if meridian::db::integrity::is_unopenable_error(&e) => Probe::Unopenable {
            error: format!("{e:#}"),
        },
        Ok(Err(e)) => {
            tracing::warn!(error = %e, "startup integrity probe errored inconclusively - proceeding normally");
            Probe::Healthy
        }
    }
}

/// Moves an unopenable database (and its `-wal`/`-shm` sidecars) aside under a
/// timestamped name, so a fresh one can be created at the canonical path. The
/// file is preserved, never deleted - it is the only copy of whatever a future
/// offline-salvage attempt might still extract.
fn set_aside_unopenable(db_path: &Path) -> anyhow::Result<PathBuf> {
    use anyhow::Context;
    let backup = db_path.with_extension(format!(
        "db.unopenable-backup-{}",
        chrono::Utc::now().format("%Y%m%d%H%M%S")
    ));
    std::fs::rename(db_path, &backup).with_context(|| {
        format!(
            "moving the unopenable database aside to {}",
            backup.display()
        )
    })?;
    // Stale sidecars must follow the main file: a leftover -wal beside a fresh
    // database would be offered to SQLite as that database's log. If the
    // rename fails, deleting is the right fallback - the main file already
    // moved, so the sidecar's salvage value is tied to the backup either way,
    // and leaving it in place realises the exact hazard above.
    for suffix in ["-wal", "-shm"] {
        let sidecar = PathBuf::from(format!("{}{}", db_path.display(), suffix));
        if !sidecar.exists() {
            continue;
        }
        let kept = PathBuf::from(format!("{}{}", backup.display(), suffix));
        if let Err(rename_err) = std::fs::rename(&sidecar, &kept) {
            match std::fs::remove_file(&sidecar) {
                Ok(()) => tracing::warn!(
                    error = %rename_err,
                    sidecar = %sidecar.display(),
                    "could not move a stale sidecar beside the backup - deleted it instead so it cannot shadow the fresh database"
                ),
                Err(remove_err) => tracing::warn!(
                    rename_error = %rename_err,
                    remove_error = %remove_err,
                    sidecar = %sidecar.display(),
                    "could not move or delete a stale sidecar - it may shadow the fresh database"
                ),
            }
        }
    }
    Ok(backup)
}

/// Any `unopenable-backup` set aside within [`FRESH_START_GUARD`] (first hit
/// in directory order - which one does not matter, any inside the window
/// blocks) - the loop breaker that keeps a machine with failing hardware from
/// minting a new empty database every launch.
fn recent_unopenable_backup(db_path: &Path) -> Option<PathBuf> {
    let dir = db_path.parent()?;
    // `with_extension` on `meridian.db` replaces the `db` extension, so the
    // backups are named `meridian.db.unopenable-backup-<ts>` - full file name
    // plus suffix (same shape `db_crypto::interrupted_migration_leftovers`
    // documents for the plaintext backups).
    let prefix = format!(
        "{}.unopenable-backup-",
        db_path.file_name()?.to_string_lossy()
    );
    let now = std::time::SystemTime::now();
    std::fs::read_dir(dir).ok()?.flatten().find_map(|entry| {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with(&prefix) || name.ends_with("-wal") || name.ends_with("-shm") {
            return None;
        }
        let modified = entry.metadata().ok()?.modified().ok()?;
        let age = now.duration_since(modified).unwrap_or_default();
        (age < FRESH_START_GUARD).then(|| entry.path())
    })
}

/// Polls until no daemon answers, or the grace period expires.
///
/// Proceeding anyway on timeout is deliberate. `repair` re-checks for writers
/// itself and refuses if one is there, so the worst case is a clean refusal
/// reported to the user - whereas blocking startup indefinitely on a daemon
/// that will not die would wedge the tray, which is a far worse failure than a
/// repair that has to be retried.
async fn wait_for_daemon_to_stand_down() {
    let deadline = std::time::Instant::now() + DAEMON_QUIESCE_TIMEOUT;
    let mut logged = false;
    while std::time::Instant::now() < deadline {
        if !meridian::platform::daemon_already_running().await {
            return;
        }
        if !logged {
            tracing::info!("waiting for the daemon to stand down before repairing");
            logged = true;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    tracing::warn!(
        timeout_secs = DAEMON_QUIESCE_TIMEOUT.as_secs(),
        "the daemon is still answering - attempting the repair anyway; it will refuse if it is genuinely still writing"
    );
}

/// One line a user can act on. Deliberately leads with what was kept, not what
/// was lost: the operation's purpose is recovery, and "1 row lost" out of
/// context reads as damage the repair caused rather than damage it survived.
fn summarise(report: &meridian::db::repair::RepairReport) -> String {
    let mut s = format!("Recovered {} records.", report.rows_copied());
    if report.rows_unreadable() > 0 {
        s.push_str(&format!(
            " {} could not be read and were lost.",
            report.rows_unreadable()
        ));
    }
    if report.tables.iter().any(|t| t.left_empty) {
        s.push_str(" Recent screen-activity data was cleared.");
    }
    s
}

#[cfg(test)]
mod probe_tests {
    use super::*;
    use sqlx::Executor;

    /// A valid 64-hex SQLCipher key for probes that need one configured.
    const TEST_KEY: &str = "1111111111111111111111111111111111111111111111111111111111111111";

    async fn create_sound_db(db_path: &Path) {
        let uri = format!("sqlite://{}", db_path.display());
        let pool = meridian_core::db_crypto::open_pool_with_key(&uri, None, true, &[])
            .await
            .expect("create");
        pool.execute("CREATE TABLE t (id INTEGER PRIMARY KEY, payload TEXT NOT NULL)")
            .await
            .expect("schema");
        // Enough rows to push the file well past one page, so damage scribbled
        // over later pages lands on real b-tree content.
        for _ in 0..20 {
            pool.execute(
                "INSERT INTO t (payload) SELECT printf('%.200c', 'x') FROM \
                 (SELECT 1 UNION ALL SELECT 2 UNION ALL SELECT 3 UNION ALL SELECT 4 \
                  UNION ALL SELECT 5 UNION ALL SELECT 6 UNION ALL SELECT 7 UNION ALL SELECT 8)",
            )
            .await
            .expect("rows");
        }
        // Fold everything into the main file: the damage tests scribble over
        // the main file's pages, which only holds the rows once the WAL is
        // checkpointed away.
        pool.execute("PRAGMA wal_checkpoint(TRUNCATE);")
            .await
            .expect("checkpoint");
        pool.close().await;
    }

    /// An absent file and a sound one must both read healthy - the probe runs
    /// on every launch, and a false positive here stops the daemon for a
    /// repair nothing needed.
    #[tokio::test]
    async fn probe_reports_healthy_for_absent_and_sound_databases() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("meridian.db");
        assert!(matches!(probe_database(&db, None).await, Probe::Healthy));

        create_sound_db(&db).await;
        assert!(matches!(probe_database(&db, None).await, Probe::Healthy));
    }

    /// A file that cannot even be opened under the configured key is the
    /// hard-down fleet signature; it must classify as unopenable, never as
    /// merely damaged (a repair would fail to ATTACH it) and never as healthy.
    #[tokio::test]
    async fn probe_flags_garbage_as_unopenable() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("meridian.db");
        std::fs::write(&db, vec![0xABu8; 8192]).unwrap();
        assert!(matches!(
            probe_database(&db, Some(TEST_KEY)).await,
            Probe::Unopenable { .. }
        ));
    }

    /// Damage past page 1 leaves the file openable but structurally broken -
    /// the repairable profile. Bytes are scribbled over interior pages of a
    /// real database, the same shape as the fleet's WAL-desync damage.
    #[tokio::test]
    async fn probe_flags_interior_damage_as_damaged() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("meridian.db");
        create_sound_db(&db).await;

        let len = std::fs::metadata(&db).unwrap().len();
        assert!(len > 4096 * 3, "fixture must span several pages, got {len}");
        use std::io::{Seek, SeekFrom, Write};
        let mut f = std::fs::OpenOptions::new().write(true).open(&db).unwrap();
        f.seek(SeekFrom::Start(4096)).unwrap();
        f.write_all(&vec![0xFFu8; 4096 * 2]).unwrap();
        drop(f);

        assert!(matches!(
            probe_database(&db, None).await,
            Probe::Damaged { .. }
        ));
    }

    /// The unopenable flow end to end: the file is set aside (sidecars too), a
    /// FreshStart outcome names the backup, and a second unopenable file
    /// within the guard window is refused rather than replaced - the loop
    /// breaker for a machine whose disk is eating databases.
    // Plain #[test], not #[tokio::test]: `probe_and_heal` drives its own
    // async work via `tauri::async_runtime::block_on` (it runs in the sync
    // `setup` closure in production), and block_on panics inside an ambient
    // tokio runtime.
    #[test]
    fn unopenable_database_is_set_aside_once_and_only_once() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("meridian.db");
        std::fs::write(&db, vec![0xABu8; 8192]).unwrap();
        std::fs::write(format!("{}-wal", db.display()), b"stale wal").unwrap();

        let outcome = probe_and_heal(&db, Some(TEST_KEY));
        let Some(Outcome::FreshStart { backup }) = outcome else {
            panic!("an unopenable file with a key must produce a fresh start");
        };
        assert!(
            !db.exists(),
            "the unopenable file must leave the canonical path"
        );
        assert!(Path::new(&backup).exists(), "the backup must be preserved");
        assert!(
            Path::new(&format!("{backup}-wal")).exists(),
            "sidecars must follow the main file - a stale -wal beside a fresh \
             database would be offered to SQLite as its log"
        );

        // Same damage again, minutes later: refuse, loudly.
        std::fs::write(&db, vec![0xCDu8; 8192]).unwrap();
        assert!(
            matches!(
                probe_and_heal(&db, Some(TEST_KEY)),
                Some(Outcome::Failed { .. })
            ),
            "a second unopenable verdict inside the guard window must not mint another database"
        );
        assert!(db.exists(), "the refused file must be left in place");
    }

    /// Without a resolved key, "file is not a database" is what a healthy
    /// encrypted file looks like - the file must be left alone. This is the
    /// guard between corruption recovery and destroying data over a keychain
    /// hiccup.
    #[test] // sync for the same block_on reason as the set-aside test above
    fn unopenable_without_a_key_is_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("meridian.db");
        std::fs::write(&db, vec![0xABu8; 8192]).unwrap();

        assert!(probe_and_heal(&db, None).is_none());
        assert!(
            db.exists(),
            "a possibly-just-locked-out file must never be moved"
        );
    }

    /// The guard window is a window, not a permanent latch: a backup older
    /// than [`FRESH_START_GUARD`] no longer blocks recovery.
    #[test]
    fn an_old_unopenable_backup_does_not_block_a_fresh_start() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("meridian.db");
        let old = dir
            .path()
            .join("meridian.db.unopenable-backup-20200101000000");
        std::fs::write(&old, b"old damage").unwrap();
        let stale = std::time::SystemTime::now() - (FRESH_START_GUARD + FRESH_START_GUARD);
        let times = std::fs::FileTimes::new().set_modified(stale);
        std::fs::File::options()
            .write(true)
            .open(&old)
            .unwrap()
            .set_times(times)
            .unwrap();

        assert!(recent_unopenable_backup(&db).is_none());

        let fresh = dir
            .path()
            .join("meridian.db.unopenable-backup-20260805000000");
        std::fs::write(&fresh, b"new damage").unwrap();
        assert_eq!(recent_unopenable_backup(&db), Some(fresh));
    }
}

#[cfg(test)]
mod tests {
    use meridian::db::repair::{RepairReport, TableOutcome};

    fn outcome(table: &str, copied: u64, unreadable: u64, empty: bool) -> TableOutcome {
        TableOutcome {
            table: table.into(),
            rows_copied: copied,
            rows_unreadable: unreadable,
            rows_rejected: 0,
            salvaged_row_by_row: false,
            left_empty: empty,
        }
    }

    /// The clean case must not mention loss at all - a scary sentence about
    /// rows lost when none were is how users learn to distrust the message.
    #[test]
    fn summary_of_a_lossless_repair_mentions_no_loss() {
        let report = RepairReport {
            tables: vec![outcome("app_sessions", 11295, 0, false)],
            ..Default::default()
        };
        let s = super::summarise(&report);
        assert!(s.contains("11295"), "{s}");
        assert!(!s.contains("lost"), "nothing was lost, so say nothing: {s}");
        assert!(!s.contains("cleared"), "{s}");
    }

    /// ...and the lossy case must say so plainly, leading with what survived.
    #[test]
    fn summary_of_a_lossy_repair_names_both_halves() {
        let report = RepairReport {
            tables: vec![
                outcome("pm_worklog_hours", 745, 1, false),
                outcome("capture_frames", 0, 0, true),
            ],
            ..Default::default()
        };
        let s = super::summarise(&report);
        assert!(
            s.starts_with("Recovered 745 records."),
            "lead with the win: {s}"
        );
        assert!(s.contains("1 could not be read"), "{s}");
        assert!(s.contains("screen-activity data was cleared"), "{s}");
    }
}
