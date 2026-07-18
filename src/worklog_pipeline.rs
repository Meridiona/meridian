//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Hour-level worklog driver — clock-aligned trigger.
//
// Fires ONE `/worklog_hour` call per completed local clock hour, 3 minutes after
// the hour ends (HH:03 local: 11:03 processes 10:00–11:00, 12:03 processes
// 11:00–12:00, …). The 3-minute offset lets ETL settle the hour's last frames and
// gives the coding-agent indexer's hour-boundary force-seal + the summariser a head
// start, so the hour's coding sessions are normally already `summarised` by the time
// we run.
//
// An hour is processed only when it has real activity — MORE than 5 sessions over
// 15s, OR at least one coding-agent session (a single long coding session always
// qualifies). Before firing we WAIT for any coding row overlapping the hour that is
// still `coding_agent_live` / `pending_summariser` to reach a terminal state (capped
// so a stuck row can't starve the next tick).
//
// No backfill: only TODAY's local hours are ever considered. A hour missed while the
// daemon was down is caught up on the next startup / tick (today only). The
// `pm_worklog_hours` ledger (`ensure_hour` / `hour_is_done` / `mark_hour_done`) gives
// idempotency + catch-up tracking. The driver NEVER posts to Jira — drafts land in
// pm_worklogs for the dashboard to approve; pm_worklog::run_post_loop posts approved
// rows (unchanged).

use std::time::Duration as StdDuration;

use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Local, NaiveDateTime, TimeZone, Timelike, Utc};
use sqlx::SqlitePool;
use tokio::sync::watch;
use tracing::Instrument;

use crate::pm_worklog::{ledger, PmWorklogConfig};

// `pub(crate)` where the LLM-Lab replay ([`crate::llm_experiment`]) reuses the request
// builders / parsers / DB fetchers to rebuild a past hour's exact model input.
pub(crate) mod distiller;
pub(crate) mod hour;
pub(crate) mod hour_db;
pub(crate) mod hour_input;
pub(crate) mod segment;
pub(crate) mod task_db;
pub(crate) mod workstream;
pub(crate) mod workstream_parse;
pub(crate) mod workstream_sanitize;
pub(crate) mod workstream_state;

/// Seconds past the top of the hour at which a completed hour is processed (HH:03).
const WAKE_OFFSET_SECS: i64 = 3 * 60;
/// A session must run longer than this to count toward the activity gate.
const MIN_SESSION_DURATION_S: i64 = 15;
/// The hour qualifies on session count only when it has strictly MORE than this many
/// sessions over `MIN_SESSION_DURATION_S` (coding sessions qualify it regardless).
const MIN_SESSIONS: i64 = 5;
/// Poll cadence while waiting for coding-agent summarisation to finish.
const CODING_POLL: StdDuration = StdDuration::from_secs(30);
/// Hard cap on the coding-summarisation wait, measured from the END OF THE HOUR (not
/// from when the daemon happened to start), so it can never bleed into the next HH:03
/// tick (~57 min away). The indexer's hour-boundary seal makes hitting this rare.
const CODING_MAX_WAIT: StdDuration = StdDuration::from_secs(20 * 60);
/// A sealed coding row shorter than this is too trivial to be worth blocking the fold
/// for: it carries essentially no content to summarise (e.g. a 0-3s Claude Code session
/// slice, common when the developer's OWN agent conversations get ingested), yet a
/// pending one would otherwise gate the whole hour for [`CODING_MAX_WAIT`]. Waiting on
/// it buys nothing — its (absent) summary would fold into the report as nothing. So the
/// readiness gate ignores trivial pending rows; a genuinely short-but-real session still
/// summarises on the indexer tick and lands in a later fold.
const MIN_CODING_READY_S: i64 = 20;

/// Canonical `+00:00` ISO bound (matches stored `started_at`).
fn iso_bound(dt: DateTime<Utc>) -> String {
    dt.format("%Y-%m-%dT%H:%M:%S+00:00").to_string()
}

/// The hour label the Python pipeline expects: `YYYY-MM-DDTHH` in local time.
/// Sessions are bucketed by the user's local clock hour, not UTC.
fn hour_label(dt: DateTime<Local>) -> String {
    dt.format("%Y-%m-%dT%H").to_string()
}

// ──────────────────────── Clock alignment ───────────────────────────────────────

/// Pure half of `next_worklog_wake`: seconds to sleep until the next HH:03 given how
/// far we already are into the current local hour. Always returns a strictly positive
/// delay (when already at/after :03 this hour, targets the next hour's :03).
fn wake_delay_secs(secs_past_hour: i64) -> u64 {
    let delta = WAKE_OFFSET_SECS - secs_past_hour;
    let delta = if delta <= 0 { delta + 3600 } else { delta };
    delta as u64
}

/// Duration until the next local HH:03:00 — the clock-aligned wake (mirrors
/// `coding_agent_session_ingest::indexer::next_tick`).
fn next_worklog_wake() -> StdDuration {
    let now = Local::now();
    let secs_past_hour = (now.minute() * 60 + now.second()) as i64;
    StdDuration::from_secs(wake_delay_secs(secs_past_hour))
}

// ──────────────────────── Activity gate + coding readiness ───────────────────────

/// SQL fragment: a session whose [started_at, ended_at] overlaps the local hour
/// [hs, he). This is a deliberate SUPERSET of the Python fold predicate
/// (`worklog_pipeline/db.py::fetch_coding_summaries`, which buckets by `started_at`
/// in-hour only): we also match a session that ENDED in the hour but started earlier.
/// That makes the coding-readiness wait conservative — it waits for any coding session
/// touching the hour to finish summarising, never firing the hour mid-summarisation —
/// at the cost of occasionally qualifying an hour whose only coding row Python will fold
/// into the *earlier* hour. Over-waiting is the safe direction; do not narrow this to a
/// strict `started_at`-only match without re-checking the readiness gate.
const OVERLAP_PRED: &str =
    "((started_at >= ? AND started_at < ?) OR (ended_at >= ? AND ended_at < ?))";

/// Count sessions that started in the hour and ran longer than `min_dur` seconds.
async fn count_sessions_over(pool: &SqlitePool, hs: &str, he: &str, min_dur: i64) -> Result<i64> {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM app_sessions \
         WHERE started_at >= ? AND started_at < ? AND duration_s > ?",
    )
    .bind(hs)
    .bind(he)
    .bind(min_dur)
    .fetch_one(pool)
    .await
    .context("count sessions over duration in hour")
}

/// Does the hour contain any coding-agent session (live or sealed)?
async fn hour_has_coding(pool: &SqlitePool, hs: &str, he: &str) -> Result<bool> {
    let n: i64 = sqlx::query_scalar(&format!(
        "SELECT COUNT(*) FROM app_sessions \
         WHERE coding_agent_session_uuid IS NOT NULL AND {OVERLAP_PRED}"
    ))
    .bind(hs)
    .bind(he)
    .bind(hs)
    .bind(he)
    .fetch_one(pool)
    .await
    .context("count coding sessions in hour")?;
    Ok(n > 0)
}

/// Count coding-agent rows overlapping the hour still moving through the pipeline
/// (`coding_agent_live` / `pending_summariser`). Terminal states (`summarised`,
/// `subprocess_error`) are NOT counted — so a dead-lettered row can never make us
/// wait forever.
async fn coding_in_flight(pool: &SqlitePool, hs: &str, he: &str) -> Result<i64> {
    // Wait for every LIVE coding row (still being written), but among sealed rows
    // awaiting summarisation only the non-trivial ones — a 0-3s slice carries nothing
    // to fold, so blocking the hour on it just wastes the CODING_MAX_WAIT budget.
    sqlx::query_scalar(&format!(
        "SELECT COUNT(*) FROM app_sessions \
         WHERE coding_agent_session_uuid IS NOT NULL \
           AND ( task_method = 'coding_agent_live' \
                 OR (task_method = 'pending_summariser' AND duration_s >= ?) ) \
           AND {OVERLAP_PRED}"
    ))
    .bind(MIN_CODING_READY_S)
    .bind(hs)
    .bind(he)
    .bind(hs)
    .bind(he)
    .fetch_one(pool)
    .await
    .context("count in-flight coding sessions in hour")
}

/// Outcome of waiting for the hour's coding sessions to finish summarising.
enum WaitOutcome {
    /// No coding row overlapping the hour is still in flight.
    Ready,
    /// The cap elapsed with rows still in flight — proceed best-effort.
    CapHit,
    /// Shutdown was signalled while waiting.
    Shutdown,
}

/// Wall-clock instant past which the coding wait gives up: `CODING_MAX_WAIT` after the
/// hour ended. `None` when `he` is not a parseable bound.
///
/// Anchored to the hour's end rather than to process start so the cap survives a
/// restart — see [`await_coding_ready`] for why that matters.
fn coding_deadline(he: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(he)
        .ok()
        .map(|end| end.with_timezone(&Utc) + Duration::seconds(CODING_MAX_WAIT.as_secs() as i64))
}

/// Poll until no coding row overlapping the hour is in flight, until `CODING_MAX_WAIT`
/// past the hour's END — a wall-clock deadline, not a per-process one.
///
/// The anchor is the whole point. `Instant::now() + CODING_MAX_WAIT` restarts the cap
/// from zero on every process start, so a daemon that restarts more often than
/// `CODING_MAX_WAIT` — a dev machine rebuilding on save, a crash loop, a run of updates
/// — never reaches the cap and the hour is never processed AT ALL. Anchoring to
/// `hour_end` makes the cap mean what `ledger.rs` already documents it to mean:
/// "process best-effort once the hour has been over longer than the aging window."
/// The wait now resumes across restarts, and an hour already past its cap (a backlog
/// after downtime) proceeds immediately instead of buying another full wait per attempt.
///
/// On a query error we return `Ready` (never block the worklog on a bad read); an
/// unparseable `he` takes the same escape rather than stalling the hour forever.
async fn await_coding_ready(
    pool: &SqlitePool,
    hs: &str,
    he: &str,
    shutdown_rx: &mut watch::Receiver<bool>,
) -> WaitOutcome {
    let Some(deadline) = coding_deadline(he) else {
        tracing::warn!(hour_end = %he, "worklog: unparseable hour end — proceeding");
        return WaitOutcome::Ready;
    };
    loop {
        match coding_in_flight(pool, hs, he).await {
            Ok(0) => return WaitOutcome::Ready,
            Ok(n) => tracing::debug!(in_flight = n, "worklog: waiting for coding summarisation"),
            Err(e) => {
                tracing::warn!(error = %e, "worklog: coding readiness query failed — proceeding");
                return WaitOutcome::Ready;
            }
        }
        if Utc::now() >= deadline {
            return WaitOutcome::CapHit;
        }
        tokio::select! {
            _ = shutdown_rx.changed() => return WaitOutcome::Shutdown,
            _ = tokio::time::sleep(CODING_POLL) => {}
        }
    }
}

// ──────────────────────── Firing one hour ────────────────────────────────────────

/// Process one completed hour: idempotency check → activity gate → wait for coding →
/// fire. Returns `true` if shutdown was observed (caller should stop).
///
/// `force` (the manual CLI path) bypasses the done-check and the activity gate so a
/// named hour always fires; it still waits for coding readiness.
#[allow(clippy::too_many_arguments)]
async fn process_hour(
    pool: &SqlitePool,
    cfg: &PmWorklogConfig,
    db_path: &str,
    day_local: &str,
    hs: &str,
    he: &str,
    label: &str,
    cycle_index: i64,
    force: bool,
    shutdown_rx: &mut watch::Receiver<bool>,
) -> bool {
    if let Err(e) = ledger::ensure_hour(pool, day_local, hs, he).await {
        tracing::warn!(hour = %label, error = %e, "worklog: ensure_hour failed — skipping");
        return false;
    }
    if !force {
        match ledger::hour_is_done(pool, hs).await {
            Ok(true) => return false,
            Ok(false) => {}
            Err(e) => {
                tracing::warn!(hour = %label, error = %e, "worklog: hour_is_done failed — skipping");
                return false;
            }
        }

        // Activity gate: > MIN_SESSIONS sessions over MIN_SESSION_DURATION_S, OR any
        // coding-agent session. A quiet hour is marked done so it is never re-scanned.
        // A query ERROR must NOT be treated as "quiet" — that would mark the hour done
        // on a transient read failure and permanently drop its worklog. Skip without
        // marking done so the hour stays pending and is retried next cycle (same posture
        // as the ensure_hour / hour_is_done / POST error paths).
        let count = match count_sessions_over(pool, hs, he, MIN_SESSION_DURATION_S).await {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(hour = %label, error = %e,
                    "worklog: session-count query failed — skipping (stays pending, will retry)");
                return false;
            }
        };
        let has_coding = match hour_has_coding(pool, hs, he).await {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!(hour = %label, error = %e,
                    "worklog: coding-check query failed — skipping (stays pending, will retry)");
                return false;
            }
        };
        if count <= MIN_SESSIONS && !has_coding {
            tracing::info!(
                hour = %label, sessions = count,
                "worklog: insufficient activity — skipping hour"
            );
            if let Err(e) = ledger::mark_hour_done(pool, hs, 0).await {
                tracing::warn!(hour = %label, error = %e, "worklog: mark_hour_done failed");
            }
            return false;
        }
    }

    match await_coding_ready(pool, hs, he, shutdown_rx).await {
        WaitOutcome::Shutdown => return true,
        WaitOutcome::CapHit => tracing::warn!(
            hour = %label,
            "worklog: coding summarisation not complete after cap — proceeding best-effort"
        ),
        WaitOutcome::Ready => {}
    }

    // Live "generating" signal for the timeline UI — flipped just before the call
    // fires, reverted on failure so a stuck/errored hour never looks perpetually
    // in-progress.
    if let Err(e) = ledger::mark_hour_generating(pool, hs).await {
        tracing::warn!(hour = %label, error = %e, "worklog: mark_hour_generating failed");
    }

    // Wrap the run in a span so `current_traceparent()` nests the hour's trace under
    // `worklog.hour` (one connected OpenObserve trace). The hour is orchestrated in Rust
    // now (`hour::run_hour`): distil locally, then summarise through the user's chosen AI.
    let span = tracing::info_span!("worklog.hour", hour = %label, cycle_index);
    let result = hour::run_hour(pool, cfg, db_path, label, hs, he)
        .instrument(span)
        .await;
    match result {
        Ok(()) => {
            if let Err(e) = ledger::mark_hour_done(pool, hs, 0).await {
                tracing::warn!(hour = %label, error = %e, "worklog: mark_hour_done failed");
            }
            tracing::info!(hour = %label, "worklog: hour processed");
        }
        Err(e) => {
            // Leave the hour pending so the next HH:03 tick (today catch-up) retries it.
            if let Err(e2) = ledger::mark_hour_pending(pool, hs).await {
                tracing::warn!(hour = %label, error = %e2, "worklog: mark_hour_pending failed");
            }
            tracing::warn!(hour = %label, error = %e, "worklog pipeline errored — hour left pending");
        }
    }
    false
}

/// Process every COMPLETED local hour of today not yet done. Walks chronologically
/// from local midnight; stops at the first hour that hasn't ended yet. Returns `true`
/// if shutdown was observed mid-walk. No yesterday / multi-day backfill.
async fn catch_up_today(
    pool: &SqlitePool,
    cfg: &PmWorklogConfig,
    db_path: &str,
    shutdown_rx: &mut watch::Receiver<bool>,
) -> bool {
    let now = Utc::now();
    let local_day = Local::now().date_naive();
    let day_local = local_day.format("%Y-%m-%d").to_string();

    let midnight_local = match local_day
        .and_hms_opt(0, 0, 0)
        .and_then(|naive| Local.from_local_datetime(&naive).single())
    {
        Some(m) => m,
        None => {
            tracing::warn!("worklog: ambiguous local midnight (DST) — skipping pass");
            return false;
        }
    };

    for i in 0..24i64 {
        let hs_local = midnight_local + Duration::hours(i);
        let he_local = hs_local + Duration::hours(1);
        let he_utc = he_local.with_timezone(&Utc);
        if he_utc > now {
            break; // hour not finished yet
        }
        let hs = iso_bound(hs_local.with_timezone(&Utc));
        let he = iso_bound(he_utc);
        let label = hour_label(hs_local);

        let shutdown = process_hour(
            pool,
            cfg,
            db_path,
            &day_local,
            &hs,
            &he,
            &label,
            i,
            false,
            shutdown_rx,
        )
        .await;
        if shutdown {
            return true;
        }
    }
    false
}

// ──────────────────────── Daemon loop ────────────────────────────────────────────

/// Daemon task: catch up today's hours on startup, then wake at every local HH:03 to
/// process the hour that just ended (plus any earlier today hour still pending — the
/// error-retry path). Drafts only; posting stays with pm_worklog::run_post_loop.
pub async fn run_loop(pool: SqlitePool, db_path: String, mut shutdown_rx: watch::Receiver<bool>) {
    let cfg = PmWorklogConfig::from_env();
    tracing::info!("worklog-pipeline driver starting (clock-aligned HH:03; drafts only)");

    // Startup catch-up — process any completed hour of today not yet done.
    if catch_up_today(&pool, &cfg, &db_path, &mut shutdown_rx).await {
        tracing::info!("worklog-pipeline driver stopped");
        return;
    }

    loop {
        let dur = next_worklog_wake();
        tokio::select! {
            _ = shutdown_rx.changed() => break,
            _ = tokio::time::sleep(dur) => {
                if catch_up_today(&pool, &cfg, &db_path, &mut shutdown_rx).await {
                    break;
                }
            }
        }
    }

    tracing::info!("worklog-pipeline driver stopped");
}

/// One-shot CLI: force-run the worklog pipeline for a single explicit local hour
/// (`YYYY-MM-DDTHH`). Bypasses the done-check + activity gate (so a named hour always
/// fires) but still waits for coding summarisation. For manual runs / testing.
pub async fn cli_run_hour(pool: &SqlitePool, db_path: &str, label: &str) {
    let cfg = PmWorklogConfig::from_env();
    let b = match hour_bounds(label) {
        Ok(b) => b,
        Err(e) => {
            // `:#` keeps the chrono parse detail behind the context line.
            eprintln!("worklog-hour: {e:#}");
            return;
        }
    };

    let (_tx, mut rx) = watch::channel(false);
    process_hour(
        pool,
        &cfg,
        db_path,
        &b.day_local,
        &b.hs,
        &b.he,
        label,
        b.cycle_index,
        true,
        &mut rx,
    )
    .await;
    println!("worklog-hour: processed {label}");
}

/// Everything an `YYYY-MM-DDTHH` local hour label resolves to: its UTC bounds
/// (`…+00:00`, the `pm_worklog_hours` key form), the local day, and the hour-of-day
/// cycle index. Extracted from [`cli_run_hour`] so the LLM-Lab replay
/// ([`crate::llm_experiment`]) resolves an hour identically.
pub(crate) struct HourBounds {
    pub hs: String,
    pub he: String,
    pub day_local: String,
    pub cycle_index: i64,
}

/// Resolve a local `YYYY-MM-DDTHH` label to [`HourBounds`]. Errors on a malformed label
/// or an hour that doesn't exist locally (DST transition).
pub(crate) fn hour_bounds(label: &str) -> Result<HourBounds> {
    let naive = NaiveDateTime::parse_from_str(&format!("{label}:00:00"), "%Y-%m-%dT%H:%M:%S")
        .with_context(|| format!("invalid hour label {label:?} (want YYYY-MM-DDTHH)"))?;
    let hs_local = Local
        .from_local_datetime(&naive)
        .single()
        .with_context(|| format!("ambiguous local hour {label:?} (DST transition)"))?;
    let he_local = hs_local + Duration::hours(1);
    Ok(HourBounds {
        hs: iso_bound(hs_local.with_timezone(&Utc)),
        he: iso_bound(he_local.with_timezone(&Utc)),
        day_local: hs_local.format("%Y-%m-%d").to_string(),
        cycle_index: hs_local.hour() as i64,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqliteConnectOptions;
    use std::str::FromStr;

    const HS: &str = "2026-05-30T05:00:00+00:00";
    const HE: &str = "2026-05-30T06:00:00+00:00";
    const IN_HOUR: &str = "2026-05-30T05:30:00+00:00";
    const IN_HOUR_END: &str = "2026-05-30T05:31:00+00:00";

    #[test]
    fn wake_delay_targets_next_hh03() {
        assert_eq!(wake_delay_secs(60), 120); // 11:01 → 11:03 (120s away)
        assert_eq!(wake_delay_secs(0), 180); // 11:00 → 11:03
        assert_eq!(wake_delay_secs(210), 3570); // 11:03:30 → next hour's :03
        assert_eq!(wake_delay_secs(180), 3600); // exactly :03 → next hour's :03
        assert_eq!(wake_delay_secs(3599), 181); // 11:59:59 → 12:03
    }

    /// The cap is 20 min past the hour's END, and is a pure function of the bound —
    /// NOT of when the process started. This is what lets the wait survive a restart:
    /// the same `he` yields the same deadline in every process, forever.
    #[test]
    fn coding_deadline_is_anchored_to_hour_end() {
        let he = "2026-07-17T13:00:00+00:00";
        let deadline = coding_deadline(he).expect("parseable bound");
        assert_eq!(iso_bound(deadline), "2026-07-17T13:20:00+00:00");
        // Stable across calls — no process-relative component.
        assert_eq!(coding_deadline(he), Some(deadline));
    }

    /// An hour long past its cap must be already-expired, so a restarted daemon
    /// proceeds best-effort immediately instead of buying a fresh 20-minute wait
    /// on every attempt (the bug: it then never generates at all).
    #[test]
    fn coding_deadline_for_a_stale_hour_is_in_the_past() {
        let long_ago = iso_bound(Utc::now() - Duration::hours(3));
        assert!(coding_deadline(&long_ago).expect("parseable bound") < Utc::now());
    }

    /// A bad bound must not yield a deadline — the caller escapes to `Ready` rather
    /// than stalling the hour forever.
    #[test]
    fn coding_deadline_rejects_an_unparseable_bound() {
        assert_eq!(coding_deadline("not-a-timestamp"), None);
    }

    async fn fresh_db() -> SqlitePool {
        let opts = SqliteConnectOptions::from_str("sqlite::memory:")
            .unwrap()
            .create_if_missing(true);
        let pool = SqlitePool::connect_with(opts).await.unwrap();
        sqlx::migrate!("src/migrations").run(&pool).await.unwrap();
        pool
    }

    #[allow(clippy::too_many_arguments)]
    async fn insert_session(
        pool: &SqlitePool,
        started_at: &str,
        ended_at: &str,
        duration_s: i64,
        coding_uuid: Option<&str>,
        task_method: Option<&str>,
    ) {
        sqlx::query(
            "INSERT INTO etl_runs (started_at, from_frame_id, to_frame_id, status) \
             VALUES ('t', 0, 0, 'success')",
        )
        .execute(pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO app_sessions ( \
                app_name, started_at, ended_at, duration_s, \
                window_titles, audio_snippets, signals, \
                min_frame_id, max_frame_id, frame_count, idle_frame_count, etl_run_id, \
                coding_agent_session_uuid, task_method \
             ) VALUES ('App', ?, ?, ?, '[]', '[]', '{}', 1, 1, 1, 0, \
                       (SELECT MAX(id) FROM etl_runs), ?, ?)",
        )
        .bind(started_at)
        .bind(ended_at)
        .bind(duration_s)
        .bind(coding_uuid)
        .bind(task_method)
        .execute(pool)
        .await
        .unwrap();
    }

    /// Six sessions over 15s qualify on count alone (no coding needed).
    #[tokio::test]
    async fn gate_count_qualifies() {
        let pool = fresh_db().await;
        for _ in 0..6 {
            insert_session(&pool, IN_HOUR, IN_HOUR_END, 20, None, None).await;
        }
        let count = count_sessions_over(&pool, HS, HE, MIN_SESSION_DURATION_S)
            .await
            .unwrap();
        assert_eq!(count, 6);
        assert!(count > MIN_SESSIONS);
        assert!(!hour_has_coding(&pool, HS, HE).await.unwrap());
    }

    /// Five sessions, no coding → does not qualify (skip path).
    #[tokio::test]
    async fn gate_five_sessions_skips() {
        let pool = fresh_db().await;
        for _ in 0..5 {
            insert_session(&pool, IN_HOUR, IN_HOUR_END, 20, None, None).await;
        }
        let count = count_sessions_over(&pool, HS, HE, MIN_SESSION_DURATION_S)
            .await
            .unwrap();
        let has_coding = hour_has_coding(&pool, HS, HE).await.unwrap();
        assert!(count <= MIN_SESSIONS && !has_coding); // → skip
    }

    /// Sub-15s sessions don't count toward the gate.
    #[tokio::test]
    async fn gate_short_sessions_ignored() {
        let pool = fresh_db().await;
        for _ in 0..10 {
            insert_session(&pool, IN_HOUR, IN_HOUR_END, 10, None, None).await;
        }
        assert_eq!(
            count_sessions_over(&pool, HS, HE, MIN_SESSION_DURATION_S)
                .await
                .unwrap(),
            0
        );
    }

    /// A single coding session qualifies the hour even with zero OCR sessions.
    #[tokio::test]
    async fn gate_single_coding_qualifies() {
        let pool = fresh_db().await;
        insert_session(
            &pool,
            IN_HOUR,
            IN_HOUR_END,
            3000,
            Some("uuid-1"),
            Some("summarised"),
        )
        .await;
        assert_eq!(
            count_sessions_over(&pool, HS, HE, MIN_SESSION_DURATION_S)
                .await
                .unwrap(),
            1
        );
        assert!(hour_has_coding(&pool, HS, HE).await.unwrap());
    }

    /// A `pending_summariser` coding row overlapping the hour is in flight (blocks).
    #[tokio::test]
    async fn coding_pending_in_flight() {
        let pool = fresh_db().await;
        insert_session(
            &pool,
            IN_HOUR,
            IN_HOUR_END,
            120,
            Some("uuid-1"),
            Some("pending_summariser"),
        )
        .await;
        assert_eq!(coding_in_flight(&pool, HS, HE).await.unwrap(), 1);
    }

    /// Terminal coding states (`summarised`, `subprocess_error`) are not in flight.
    #[tokio::test]
    async fn coding_terminal_not_in_flight() {
        let pool = fresh_db().await;
        insert_session(
            &pool,
            IN_HOUR,
            IN_HOUR_END,
            120,
            Some("uuid-1"),
            Some("summarised"),
        )
        .await;
        insert_session(
            &pool,
            IN_HOUR,
            IN_HOUR_END,
            120,
            Some("uuid-2"),
            Some("subprocess_error"),
        )
        .await;
        assert_eq!(coding_in_flight(&pool, HS, HE).await.unwrap(), 0);
    }
}
