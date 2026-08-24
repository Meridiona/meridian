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

/// How many times [`execute_pending_repair`] retries a failed rebuild before
/// giving up and reporting [`Outcome::Failed`].
///
/// Observed on the 2026-08 fleet incident: the same machine logged `database
/// repair failed - the original is unchanged` twice within about a minute of
/// its own repeated auto-repair-at-boot attempts, before a later attempt
/// (elsewhere, hours afterward) finally succeeded. That is not proof of a
/// specific transient cause here the way Windows' os-error-32 file lock is
/// proven in [`meridian_core::retry`] - macOS was the only OS observed in this
/// pattern, and nothing in `repair()`'s own error path currently distinguishes
/// a transient failure from a genuinely unsalvageable one. But a bounded,
/// same-boot retry costs nothing on the common path (a healthy database never
/// reaches this code at all) and directly narrows the gap this fleet evidence
/// pointed at: today, giving a corrupt database more than one chance to repair
/// requires the user (or a relaunch) to reopen the app between every attempt,
/// which an ordinary user has no reason to know to do.
const REPAIR_RETRY_ATTEMPTS: u32 = 3;

/// Linear backoff base for [`REPAIR_RETRY_ATTEMPTS`] - `base × attempt`
/// between tries (2s, then 4s), matching [`meridian_core::retry`]'s schedule.
/// Small enough that even the worst case (two retries) adds only ~6s to a
/// boot that was already about to run a multi-table rebuild.
const REPAIR_RETRY_BASE_DELAY: std::time::Duration = std::time::Duration::from_secs(2);

/// Outcome, kept plain so `lib.rs` can turn it into a notice once it has a pool
/// (this runs before one exists).
#[derive(Debug)]
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

/// `quick_check` ran and found nothing wrong. The ONLY positive health verdict
/// this module can produce.
pub(crate) const VERDICT_VERIFIED: &str = "verified";
/// No database file yet - a fresh install.
pub(crate) const VERDICT_ABSENT: &str = "absent";
/// The probe ran but could not conclude (timeout, or a non-corruption error).
pub(crate) const VERDICT_INCONCLUSIVE: &str = "inconclusive";
/// `quick_check` reported structural damage, or a read failed with SQLITE_CORRUPT
/// (code 11). Page 1 is readable, so the file still ATTACHes and is repairable.
pub(crate) const VERDICT_DAMAGED: &str = "damaged";
/// A read failed with SQLITE_NOTADB (code 26) - page 1 is unreadable UNDER THE
/// KEY THAT WAS USED.
///
/// Kept distinct from [`VERDICT_DAMAGED`] on purpose, and deliberately not
/// called "corrupt": SQLCipher returns code 26 both for a damaged page 1 and
/// for a perfectly healthy database opened with the WRONG KEY, and nothing at
/// this layer can tell them apart (see [`Probe::Unopenable`]). Collapsing the
/// two into one "corrupt" label would rebuild exactly the ambiguity this
/// telemetry exists to remove - measured 2026-08-24, 58 fleet installs could
/// not open their database and only 28 showed code 11, leaving 30 whose cause
/// is still unknown.
pub(crate) const VERDICT_UNOPENABLE: &str = "unopenable";

/// No verdict was ever recorded - the COMMON case, because
/// [`run_auto_if_needed`] only probes when no daemon answers, so a machine
/// whose daemon starts normally never scans its database at all. Distinct from
/// [`VERDICT_INCONCLUSIVE`], which means a probe ran and could not decide.
pub(crate) const VERDICT_UNKNOWN: &str = "unknown";

/// The closed set every persisted verdict must belong to. Read back through
/// [`last_probe_verdict`], which returns one of THESE `&'static str`s rather
/// than the file's contents - so a corrupted or hand-edited sidecar can never
/// put arbitrary text into a telemetry property.
const VERDICTS: &[&str] = &[
    VERDICT_VERIFIED,
    VERDICT_ABSENT,
    VERDICT_INCONCLUSIVE,
    VERDICT_DAMAGED,
    VERDICT_UNOPENABLE,
];

/// Where the last verdict is persisted: beside the database, never inside it.
///
/// That placement is the whole point. The verdict that matters most describes a
/// database that could not be opened at all, so anywhere inside `meridian.db`
/// is unreachable exactly when the value is worth having - the same trap that
/// makes the `db.corrupt` NOTICE (a row in that database) invisible on the
/// machines it most needs to describe.
fn verdict_path(db_path: &Path) -> PathBuf {
    db_path.with_extension("db.probe")
}

/// Persist the probe verdict. Best-effort: a failure here must never affect
/// whether the database gets healed, so it logs at DEBUG and moves on.
fn record_probe_verdict(db_path: &Path, verdict: &str) {
    let path = verdict_path(db_path);
    if let Err(e) = std::fs::write(&path, verdict) {
        tracing::debug!(
            error = %e,
            verdict,
            "could not persist the startup probe verdict - analytics will report it as unknown"
        );
    }
}

/// The last persisted startup probe verdict, or `None` if none was ever
/// written (the common case: [`run_auto_if_needed`] only probes when no daemon
/// answers, so a machine whose daemon starts normally never scans).
///
/// Returns a `&'static str` from [`VERDICTS`] rather than the file's bytes, so
/// a truncated, corrupted, or hand-edited sidecar reads as `None` instead of
/// putting arbitrary text into a shipped telemetry property.
pub(crate) fn last_probe_verdict(db_path: &Path) -> Option<&'static str> {
    let raw = std::fs::read_to_string(verdict_path(db_path)).ok()?;
    let raw = raw.trim();
    VERDICTS.iter().copied().find(|v| *v == raw)
}

/// Why a probe concluded "do not repair".
///
/// The HEALING decision is identical for all three - leave the file alone - so
/// [`probe_database`] deliberately collapses them for that purpose. The
/// ANALYTICS answer is not identical, and collapsing them there would report a
/// database we never scanned as verified-healthy. Only [`NoAction::Verified`]
/// is evidence of health; the other two are evidence of nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NoAction {
    /// `quick_check` ran to completion and returned no problems.
    Verified,
    /// No database file exists yet - a fresh install the daemon has not
    /// created its database for.
    Absent,
    /// The probe could not reach a verdict: it timed out, or failed with an
    /// error that is not a corruption signature. **Not** evidence of health.
    Inconclusive,
}

impl NoAction {
    fn verdict(self) -> &'static str {
        match self {
            Self::Verified => VERDICT_VERIFIED,
            Self::Absent => VERDICT_ABSENT,
            Self::Inconclusive => VERDICT_INCONCLUSIVE,
        }
    }
}

/// What the startup probe concluded about the on-disk `meridian.db`.
#[derive(Debug)]
enum Probe {
    /// Nothing to repair - see [`NoAction`] for which of the three reasons,
    /// which matters to analytics even though it does not matter to healing.
    Healthy(NoAction),
    /// Opens, but `quick_check` reports structural damage. Repairable in
    /// place - page 1 is readable, so the salvage can ATTACH it.
    Damaged { problems: Vec<String> },
    /// The keyed open (or first read) fails with SQLITE_NOTADB (code 26) -
    /// page 1 is unreadable **under the key that was used**. A plain-corrupt
    /// failure (code 11) classifies [`Probe::Damaged`] instead, because a file
    /// that ATTACHes is still salvageable.
    ///
    /// This verdict is deliberately NOT "the file is destroyed". SQLCipher
    /// returns code 26 both for a damaged page 1 and for a perfectly healthy
    /// database opened with the wrong key, and nothing at this layer can tell
    /// them apart. That is why acting on it requires [`KeyTrust`] - the
    /// verdict describes the *pairing* of file and key, not the file.
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
    probe_and_heal(db_path, key_hex, KeyTrust::of(key_hex))
}

/// Whether the key in hand is provably this machine's own database key.
///
/// The fresh-start branch discards a database, and it fires on a SQLCipher
/// verdict (code 26) that means *either* "page 1 is damaged" *or* "this is the
/// wrong key" - SQLCipher cannot tell them apart, and neither can this module.
/// So the destructive branch is gated on provenance instead: only a key the OS
/// keychain confirms is allowed to condemn a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KeyTrust {
    /// The OS keychain holds exactly this key for `meridian.db`, so an
    /// unopenable verdict is about the file, not about the key.
    KeychainVerified,
    /// A key resolved, but nothing proves it owns this database - typically
    /// read from `.env`, which a stale mirror, a hand-edit or a copy from
    /// another machine can populate with a key that opens nothing. An
    /// unopenable verdict here is indistinguishable from a healthy encrypted
    /// file being read with the wrong key.
    Unverified,
}

impl KeyTrust {
    /// Classify the key the tray resolved this launch.
    ///
    /// Split from [`probe_and_heal`] for the same reason the daemon gate is:
    /// so tests can drive the destructive path without the developer's real
    /// keychain deciding the outcome.
    fn of(key_hex: Option<&str>) -> Self {
        match key_hex {
            Some(k) if crate::db_key::key_matches_keychain(k) => Self::KeychainVerified,
            _ => Self::Unverified,
        }
    }
}

/// The probe + heal body behind [`run_auto_if_needed`]'s daemon gate. Split
/// out so tests can exercise it without the gate consulting the development
/// machine's real daemon.
/// # Observability
///
/// This is the least observable path in the tray and the one with the largest
/// consequence - it can move a user's database aside and start an empty one -
/// so the verdict, the trust decision and the outcome are span attributes
/// rather than log lines to be grepped back together, and every failure
/// boundary marks the span `ERROR`. All fields are declared up front:
/// `Span::record` on an undeclared field is a silent no-op, so a status
/// recorded later would never reach the exporter.
fn probe_and_heal(db_path: &Path, key_hex: Option<&str>, key_trust: KeyTrust) -> Option<Outcome> {
    let span = tracing::info_span!(
        "repair_boot.probe_and_heal",
        verdict = tracing::field::Empty,
        key_trust = ?key_trust,
        outcome = tracing::field::Empty,
        otel.status_code = tracing::field::Empty,
    );
    let _enter = span.enter();

    let probe = tauri::async_runtime::block_on(probe_database(db_path, key_hex));
    let verdict = match &probe {
        Probe::Healthy(reason) => reason.verdict(),
        Probe::Damaged { .. } => VERDICT_DAMAGED,
        Probe::Unopenable { .. } => VERDICT_UNOPENABLE,
    };
    span.record("verdict", verdict);
    // Persisted beside the database so the analytics snapshot can report it
    // even when the database itself cannot be opened - see `verdict_path`.
    record_probe_verdict(db_path, verdict);

    match probe {
        Probe::Healthy(_) => {
            span.record("outcome", "none");
            None
        }
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
                span.record("outcome", "marker_write_failed");
                span.record("otel.status_code", "ERROR");
                tracing::error!(error = %meridian::errors::chain(&e), "could not write the repair marker - skipping the automatic repair");
                return None;
            }
            match execute_pending_repair(db_path, key_hex) {
                Outcome::Failed { error } => {
                    // `Damaged` means the file ATTACHes - repairable in
                    // principle - but a file that keeps failing every actual
                    // repair attempt (all `REPAIR_RETRY_ATTEMPTS` of them) is
                    // not actually salvageable in place, whatever the initial
                    // classification promised. Left here, this file fails the
                    // identical way on every future launch forever - the same
                    // dead end `Probe::Unopenable` already has a fallback for,
                    // just reached from the other classification. Observed in
                    // the field (2026-08): a machine whose `meridian db
                    // repair` failed at `ATTACH DATABASE` itself stayed
                    // hard-down across every subsequent launch with no
                    // further signal, exactly this profile.
                    span.record("otel.status_code", "ERROR");
                    tracing::error!(
                        error = %error,
                        "automatic repair failed after retries - falling back to a fresh start, the same last resort an unopenable database already gets"
                    );
                    Some(fresh_start(db_path, "unrepairable", error))
                }
                repaired @ Outcome::Repaired { .. } => {
                    span.record("outcome", "repaired");
                    Some(repaired)
                }
                // `execute_pending_repair` never produces this variant itself
                // - kept exhaustive so a future change to its return type is
                // a compile error here, not a silently mis-recorded span.
                fresh @ Outcome::FreshStart { .. } => {
                    span.record("outcome", "fresh_start");
                    Some(fresh)
                }
            }
        }
        Probe::Unopenable { error } => {
            // Both arms of one rule: only a key the keychain vouches for may
            // condemn a file. `key_hex.is_none()` alone was never enough -
            // the key usually comes from `.env`, so "a key resolved" can mean
            // a stale mirror that opens nothing, and every healthy encrypted
            // database reads as `file is not a database` under the wrong key.
            // Acting on that would discard exactly the data this path exists
            // to protect. See [`KeyTrust`].
            if key_hex.is_none() || key_trust != KeyTrust::KeychainVerified {
                tracing::error!(
                    error = %error,
                    key_resolved = key_hex.is_some(),
                    "meridian.db cannot be opened and the key in hand is not confirmed by the OS keychain - leaving the file alone (a key problem is indistinguishable from corruption here, and only one of the two is safe to act on)"
                );
                span.record("outcome", "left_alone_key_unverified");
                span.record("otel.status_code", "ERROR");
                return None;
            }
            tracing::error!(
                error = %error,
                "meridian.db cannot be opened under its own key - nothing in it is salvageable, moving it aside and starting fresh"
            );
            Some(fresh_start(db_path, "unopenable", error))
        }
    }
}

/// Sets `db_path` aside and reports the resulting [`Outcome`] - the shared
/// last resort both a `Probe::Unopenable` verdict and a `Probe::Damaged`
/// verdict whose own repair attempts still failed fall back to. Refuses a
/// second fresh start of the SAME `kind` within [`FRESH_START_GUARD`] (see
/// [`recent_backup_of_kind`]), so a machine with failing hardware - or a
/// corruption cause that is still active - accumulates one set-aside file and
/// a loud error per incident, not a new empty database every launch.
///
/// `kind` is `"unopenable"` or `"unrepairable"` - distinct backup extensions
/// so an operator reading the directory listing (or Export Diagnostics) knows
/// which failure produced which backup, and so each failure mode gets its own
/// one-shot budget rather than one silently exhausting the other's.
///
/// Callers must have already decided the data cannot be salvaged any other
/// way. No marker/daemon gating of its own beyond the handshake below -
/// both call sites already hold [`run_auto_if_needed`]'s daemon-unreachable
/// gate.
///
/// Records onto `Span::current()`, NOT a passed-in `&Span` - both call sites
/// run synchronously inside [`probe_and_heal`]'s `span.enter()` guard, so
/// `Span::current()` resolves to that span, which already declares
/// `otel.status_code`. This is the documented, checkable pattern
/// (`src/observability/span_status_guard.rs`'s header names
/// `commands::setup::mark_span_failed` as the reference shape) - an explicit
/// `&Span` parameter is NOT: the guard cannot trace a field declaration
/// across a function boundary, so it would flag every `record()` call in
/// this function as an unverifiable, possibly-silent no-op.
fn fresh_start(db_path: &Path, kind: &str, error: String) -> Outcome {
    let span = tracing::Span::current();
    if let Some(previous) = recent_backup_of_kind(db_path, kind) {
        tracing::error!(
            error = %error,
            kind,
            previous_backup = %previous.display(),
            "meridian.db failed the same way again within a day of being replaced - refusing a second fresh start; something is damaging databases faster than they can be replaced"
        );
        span.record("outcome", "refused_repeat_fresh_start");
        span.record("otel.status_code", "ERROR");
        return Outcome::Failed { error };
    }
    // The marker keeps a KeepAlive relaunch out of the window while the file
    // moves - same reasoning as the repair path above. (No `set_running`
    // call: both callers already proved via `run_auto_if_needed`'s gate that
    // no daemon is answering, so there is provably nothing to stop.)
    if let Err(e) = meridian::db::repair::marker::request(db_path) {
        span.record("outcome", "marker_write_failed");
        span.record("otel.status_code", "ERROR");
        tracing::error!(error = %meridian::errors::chain(&e), "could not write the repair marker - leaving the database in place");
        return Outcome::Failed { error };
    }
    let result = set_aside_database(db_path, kind);
    if let Err(e) = meridian::db::repair::marker::clear(db_path) {
        tracing::error!(error = %meridian::errors::chain(&e), "could not clear the repair marker - the daemon will stay down until it expires");
    }
    match result {
        Ok(backup) => {
            span.record("outcome", "fresh_start");
            Outcome::FreshStart {
                backup: backup.display().to_string(),
            }
        }
        Err(e) => {
            span.record("outcome", "set_aside_failed");
            span.record("otel.status_code", "ERROR");
            tracing::error!(error = %meridian::errors::chain(&e), "could not move the database aside - leaving it in place");
            Outcome::Failed {
                error: format!("{e:#}"),
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
        retry_repair(|| meridian::db::repair::repair(db_path, key_hex)).await
    });

    // Clear before reporting: a panic while formatting the summary must not
    // leave the marker behind and the daemon permanently down.
    if let Err(e) = meridian::db::repair::marker::clear(db_path) {
        tracing::error!(error = %meridian::errors::chain(&e), "could not clear the repair marker - the daemon will stay down until it expires");
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
            tracing::error!(error = %meridian::errors::chain(&e), "database repair failed - the original is unchanged");
            Outcome::Failed {
                error: format!("{e:#}"),
            }
        }
    }
}

/// Runs `repair_fn` up to [`REPAIR_RETRY_ATTEMPTS`] times with
/// [`REPAIR_RETRY_BASE_DELAY`] linear backoff, via the shared
/// [`meridian_core::retry::retry_transient`] - see [`REPAIR_RETRY_ATTEMPTS`]'s
/// doc for why this exists. `repair()` never modifies the original database
/// until its final successful swap, so retrying a failed attempt costs
/// nothing beyond time: each attempt either fails having changed nothing, or
/// the whole operation succeeds.
///
/// Split out (rather than inlined into [`execute_pending_repair`]) so the
/// retry/backoff wiring is testable with a fake fallible closure, the same
/// way [`KeyTrust::of`] and [`probe_and_heal`] are split from their real I/O
/// for the same reason.
async fn retry_repair<F, Fut>(repair_fn: F) -> anyhow::Result<meridian::db::repair::RepairReport>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<meridian::db::repair::RepairReport>>,
{
    meridian_core::retry::retry_transient(REPAIR_RETRY_ATTEMPTS, REPAIR_RETRY_BASE_DELAY, repair_fn)
        .await
}

/// Classifies the on-disk database. Read-only and safe against a live daemon:
/// the open takes no write lock and `quick_check` reads a WAL snapshot.
///
/// Every inconclusive path deliberately reports [`Probe::Healthy`]: repairing
/// on uncertainty would stop the daemon and rebuild a database over an error
/// that may have been a transient lock. Only the two positively-identified
/// corruption signatures trigger action.
#[tracing::instrument(
    name = "repair_boot.probe_database",
    // `skip_all`, not a field list: `key_hex` IS the database encryption key
    // and must never reach a span, and `db_path` carries the user's home
    // directory. The verdict is recorded by the caller's span instead.
    skip_all
)]
async fn probe_database(db_path: &Path, key_hex: Option<&str>) -> Probe {
    if !db_path.exists() {
        return Probe::Healthy(NoAction::Absent); // the daemon will create it
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
            tracing::warn!(error = %meridian::errors::chain(&e), "startup probe could not open meridian.db for a reason that is not corruption - proceeding normally");
            return Probe::Healthy(NoAction::Inconclusive);
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
            Probe::Healthy(NoAction::Inconclusive)
        }
        Ok(Ok(problems)) if problems.is_empty() => Probe::Healthy(NoAction::Verified),
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
            tracing::warn!(error = %meridian::errors::chain(&e), "startup integrity probe errored inconclusively - proceeding normally");
            Probe::Healthy(NoAction::Inconclusive)
        }
    }
}

/// Attempts for [`set_aside_database`]'s renames - cheap-failure semantics
/// (~1s of linear backoff): if every attempt fails, [`fresh_start`] just
/// reports [`Outcome::Failed`] having changed nothing, and the NEXT app
/// launch re-probes and retries the whole operation from scratch. Matches
/// `meridian_core::db_crypto::RENAME_ATTEMPTS_CHEAP`'s reasoning exactly -
/// not reused directly since that constant is private to `db_crypto`.
const SET_ASIDE_RENAME_ATTEMPTS: u32 = 5;

/// Moves a database (and its `-wal`/`-shm` sidecars) aside under a
/// timestamped `db.<kind>-backup-<ts>` name, so a fresh one can be created at
/// the canonical path. The file is preserved, never deleted - it is the only
/// copy of whatever a future offline-salvage attempt might still extract.
///
/// Every rename retries through [`meridian_core::retry::retry_transient_blocking`]
/// - **required**, not defence in depth: this always runs moments after a
/// `repair()` attempt against the very same `db_path` (either the failed
/// `ATTACH` on the `Damaged` path, or the classification probe's own open on
/// the `Unopenable` path), and on Windows a rename fails with os error 32
/// ("used by another process") for as long as any handle to the file is
/// still closing - exactly the failure `db_crypto::rename_with_retry` exists
/// for (see its doc: "the rebuilt file was still held open by a connection
/// `pool.close()` had not actually closed"). Reproduced directly by this
/// module's own `damaged_page_one_that_attach_cannot_salvage_falls_back_to_fresh_start`
/// test on Windows CI before this retry was added.
#[tracing::instrument(name = "repair_boot.set_aside_database", skip(db_path))]
fn set_aside_database(db_path: &Path, kind: &str) -> anyhow::Result<PathBuf> {
    use anyhow::Context;
    let backup = db_path.with_extension(format!(
        "db.{kind}-backup-{}",
        chrono::Utc::now().format("%Y%m%d%H%M%S")
    ));
    meridian_core::retry::retry_transient_blocking(
        SET_ASIDE_RENAME_ATTEMPTS,
        std::time::Duration::from_millis(100),
        || std::fs::rename(db_path, &backup),
    )
    .with_context(|| format!("moving the {kind} database aside to {}", backup.display()))?;
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
        let rename_result = meridian_core::retry::retry_transient_blocking(
            SET_ASIDE_RENAME_ATTEMPTS,
            std::time::Duration::from_millis(100),
            || std::fs::rename(&sidecar, &kept),
        );
        if let Err(rename_err) = rename_result {
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

/// Any `<kind>-backup` set aside within [`FRESH_START_GUARD`] (first hit in
/// directory order - which one does not matter, any inside the window
/// blocks) - the loop breaker that keeps a machine with failing hardware (or
/// a corruption cause still active) from minting a new empty database every
/// launch. Scoped to `kind` so an unopenable-file incident and an
/// unrepairable-damage incident each get their own one-shot budget.
fn recent_backup_of_kind(db_path: &Path, kind: &str) -> Option<PathBuf> {
    let dir = db_path.parent()?;
    // `with_extension` on `meridian.db` replaces the `db` extension, so the
    // backups are named `meridian.db.<kind>-backup-<ts>` - full file name
    // plus suffix (same shape `db_crypto::interrupted_migration_leftovers`
    // documents for the plaintext backups).
    let prefix = format!("{}.{kind}-backup-", db_path.file_name()?.to_string_lossy());
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
        assert!(matches!(
            probe_database(&db, None).await,
            Probe::Healthy(..)
        ));

        create_sound_db(&db).await;
        assert!(matches!(
            probe_database(&db, None).await,
            Probe::Healthy(..)
        ));
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

    /// Damage to page 1's own schema b-tree (past the 100-byte file header,
    /// which stays intact so the file still opens and classifies `Damaged`,
    /// never `Unopenable`) reproduces the actual 2026-08-24 field incident:
    /// `quick_check` reports damage, but `ATTACH DATABASE` inside `repair()`
    /// ALSO fails on the very same file, because SQLCipher's `ATTACH` eagerly
    /// re-reads the attached file's page 1 rather than lazily loading its
    /// schema the way plain SQLite does. That combination - repairable by
    /// classification, unrepairable in practice - is exactly what
    /// [`fresh_start`]'s new call site in the `Damaged` arm exists for.
    // Plain #[test], not #[tokio::test] - same reason as
    // `unopenable_database_is_set_aside_once_and_only_once`: `probe_and_heal`
    // drives its own async work via `block_on`, which panics inside an
    // ambient tokio runtime. The async setup below gets its OWN runtime,
    // dropped before `probe_and_heal` runs.
    #[test]
    fn damaged_page_one_that_attach_cannot_salvage_falls_back_to_fresh_start() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("meridian.db");
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(create_sound_db(&db));

        use std::io::{Seek, SeekFrom, Write};
        let mut f = std::fs::OpenOptions::new().write(true).open(&db).unwrap();
        // Bytes 0-99 are the SQLite file header (magic, page size, ...) -
        // left untouched so the file still opens. Everything from 100 to the
        // end of page 1 is the schema b-tree's own content; scribbling over
        // it breaks both `quick_check` (Damaged, not Unopenable) and
        // `ATTACH`'s eager schema read (fails identically).
        f.seek(SeekFrom::Start(100)).unwrap();
        f.write_all(&vec![0xFFu8; 4096 - 100]).unwrap();
        drop(f);

        // Precondition: this really is the `Damaged` profile, not `Unopenable`
        // - otherwise this test would be exercising the OTHER arm.
        assert!(
            matches!(rt.block_on(probe_database(&db, None)), Probe::Damaged { .. }),
            "fixture must classify as Damaged, not Unopenable, to exercise the repair-failure fallback"
        );
        drop(rt);

        let outcome = probe_and_heal(&db, None, KeyTrust::Unverified);
        let Some(Outcome::FreshStart { backup }) = outcome else {
            panic!(
                "a Damaged database whose own repair attempts fail must fall back \
                 to a fresh start, got {outcome:?}"
            );
        };
        assert!(
            backup.contains("unrepairable-backup"),
            "the backup must be tagged as an unrepairable-kind fresh start, not unopenable: {backup}"
        );
        assert!(
            !db.exists(),
            "the unrepairable file must leave the canonical path so a fresh one can take it"
        );
        assert!(Path::new(&backup).exists(), "the backup must be preserved");
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

        let outcome = probe_and_heal(&db, Some(TEST_KEY), KeyTrust::KeychainVerified);
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
                probe_and_heal(&db, Some(TEST_KEY), KeyTrust::KeychainVerified),
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

        assert!(probe_and_heal(&db, None, KeyTrust::Unverified).is_none());
        assert!(
            db.exists(),
            "a possibly-just-locked-out file must never be moved"
        );
    }

    /// The same protection, for the case that actually reaches users: a key
    /// that resolved but is not the one this machine's keychain holds.
    ///
    /// This is not hypothetical. The key normally comes from `.env`, and the
    /// keychain is consulted only when `.env` has none - so a stale mirror, a
    /// hand-edited file, or a `.env` copied between machines all produce a key
    /// that resolves and opens nothing. SQLCipher then reports the healthy
    /// encrypted database as `file is not a database`, identically to real
    /// page-1 damage.
    ///
    /// Before [`KeyTrust`], the gate was `key_hex.is_none()` - which this case
    /// passes - so a wrong `.env` key was enough to move a perfectly good
    /// database aside and start an empty one in its place. It also takes the
    /// daemon down with it (same wrong key, same failure), so the
    /// daemon-unreachable gate does not catch it either.
    #[test] // sync for the same block_on reason as the set-aside test above
    fn unopenable_under_an_unverified_key_is_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("meridian.db");
        std::fs::write(&db, vec![0xABu8; 8192]).unwrap();

        assert!(
            probe_and_heal(&db, Some(TEST_KEY), KeyTrust::Unverified).is_none(),
            "an unopenable verdict under an unconfirmed key must not condemn \
             the file - it is what a healthy encrypted database looks like \
             through the wrong key"
        );
        assert!(
            db.exists(),
            "the database must still be there for a recovered key to open"
        );
        assert!(
            !meridian::db::repair::marker::pending(&db),
            "and no repair marker may be left behind holding the daemon down"
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

        assert!(recent_backup_of_kind(&db, "unopenable").is_none());

        let fresh = dir
            .path()
            .join("meridian.db.unopenable-backup-20260805000000");
        std::fs::write(&fresh, b"new damage").unwrap();
        assert_eq!(recent_backup_of_kind(&db, "unopenable"), Some(fresh));
    }

    /// Each `kind` gets its OWN one-shot budget: a recent `unopenable` backup
    /// must not block an `unrepairable` fresh start, or vice versa - the two
    /// failure modes are unrelated incidents and must not exhaust each
    /// other's guard window.
    #[test]
    fn guard_window_is_scoped_per_kind() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("meridian.db");
        std::fs::write(
            dir.path()
                .join("meridian.db.unopenable-backup-20260824000000"),
            b"damage",
        )
        .unwrap();

        assert!(
            recent_backup_of_kind(&db, "unopenable").is_some(),
            "the unopenable backup must be found under its own kind"
        );
        assert!(
            recent_backup_of_kind(&db, "unrepairable").is_none(),
            "an unopenable backup must not block an unrepairable fresh start"
        );
    }

    /// A repair that fails on its first attempts but succeeds within
    /// [`REPAIR_RETRY_ATTEMPTS`] must recover, not report [`Outcome::Failed`]
    /// - the whole point of [`REPAIR_RETRY_ATTEMPTS`]'s hardening: giving a
    /// transient failure a same-boot second chance instead of requiring the
    /// user to relaunch the app between every attempt.
    #[tokio::test(start_paused = true)]
    async fn retry_repair_recovers_within_the_attempt_budget() {
        use std::sync::atomic::{AtomicU32, Ordering};
        let calls = AtomicU32::new(0);
        let result = retry_repair(|| {
            let n = calls.fetch_add(1, Ordering::SeqCst);
            async move {
                if n + 1 < REPAIR_RETRY_ATTEMPTS {
                    Err(anyhow::anyhow!("transient failure"))
                } else {
                    Ok(meridian::db::repair::RepairReport::default())
                }
            }
        })
        .await;
        assert!(result.is_ok(), "must recover within the retry budget");
        assert_eq!(
            calls.load(Ordering::SeqCst),
            REPAIR_RETRY_ATTEMPTS,
            "no extra attempt past the success"
        );
    }

    /// A repair that never succeeds must still give up after exactly
    /// [`REPAIR_RETRY_ATTEMPTS`] tries and surface the last error - a
    /// genuinely unsalvageable database must not retry forever.
    #[tokio::test(start_paused = true)]
    async fn retry_repair_gives_up_after_exhausting_the_attempt_budget() {
        use std::sync::atomic::{AtomicU32, Ordering};
        let calls = AtomicU32::new(0);
        let result = retry_repair(|| {
            calls.fetch_add(1, Ordering::SeqCst);
            async { Err(anyhow::anyhow!("still broken")) }
        })
        .await;
        assert!(result.is_err());
        assert_eq!(calls.load(Ordering::SeqCst), REPAIR_RETRY_ATTEMPTS);
    }

    /// The common path - success on the very first attempt - must not pay any
    /// backoff or retry cost at all.
    #[tokio::test]
    async fn retry_repair_returns_on_first_success_without_retrying() {
        use std::sync::atomic::{AtomicU32, Ordering};
        let calls = AtomicU32::new(0);
        let result = retry_repair(|| {
            calls.fetch_add(1, Ordering::SeqCst);
            async { Ok(meridian::db::repair::RepairReport::default()) }
        })
        .await;
        assert!(result.is_ok());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}

#[cfg(test)]
mod tests {
    use meridian::db::repair::{RepairReport, TableOutcome};

    /// A verdict written by one release must still read back on the next, and
    /// nothing outside the closed set may ever reach a telemetry property.
    #[test]
    fn a_persisted_verdict_round_trips_and_rejects_anything_unrecognised() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = dir.path().join("meridian.db");

        // Never probed - the common case on a machine whose daemon starts fine.
        assert_eq!(super::last_probe_verdict(&db), None);

        for verdict in super::VERDICTS {
            super::record_probe_verdict(&db, verdict);
            assert_eq!(super::last_probe_verdict(&db), Some(*verdict));
        }

        // A truncated, hand-edited or corrupted sidecar must read as "never
        // recorded", NOT as arbitrary text shipped to PostHog as a property.
        for junk in [
            "",
            "corrupt",
            "healthy",
            "../../etc/passwd",
            "verified\u{0}x",
        ] {
            std::fs::write(db.with_extension("db.probe"), junk).expect("write");
            assert_eq!(
                super::last_probe_verdict(&db),
                None,
                "unrecognised sidecar content must not reach telemetry: {junk:?}"
            );
        }
    }

    /// The sidecar must live beside the database, never inside it - the whole
    /// point is to survive a database that cannot be opened.
    #[test]
    fn the_verdict_sidecar_is_a_separate_file_from_the_database() {
        let db = std::path::Path::new("/tmp/x/meridian.db");
        let sidecar = super::verdict_path(db);
        assert_ne!(sidecar, db);
        assert_eq!(sidecar.parent(), db.parent());
        assert_eq!(sidecar.file_name().unwrap(), "meridian.db.probe");
    }

    /// Only a completed `quick_check` may report health. An absent file and an
    /// inconclusive probe both mean "do not repair", and collapsing them into
    /// the positive verdict is exactly how a never-scanned database would come
    /// to read as verified-healthy in PostHog.
    #[test]
    fn only_a_completed_check_reports_the_positive_verdict() {
        assert_eq!(super::NoAction::Verified.verdict(), super::VERDICT_VERIFIED);
        assert_eq!(super::NoAction::Absent.verdict(), super::VERDICT_ABSENT);
        assert_eq!(
            super::NoAction::Inconclusive.verdict(),
            super::VERDICT_INCONCLUSIVE
        );
        assert_ne!(super::NoAction::Absent.verdict(), super::VERDICT_VERIFIED);
        assert_ne!(
            super::NoAction::Inconclusive.verdict(),
            super::VERDICT_VERIFIED
        );
    }

    /// `damaged` (code 11) and `unopenable` (code 26) must stay distinct.
    /// Code 26 is returned both for a damaged page 1 AND for a healthy database
    /// opened with the wrong key, so folding them into one "corrupt" label
    /// rebuilds the ambiguity this telemetry exists to remove.
    #[test]
    fn the_two_failure_verdicts_are_never_the_same_value() {
        assert_ne!(super::VERDICT_DAMAGED, super::VERDICT_UNOPENABLE);
        assert!(!super::VERDICTS.contains(&"corrupt"));
    }

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
