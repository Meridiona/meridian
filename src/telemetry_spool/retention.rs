//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Local spool housekeeping: crash-orphan sweep, age-based retention, oldest-
//! first pending-size cap, and the ordering `shipper.rs` ships files in. Split
//! out of `shipper.rs` (which was over the repo's 500-line cap) since these
//! are self-contained disk-maintenance operations, independent of the
//! HTTP-delivery logic the rest of that module owns — they run even on a
//! Canonical/packaged install that never ships anything.
//!
//! # Who calls this
//! `shipper::run_tick()`, every `MERIDIAN_TELEMETRY_SHIP_INTERVAL_S` (default
//! 30s), before the ship-target check — as ONE [`run_housekeeping`] unit on
//! `spawn_blocking`, never inline on the runtime: orphan sweep → age-based
//! prune (both `pending/` and `sent/`, rationed to every 60th tick — see
//! [`prune_due`]) → pending-size cap → (only then) ship attempt.
//!
//! # Related
//! - `writer.rs` — the `<signal>-<unix_micros>-<seq>.otlp` filename scheme
//!   [`list_pending_oldest_first`] and [`enforce_pending_cap`] parse.
//! - `shipper::run_tick` — the sole caller, and the thing whose delivery
//!   order [`list_pending_oldest_first`] determines.

use std::{
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::Result;

use crate::telemetry_spool::writer::{micros_from_filename, seq_from_filename};

const DEFAULT_MAX_PENDING_MB: u64 = 512;
/// `.otlp.tmp` files older than this are crash orphans — a healthy write turns
/// tmp → final in milliseconds, so anything this old will never be completed.
const TMP_ORPHAN_MAX_AGE_SECS: u64 = 300;

pub(super) fn max_pending_bytes() -> u64 {
    let mb = std::env::var("MERIDIAN_TELEMETRY_MAX_PENDING_MB")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(DEFAULT_MAX_PENDING_MB);
    mb * 1024 * 1024
}

/// Remove `.otlp.tmp` files left behind by a crash between write and rename.
/// A healthy write completes the rename in milliseconds, so any tmp older than
/// `TMP_ORPHAN_MAX_AGE_SECS` is dead weight the cap/lister would never see.
pub(super) fn sweep_tmp_orphans(pending: &Path) {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let Ok(entries) = std::fs::read_dir(pending) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !name.ends_with(".otlp.tmp") {
            continue;
        }
        let age = entry
            .metadata()
            .and_then(|m| m.modified())
            .map(|mt| {
                now.saturating_sub(mt.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs())
            })
            .unwrap_or(u64::MAX);
        if age >= TMP_ORPHAN_MAX_AGE_SECS {
            let _ = std::fs::remove_file(&path);
            tracing::debug!(file = %path.display(), age_secs = age, "swept crash-orphaned spool tmp file");
        }
    }
}

/// List `.otlp` files in `dir` sorted oldest-first by `(micros, seq)`.
///
/// Including `seq` makes ordering deterministic when two files share a
/// microsecond (traces+logs back-to-back, or a burst). Files with an
/// unparseable name are SKIPPED rather than collapsed to key `0` — a renamed /
/// foreign file must not sort permanently to the front and ship every tick.
pub(super) fn list_pending_oldest_first(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return vec![];
    };

    let mut files: Vec<(u64, u64, PathBuf)> = entries
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            let name = p.file_name()?.to_str()?.to_string();
            if !name.ends_with(".otlp") {
                return None;
            }
            let micros = micros_from_filename(&name)?;
            let seq = seq_from_filename(&name)?;
            Some((micros, seq, p))
        })
        .collect();

    files.sort_by_key(|(m, s, _)| (*m, *s));
    files.into_iter().map(|(_, _, p)| p).collect()
}

/// Should tick `tick` run the age prune? Tick 0, then every 60th tick
/// (~every 30 min at the default 30 s ship interval).
///
/// The prune scans BOTH spool dirs file-by-file, and `sent/` sits at a
/// ~264k-file steady state (one file per flush x 7-day retention) — measured
/// at 263,787 files on 2026-08-05, when thread sampling caught the shipper
/// tick pinned in `prune_by_age -> stat` on a tokio runtime worker while the
/// daemon's poll timers ran 30-100 s late and its socket greeting starved
/// (the probe false-negatives behind the tray's status flapping and the old
/// watchdog storms). A 7-day cutoff needs nothing close to twice-a-minute
/// precision: every 60th tick bounds the scan cost, and tick 0 still prunes
/// so a daemon that was stopped for days catches up on its first tick. Same
/// tick-counter idiom as `etl::capture_retention`'s vacuum cadence.
pub(super) fn prune_due(tick: u64) -> bool {
    tick.is_multiple_of(60)
}

/// The shipper tick's whole filesystem sweep, as one blocking unit: clear
/// crash-orphaned tmp files, age-prune both dirs (only when `prune` — see
/// [`prune_due`]), and enforce the pending-size cap.
///
/// Exists so `shipper::run_tick` can hand the entire sweep to
/// `tokio::task::spawn_blocking` and never touch these helpers from async
/// context itself — every call here walks directories and `stat`s files, and
/// doing that on a runtime worker is exactly the stall described on
/// [`prune_due`]. The shipper's source is pinned to this split by
/// `housekeeping_stays_off_the_async_runtime` in `shipper.rs`.
pub(super) fn run_housekeeping(
    pending: &Path,
    sent: &Path,
    quarantine: &Path,
    cutoff_secs: u64,
    prune: bool,
) -> Result<()> {
    sweep_tmp_orphans(pending);
    if prune {
        prune_by_age(pending, cutoff_secs)?;
        prune_by_age(sent, cutoff_secs)?;
        // `quarantine/` was covered by nothing: not by retention, and not by
        // any reader either - neither `build_export_bundle` nor `meridian logs`
        // looks at it. A permanently-rejected payload was therefore immortal
        // AND invisible, the one genuinely unbounded directory in the spool.
        prune_by_age(quarantine, cutoff_secs)?;
    }
    crate::telemetry_spool::launchd_log_cap::cap_launchd_logs();
    enforce_pending_cap(pending)?;
    // `sent/` had an age policy and no volume policy at all, so its size was
    // whatever the emission rate happened to produce inside the 7-day window -
    // ~800 MB across ~150k files on the two machines measured, and nothing
    // would have stopped 2 GB on a busier one. The batching fix in
    // `observability::init` addresses the rate; this bounds the outcome
    // regardless of rate, which is what makes it a ceiling rather than a
    // second guess.
    enforce_sent_cap(sent)
}

/// Delete `.otlp` files in `dir` whose mtime is older than `cutoff_secs`.
/// Used for both `pending/` and `sent/` — retention applies to spooled
/// telemetry regardless of whether it was ever shipped (a Canonical/packaged
/// install never ships at all, so `pending/` is its only spool location).
pub(super) fn prune_by_age(dir: &Path, cutoff_secs: u64) -> Result<()> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let Ok(entries) = std::fs::read_dir(dir) else {
        return Ok(());
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "otlp") {
            continue;
        }
        if let Ok(meta) = std::fs::metadata(&path) {
            if let Ok(mtime) = meta.modified() {
                let age_secs = now.saturating_sub(
                    mtime
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs(),
                );
                // `>=` so a retention of 0 days means "keep nothing" (prune even
                // just-shipped files); the default 7-day cutoff is unaffected since
                // a fresh file's age (~0s) is never >= 604800s.
                if age_secs >= cutoff_secs {
                    let _ = std::fs::remove_file(&path);
                    tracing::debug!(file = %path.display(), age_days = age_secs / 86400, "pruned old telemetry file");
                }
            }
        }
    }
    Ok(())
}

/// Drop OLDEST pending files beyond the size cap with a structured warning.
/// Never silently drops — always emits `tracing::warn!` with count + bytes.
pub(super) fn enforce_pending_cap(pending: &Path) -> Result<()> {
    let max = max_pending_bytes();

    let Ok(entries) = std::fs::read_dir(pending) else {
        return Ok(());
    };

    let mut files: Vec<(u64, u64, u64, PathBuf)> = entries // (micros, seq, size, path)
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            let name = p.file_name()?.to_str()?.to_string();
            if !name.ends_with(".otlp") {
                return None;
            }
            let micros = micros_from_filename(&name)?;
            let seq = seq_from_filename(&name)?;
            let size = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
            Some((micros, seq, size, p))
        })
        .collect();

    // Sort oldest-first by (micros, seq) — same total order the shipper uses, so
    // the cap evicts the genuinely-oldest records, not whatever read_dir yields.
    files.sort_by_key(|(m, s, _, _)| (*m, *s));

    let total: u64 = files.iter().map(|(_, _, s, _)| s).sum();
    if total <= max {
        return Ok(());
    }

    let mut to_drop = total - max;
    let mut dropped_count = 0u64;
    let mut dropped_bytes = 0u64;

    for (_, _, size, path) in &files {
        if to_drop == 0 {
            break;
        }
        let _ = std::fs::remove_file(path);
        dropped_bytes += size;
        dropped_count += 1;
        to_drop = to_drop.saturating_sub(*size);
    }

    if dropped_count > 0 {
        tracing::warn!(
            dropped_files = dropped_count,
            dropped_bytes,
            cap_bytes = max,
            "pending telemetry cap exceeded — oldest spool files dropped"
        );
    }

    Ok(())
}

/// Default ceiling on `sent/`, the local archive of already-delivered payloads.
///
/// Generous on purpose: `sent/` is not dead weight. `meridian logs` reads
/// `pending/` AND `sent/` (`render::collect_all_records`), so without it local
/// log history on a shipping install would be about thirty seconds deep, and
/// `build_export_bundle` archives both. The point is a ceiling, not a purge.
const DEFAULT_MAX_SENT_MB: u64 = 256;

/// Ceiling for [`enforce_sent_cap`], overridable for operators with unusual
/// retention needs.
pub(super) fn max_sent_bytes() -> u64 {
    let mb = std::env::var("MERIDIAN_TELEMETRY_MAX_SENT_MB")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(DEFAULT_MAX_SENT_MB);
    mb.saturating_mul(1024 * 1024)
}

/// Drop the OLDEST already-shipped files once `sent/` exceeds its size cap.
///
/// # Why an age policy was not enough
///
/// `sent/` had a 7-day cutoff and no volume policy, so its size was whatever
/// the emission rate produced inside that window. Measured on two machines:
/// ~150k files and ~800 MB, with the module's own docs recording a ~264k-file
/// steady state. Nothing in the code would have stopped 2 GB on a busier
/// machine - the bound was on time, not on space.
///
/// That is not only a disk-usage problem. `build_export_bundle` archives every
/// file in here, and `docs/privacy.md` asks the user to inspect the archive
/// before sending it to support. Nobody inspects 150,000 protobuf files, so
/// the consent step that makes Export Diagnostics acceptable was defeated by
/// sheer count.
///
/// Evicts oldest-first by `(micros, seq)` - the same total order the shipper
/// and [`enforce_pending_cap`] use - so the newest, most diagnostically useful
/// records are the ones kept. Logged at `debug`, not `warn`: unlike the pending
/// cap, dropping something already delivered to OpenObserve loses no data that
/// was not already sent, so it is routine housekeeping rather than a fault.
pub(super) fn enforce_sent_cap(sent: &Path) -> Result<()> {
    let max = max_sent_bytes();

    let Ok(entries) = std::fs::read_dir(sent) else {
        return Ok(());
    };

    let mut files: Vec<(u64, u64, u64, PathBuf)> = entries
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            let name = p.file_name()?.to_str()?.to_string();
            if !name.ends_with(".otlp") {
                return None;
            }
            let micros = micros_from_filename(&name)?;
            let seq = seq_from_filename(&name)?;
            let size = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
            Some((micros, seq, size, p))
        })
        .collect();

    files.sort_by_key(|(m, s, _, _)| (*m, *s));

    let total: u64 = files.iter().map(|(_, _, s, _)| s).sum();
    if total <= max {
        return Ok(());
    }

    let mut to_drop = total - max;
    let mut dropped_count = 0u64;
    let mut dropped_bytes = 0u64;
    for (_, _, size, path) in &files {
        if to_drop == 0 {
            break;
        }
        let _ = std::fs::remove_file(path);
        dropped_bytes += size;
        dropped_count += 1;
        to_drop = to_drop.saturating_sub(*size);
    }

    if dropped_count > 0 {
        tracing::debug!(
            dropped_files = dropped_count,
            dropped_bytes,
            cap_bytes = max,
            "sent telemetry cap exceeded - oldest already-shipped files dropped"
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::telemetry_spool::writer::{pending_dir, write_pending};
    use tempfile::TempDir;

    #[test]
    fn list_pending_sorted_oldest_first() {
        let dir = TempDir::new().unwrap();
        let base = dir.path().to_path_buf();

        // Write files with different seq numbers; filename micros will be equal
        // or ascending since they run in sequence. Seq counter disambiguates.
        let p1 = write_pending(&base, "traces", b"old").unwrap();
        let p2 = write_pending(&base, "traces", b"new").unwrap();

        let pending = pending_dir(&base);
        let sorted = list_pending_oldest_first(&pending);
        assert_eq!(sorted.len(), 2);
        // First file written should appear first (lower seq or lower micros)
        assert_eq!(sorted[0], p1);
        assert_eq!(sorted[1], p2);
    }

    #[test]
    fn list_pending_orders_same_micros_by_seq_and_skips_unparseable() {
        let dir = TempDir::new().unwrap();
        let pending = pending_dir(dir.path());
        std::fs::create_dir_all(&pending).unwrap();

        // Same microsecond, out-of-order seq — must come back seq 0 then seq 1.
        std::fs::write(pending.join("traces-1000-1.otlp"), b"b").unwrap();
        std::fs::write(pending.join("traces-1000-0.otlp"), b"a").unwrap();
        // A crash-orphan tmp and a foreign name must be ignored entirely (the
        // old `unwrap_or(0)` would have sorted the foreign file permanently first).
        std::fs::write(pending.join("traces-1000-2.otlp.tmp"), b"x").unwrap();
        std::fs::write(pending.join("garbage.otlp"), b"y").unwrap();

        let sorted = list_pending_oldest_first(&pending);
        assert_eq!(sorted.len(), 2, "tmp + foreign names excluded");
        assert!(sorted[0]
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .ends_with("-0.otlp"));
        assert!(sorted[1]
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .ends_with("-1.otlp"));
    }

    /// The prune cadence. Every 30 s ship tick used to age-scan BOTH spool
    /// dirs, and `sent/` sits at a ~264k-file steady state (one file per
    /// flush x 7-day retention) - so the daemon issued ~264k blocking `stat`s
    /// twice a minute on a tokio runtime worker. Caught live by thread
    /// sampling on 2026-08-05: the shipper tick was pinned in
    /// `prune_by_age -> stat` while the daemon's 60 s poll timers fired
    /// 30-100 s late and its socket greeting starved (the probe
    /// false-negatives behind the status flapping and the old watchdog
    /// storms). A 7-day cutoff does not need twice-a-minute precision; every
    /// 60th tick is ample, and the first tick still prunes so a long-stopped
    /// daemon catches up promptly on start.
    #[test]
    fn prune_runs_on_the_first_tick_then_every_60th() {
        assert!(
            prune_due(0),
            "first tick must prune (catch-up after downtime)"
        );
        for t in 1..60 {
            assert!(
                !prune_due(t),
                "tick {t} must not prune - the scan is ~264k stats"
            );
        }
        assert!(prune_due(60));
        assert!(!prune_due(61));
        assert!(prune_due(120));
    }

    /// One call owning the whole blocking sweep, so the shipper can hand it to
    /// `spawn_blocking` as a unit and its own source never touches the
    /// filesystem helpers directly (pinned by a source-scan in `shipper.rs`).
    ///
    /// Asserted on `sent/` only: `pending/` is also subject to
    /// `enforce_pending_cap`, whose size cap reads an env var that other
    /// tests mutate - under a parallel run the cap can legitimately drop a
    /// pending file and turn a gating assertion flaky. Only the age prune
    /// ever touches `sent/`, so it isolates the gate. (Both-dirs prune
    /// coverage lives in the `prune_by_age_*` tests above.)
    #[test]
    fn run_housekeeping_prunes_only_when_due() {
        let dir = TempDir::new().unwrap();
        let pending = pending_dir(dir.path());
        std::fs::create_dir_all(&pending).unwrap();
        let sent = dir.path().join("sent");
        std::fs::create_dir_all(&sent).unwrap();
        let s = sent.join("traces-1-0.otlp");
        std::fs::write(&s, b"x").unwrap();

        let quarantine = dir.path().join("quarantine");
        std::fs::create_dir_all(&quarantine).unwrap();

        // Not due: survives a keep-nothing cutoff.
        run_housekeeping(&pending, &sent, &quarantine, 0, false).unwrap();
        assert!(s.exists(), "not-due housekeeping must not prune");

        // Due: the 0-second cutoff removes it.
        run_housekeeping(&pending, &sent, &quarantine, 0, true).unwrap();
        assert!(!s.exists(), "due housekeeping must prune");
    }

    #[test]
    fn prune_by_age_removes_old_files_in_sent() {
        let dir = TempDir::new().unwrap();
        let sent = dir.path().join("sent");
        std::fs::create_dir_all(&sent).unwrap();

        let old_file = sent.join("traces-1-0.otlp");
        std::fs::write(&old_file, b"x").unwrap();
        // A 0-second cutoff means "keep nothing" — everything is at least 0s old.
        prune_by_age(&sent, 0).unwrap();

        assert!(!old_file.exists());
    }

    #[test]
    fn prune_by_age_removes_old_files_in_pending() {
        // Retention must apply to pending/ too — it's the ONLY spool location
        // for a Canonical/packaged install, which never ships to sent/.
        let dir = TempDir::new().unwrap();
        let base = dir.path().to_path_buf();
        let p = write_pending(&base, "traces", b"x").unwrap();
        let pending = pending_dir(&base);

        prune_by_age(&pending, 0).unwrap();

        assert!(!p.exists());
    }

    #[test]
    fn prune_by_age_keeps_fresh_files() {
        let dir = TempDir::new().unwrap();
        let base = dir.path().to_path_buf();
        let p = write_pending(&base, "traces", b"x").unwrap();
        let pending = pending_dir(&base);

        // A generous cutoff (1 day) must not touch a file written moments ago.
        prune_by_age(&pending, 24 * 3600).unwrap();

        assert!(p.exists());
    }

    #[test]
    fn pending_cap_drops_oldest_first_with_warn() {
        // Set cap to 1 byte so everything over 1 byte gets dropped
        std::env::set_var("MERIDIAN_TELEMETRY_MAX_PENDING_MB", "0");

        let dir = TempDir::new().unwrap();
        let base = dir.path().to_path_buf();
        write_pending(&base, "traces", b"aaa").unwrap();
        write_pending(&base, "traces", b"bbb").unwrap();

        let pending = pending_dir(&base);
        // Cap of 0 MB → all files should be dropped
        enforce_pending_cap(&pending).unwrap();

        let remaining: Vec<_> = std::fs::read_dir(&pending)
            .unwrap()
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "otlp"))
            .collect();
        // With 0MB cap all are dropped
        assert!(remaining.is_empty());

        std::env::remove_var("MERIDIAN_TELEMETRY_MAX_PENDING_MB");
    }

    /// `quarantine/` was pruned by nothing and read by nothing - not by
    /// retention, not by `build_export_bundle`, not by `meridian logs`. A
    /// permanently-rejected payload was immortal AND invisible, the one
    /// genuinely unbounded directory in the spool.
    #[test]
    fn housekeeping_prunes_quarantine_too() {
        let dir = TempDir::new().unwrap();
        let pending = pending_dir(dir.path());
        std::fs::create_dir_all(&pending).unwrap();
        let sent = dir.path().join("sent");
        std::fs::create_dir_all(&sent).unwrap();
        let quarantine = dir.path().join("quarantine");
        std::fs::create_dir_all(&quarantine).unwrap();
        let q = quarantine.join("traces-1-0.otlp");
        std::fs::write(&q, b"rejected").unwrap();

        run_housekeeping(&pending, &sent, &quarantine, 0, false).unwrap();
        assert!(q.exists(), "not-due housekeeping must not prune quarantine");

        run_housekeeping(&pending, &sent, &quarantine, 0, true).unwrap();
        assert!(
            !q.exists(),
            "a quarantined payload must age out - it was previously immortal"
        );
    }

    /// `sent/` had an age policy and NO volume policy, so its size was whatever
    /// the emission rate produced inside the 7-day window (~800 MB / ~150k
    /// files measured). The cap evicts oldest-first so the newest, most useful
    /// records survive.
    #[test]
    fn the_sent_cap_evicts_oldest_first() {
        let dir = TempDir::new().unwrap();
        let sent = dir.path().join("sent");
        std::fs::create_dir_all(&sent).unwrap();
        // 3 files, ~4KB each, against a cap of 0MB -> everything over the cap
        // goes, oldest first.
        let oldest = sent.join("traces-100-0.otlp");
        let middle = sent.join("traces-200-0.otlp");
        let newest = sent.join("traces-300-0.otlp");
        for p in [&oldest, &middle, &newest] {
            std::fs::write(p, vec![b'x'; 4096]).unwrap();
        }

        std::env::set_var("MERIDIAN_TELEMETRY_MAX_SENT_MB", "0");
        enforce_sent_cap(&sent).unwrap();
        std::env::remove_var("MERIDIAN_TELEMETRY_MAX_SENT_MB");

        assert!(!oldest.exists(), "the oldest shipped file must go first");
        assert!(
            !middle.exists() || !newest.exists(),
            "the cap must actually reclaim space"
        );
    }

    /// A `sent/` under its ceiling must be left completely alone - this is an
    /// archive `meridian logs` reads, not scratch.
    #[test]
    fn the_sent_cap_keeps_everything_under_the_ceiling() {
        let dir = TempDir::new().unwrap();
        let sent = dir.path().join("sent");
        std::fs::create_dir_all(&sent).unwrap();
        let f = sent.join("traces-100-0.otlp");
        std::fs::write(&f, b"small").unwrap();

        enforce_sent_cap(&sent).unwrap();
        assert!(f.exists(), "a spool under its cap must not be pruned");
    }

    /// The ceiling has to leave room for the log history it backs; a tiny cap
    /// would silently turn `meridian logs` into a thirty-second window.
    #[test]
    fn the_sent_ceiling_leaves_room_for_real_log_history() {
        let bytes = max_sent_bytes();
        assert!(
            bytes >= 64 * 1024 * 1024,
            "sent/ backs `meridian logs`; {bytes} bytes is too small to be useful"
        );
    }
}
