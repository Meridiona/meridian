//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! User corrections to the AI-inferred day-tasks (timeline workstreams): DISMISS a
//! task, or MERGE one task into another (migration 074).
//!
//! # The problem this solves
//! `day_tasks` is derived, roll-forward data the worklog fold rewrites every hour
//! (`src/worklog_pipeline/task_db.rs::replace_day_tasks` UPSERTs on
//! `(day_local, task_id)` clobbering `status`, and PRUNES any task_id the fold no
//! longer names; the model re-derives the grouping from scratch each hour). So a
//! `status = 'dismissed'` flag or a hard DELETE on `day_tasks` would be undone on
//! the very next fold - the workstream would come back, or a merge would re-split.
//!
//! # How a correction survives re-folding
//! The correction is recorded here (durable, fold never touches this table), and
//! [`reconcile`] RE-MATERIALIZES it into `day_tasks`:
//! - a dismissed task is stamped `status = 'dismissed'` (the reader filters it out);
//! - a merged task's segments / minutes / summary / hours are folded into its target
//!   and the merged row is deleted (mirroring `worklogs::rematch_worklog`'s
//!   merge-and-redirect).
//!
//! [`reconcile`] is idempotent and self-healing, and is called from BOTH sides:
//! - every command here calls it, so the correction takes effect instantly;
//! - the fold calls it after `replace_day_tasks`, so if the model resurrected a
//!   dismissed task or re-split a merge, the next fold re-applies the correction.
//!
//! # Who calls this
//! - The tray `dismiss_day_task` / `merge_day_task` / `restore_day_task` commands
//!   -> the timeline task-detail panel.
//! - `src/worklog_pipeline/workstream.rs` (the fold) -> [`reconcile`] after each write.
//!
//! # Related
//! - [`crate::day_tasks`] - the reader that filters dismissed tasks out.

use crate::SqlitePool;
use anyhow::{bail, Context, Result};
use serde::Deserialize;
use sqlx::{FromRow, Row};
use std::collections::{HashMap, HashSet};
use tracing::Instrument;

const DISMISSED: &str = "dismissed";
const MERGED: &str = "merged";

/// One recorded correction row.
#[derive(Debug, Clone, FromRow)]
struct Correction {
    task_id: String,
    action: String,
    merged_into: Option<String>,
}

/// One `{start,end}` clock segment as stored in `day_tasks.segments_json`.
#[derive(Debug, Clone, Deserialize)]
struct Seg {
    start: String,
    end: String,
}

/// Dismiss a day-task: record the intent and hide it immediately.
#[tracing::instrument(skip(pool))]
pub async fn dismiss_day_task(
    pool: &SqlitePool,
    day: &str,
    task_id: &str,
    now: &str,
) -> Result<()> {
    upsert_correction(pool, day, task_id, DISMISSED, None, now).await?;
    reconcile(pool, day, now).await?;
    tracing::info!(day, task_id, "day-task: dismissed");
    Ok(())
}

/// Merge `task_id` into `into_task_id`: record the intent and combine immediately.
///
/// Rejects a self-merge and a merge into a task that is itself dismissed or already
/// merged away (the target must be a live task the work can actually land on).
#[tracing::instrument(skip(pool))]
pub async fn merge_day_task(
    pool: &SqlitePool,
    day: &str,
    task_id: &str,
    into_task_id: &str,
    now: &str,
) -> Result<()> {
    if task_id == into_task_id {
        bail!("a task can't be merged into itself");
    }
    // The target must be a live, non-corrected task - merging into a dismissed or
    // merged-away task would send the work nowhere.
    let existing = load_corrections(pool, day).await;
    if let Some(c) = existing.iter().find(|c| c.task_id == into_task_id) {
        bail!(
            "can't merge into {into_task_id} - it was already {}",
            c.action
        );
    }
    upsert_correction(pool, day, task_id, MERGED, Some(into_task_id), now).await?;
    reconcile(pool, day, now).await?;
    tracing::info!(day, task_id, into_task_id, "day-task: merged");
    Ok(())
}

/// Clear a correction (undo a dismiss).
///
/// A dismiss is fully reversible: dropping the row un-hides the task on the next
/// read (as long as the fold hasn't pruned it). A MERGE is only reversible going
/// FORWARD - the segments were already folded into the target and can't be cleanly
/// un-combined - so clearing a merge just stops it re-applying; it does not restore
/// the merged-away task. Callers that only expose "un-dismiss" should gate on the
/// correction being a dismiss.
#[tracing::instrument(skip(pool))]
pub async fn restore_day_task(
    pool: &SqlitePool,
    day: &str,
    task_id: &str,
    now: &str,
) -> Result<()> {
    let res = sqlx::query("DELETE FROM day_task_corrections WHERE day_local = ? AND task_id = ?")
        .bind(day)
        .bind(task_id)
        .execute(pool)
        .instrument(tracing::debug_span!("day_task_corrections.write.clear"))
        .await
        .context("clearing a day-task correction")?;
    // Un-stamp the dismissed status so the reader shows it again right away.
    sqlx::query(
        "UPDATE day_tasks SET status = 'active', updated_at = ? \
         WHERE day_local = ? AND task_id = ? AND status = 'dismissed'",
    )
    .bind(now)
    .bind(day)
    .bind(task_id)
    .execute(pool)
    .await
    .context("restoring a dismissed day-task's status")?;
    tracing::info!(
        day,
        task_id,
        cleared = res.rows_affected(),
        "day-task: restored"
    );
    Ok(())
}

/// The task_ids that should be hidden from the raw timeline read (dismissed OR
/// merged away). The reader filters `status = 'dismissed'`; this is the broader
/// set for any consumer that wants "tasks the user corrected away". Missing table
/// (pre-074 DB) degrades to empty.
pub async fn hidden_task_ids(pool: &SqlitePool, day: &str) -> HashSet<String> {
    load_corrections(pool, day)
        .await
        .into_iter()
        .map(|c| c.task_id)
        .collect()
}

/// Re-materialize every correction for `day` into `day_tasks`. Idempotent: a
/// dismissed task is stamped `status = 'dismissed'`; a merged task is folded into
/// its (chain-resolved) target and its row deleted. Safe to call after every fold
/// and after each command. Best-effort per correction - a skip (missing row, broken
/// chain) is logged, never fatal, so one bad correction can't stall the fold.
#[tracing::instrument(skip(pool))]
pub async fn reconcile(pool: &SqlitePool, day: &str, now: &str) -> Result<()> {
    let corrections = load_corrections(pool, day).await;
    if corrections.is_empty() {
        return Ok(());
    }

    // Resolve each merged task to its FINAL target (follow merged_into chains, with
    // a cycle guard). A task whose chain hits a dismiss or a cycle is left alone.
    let by_id: HashMap<&str, &Correction> = corrections
        .iter()
        .map(|c| (c.task_id.as_str(), c))
        .collect();

    for c in &corrections {
        match c.action.as_str() {
            DISMISSED => {
                if let Err(e) = sqlx::query(
                    "UPDATE day_tasks SET status = 'dismissed', updated_at = ? \
                     WHERE day_local = ? AND task_id = ?",
                )
                .bind(now)
                .bind(day)
                .bind(&c.task_id)
                .execute(pool)
                .await
                {
                    tracing::warn!(day, task_id = %c.task_id, error = %e, "day-task: dismiss re-stamp failed");
                }
            }
            MERGED => {
                let Some(target) = resolve_target(&by_id, &c.task_id) else {
                    tracing::warn!(day, task_id = %c.task_id, "day-task: merge target unresolved (cycle or dismissed) - skipping");
                    continue;
                };
                if let Err(e) = fold_into(pool, day, &c.task_id, &target, now).await {
                    tracing::warn!(day, task_id = %c.task_id, target, error = %e, "day-task: merge re-apply skipped");
                }
            }
            other => {
                tracing::warn!(day, task_id = %c.task_id, action = other, "day-task: unknown correction action - ignoring")
            }
        }
    }
    Ok(())
}

/// Follow `start`'s merge chain to a task that is not itself merged, guarding
/// against cycles and a target that was dismissed. Returns the final target id, or
/// `None` if the chain is broken (cycle, or resolves onto a dismissed task).
fn resolve_target(by_id: &HashMap<&str, &Correction>, start: &str) -> Option<String> {
    let mut seen = HashSet::new();
    let mut cur = by_id.get(start)?.merged_into.clone()?;
    loop {
        if !seen.insert(cur.clone()) {
            return None; // cycle
        }
        match by_id.get(cur.as_str()) {
            None => return Some(cur), // a live, uncorrected task - the real target
            Some(c) if c.action == MERGED => {
                cur = c.merged_into.clone()?;
            }
            Some(_) => return None, // target itself dismissed
        }
    }
}

/// Fold `src`'s segments/minutes/summary/hours into `target` in `day_tasks`, then
/// delete `src`. No-op if either row is absent (already merged/pruned).
async fn fold_into(pool: &SqlitePool, day: &str, src: &str, target: &str, now: &str) -> Result<()> {
    let mut tx = pool.begin().await.context("begin day-task merge tx")?;

    let src_row = read_row(&mut tx, day, src).await?;
    let tgt_row = read_row(&mut tx, day, target).await?;
    let (Some(src_row), Some(tgt_row)) = (src_row, tgt_row) else {
        // Nothing to combine (the model hasn't re-created the source, or the target
        // was pruned). The correction stays recorded for when both exist again.
        tx.rollback().await.ok();
        return Ok(());
    };

    // Union the segments, recompute minutes + hours, append the summary lines.
    let mut segs = parse_segs(&src_row.segments_json);
    segs.extend(parse_segs(&tgt_row.segments_json));
    let segs = union(segs);
    let minutes = union_minutes(&segs);
    let hours = hours_touched(&segs, day);
    let summary = join_summaries(&tgt_row.summary, &src_row.summary);

    sqlx::query(
        "UPDATE day_tasks SET segments_json = ?, minutes = ?, hours_json = ?, summary = ?, \
                updated_at = ? WHERE day_local = ? AND task_id = ?",
    )
    .bind(segs_to_json(&segs))
    .bind(minutes)
    .bind(serde_json::to_string(&hours).unwrap_or_else(|_| "[]".to_string()))
    .bind(summary)
    .bind(now)
    .bind(day)
    .bind(target)
    .execute(&mut *tx)
    .await
    .context("writing the merged target task")?;

    sqlx::query("DELETE FROM day_tasks WHERE day_local = ? AND task_id = ?")
        .bind(day)
        .bind(src)
        .execute(&mut *tx)
        .await
        .context("deleting the merged-away task")?;

    tx.commit().await.context("commit day-task merge tx")?;
    Ok(())
}

struct RawRow {
    segments_json: String,
    summary: String,
}

async fn read_row(
    tx: &mut sqlx::SqliteConnection,
    day: &str,
    task_id: &str,
) -> Result<Option<RawRow>> {
    let row = sqlx::query(
        "SELECT COALESCE(segments_json, '[]') AS segments_json, COALESCE(summary, '') AS summary \
         FROM day_tasks WHERE day_local = ? AND task_id = ?",
    )
    .bind(day)
    .bind(task_id)
    .fetch_optional(&mut *tx)
    .await
    .context("reading a day-task row for merge")?;
    Ok(row.map(|r| RawRow {
        segments_json: r.get("segments_json"),
        summary: r.get("summary"),
    }))
}

// ── Persistence helpers ───────────────────────────────────────────────────────

async fn upsert_correction(
    pool: &SqlitePool,
    day: &str,
    task_id: &str,
    action: &str,
    merged_into: Option<&str>,
    now: &str,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO day_task_corrections (day_local, task_id, action, merged_into, created_at) \
         VALUES (?, ?, ?, ?, ?) \
         ON CONFLICT(day_local, task_id) DO UPDATE SET \
            action = excluded.action, merged_into = excluded.merged_into, \
            created_at = excluded.created_at",
    )
    .bind(day)
    .bind(task_id)
    .bind(action)
    .bind(merged_into)
    .bind(now)
    .execute(pool)
    .instrument(tracing::debug_span!("day_task_corrections.write.upsert"))
    .await
    .context("recording a day-task correction")?;
    Ok(())
}

/// Every correction for the day. Missing table (pre-074 DB) degrades to empty.
async fn load_corrections(pool: &SqlitePool, day: &str) -> Vec<Correction> {
    let res = sqlx::query_as::<_, Correction>(
        "SELECT task_id, action, merged_into FROM day_task_corrections WHERE day_local = ?",
    )
    .bind(day)
    .fetch_all(pool)
    .instrument(tracing::debug_span!("day_task_corrections.read"))
    .await;
    match res {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(error = %e, "day_task_corrections: read skipped (pre-074 DB?)");
            Vec::new()
        }
    }
}

// ── Clock-segment math (self-contained: DaySegment is HH:MM clock strings) ─────

fn parse_hhmm(s: &str) -> Option<i64> {
    let (h, m) = s.split_once(':')?;
    let h: i64 = h.trim().parse().ok()?;
    let m: i64 = m.trim().parse().ok()?;
    if (0..=23).contains(&h) && (0..=59).contains(&m) {
        Some(h * 60 + m)
    } else {
        None
    }
}

fn fmt_hhmm(min: i64) -> String {
    format!("{:02}:{:02}", (min / 60).clamp(0, 23), min % 60)
}

fn parse_segs(json: &str) -> Vec<(i64, i64)> {
    serde_json::from_str::<Vec<Seg>>(json)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|s| {
            let a = parse_hhmm(&s.start)?;
            let b = parse_hhmm(&s.end)?;
            (b > a).then_some((a, b))
        })
        .collect()
}

/// Sort by start, merge overlapping/touching spans into a disjoint ascending set.
fn union(mut segs: Vec<(i64, i64)>) -> Vec<(i64, i64)> {
    segs.sort_by_key(|s| s.0);
    let mut out: Vec<(i64, i64)> = Vec::with_capacity(segs.len());
    for (a, b) in segs {
        match out.last_mut() {
            Some(prev) if a <= prev.1 => prev.1 = prev.1.max(b),
            _ => out.push((a, b)),
        }
    }
    out
}

fn union_minutes(segs: &[(i64, i64)]) -> i64 {
    segs.iter().map(|(a, b)| b - a).sum()
}

fn hours_touched(segs: &[(i64, i64)], day: &str) -> Vec<String> {
    let mut hours: Vec<String> = Vec::new();
    for (a, b) in segs {
        // A span [a,b) touches every clock hour from floor(a/60) to floor((b-1)/60).
        for h in (a / 60)..=((b - 1).max(*a) / 60) {
            let label = format!("{day}T{:02}", h.clamp(0, 23));
            if !hours.contains(&label) {
                hours.push(label);
            }
        }
    }
    hours.sort();
    hours
}

fn segs_to_json(segs: &[(i64, i64)]) -> String {
    let arr: Vec<serde_json::Value> = segs
        .iter()
        .map(|(a, b)| serde_json::json!({ "start": fmt_hhmm(*a), "end": fmt_hhmm(*b) }))
        .collect();
    serde_json::to_string(&arr).unwrap_or_else(|_| "[]".to_string())
}

/// Target's summary first, then the source's - newline-joined, blank lines and
/// exact-duplicate lines dropped so a merge doesn't repeat a bullet.
fn join_summaries(target: &str, src: &str) -> String {
    let mut seen = HashSet::new();
    let mut out: Vec<&str> = Vec::new();
    for line in target.lines().chain(src.lines()) {
        let t = line.trim();
        if !t.is_empty() && seen.insert(t.to_string()) {
            out.push(t);
        }
    }
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;

    async fn seeded() -> SqlitePool {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE day_tasks (day_local TEXT NOT NULL, task_id TEXT NOT NULL, \
                title TEXT, summary TEXT, hours_json TEXT, segments_json TEXT, minutes INTEGER, \
                status TEXT DEFAULT 'active', linked_ticket TEXT, created_at TEXT, updated_at TEXT, \
                PRIMARY KEY (day_local, task_id))",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "CREATE TABLE day_task_corrections (day_local TEXT NOT NULL, task_id TEXT NOT NULL, \
                action TEXT NOT NULL, merged_into TEXT, created_at TEXT NOT NULL, \
                PRIMARY KEY (day_local, task_id))",
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    async fn add_task(pool: &SqlitePool, id: &str, summary: &str, segs: &str, minutes: i64) {
        sqlx::query(
            "INSERT INTO day_tasks (day_local, task_id, title, summary, hours_json, segments_json, \
                minutes, status, created_at, updated_at) \
             VALUES ('2026-07-22', ?, ?, ?, '[]', ?, ?, 'active', 't0', 't0')",
        )
        .bind(id)
        .bind(id)
        .bind(summary)
        .bind(segs)
        .bind(minutes)
        .execute(pool)
        .await
        .unwrap();
    }

    async fn status_of(pool: &SqlitePool, id: &str) -> Option<String> {
        sqlx::query_scalar(
            "SELECT status FROM day_tasks WHERE day_local='2026-07-22' AND task_id=?",
        )
        .bind(id)
        .fetch_optional(pool)
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn dismiss_hides_the_task_and_survives_a_refold() {
        let pool = seeded().await;
        add_task(&pool, "T1", "a", "[]", 10).await;

        dismiss_day_task(&pool, "2026-07-22", "T1", "t1")
            .await
            .unwrap();
        assert_eq!(status_of(&pool, "T1").await.as_deref(), Some("dismissed"));

        // Simulate the fold rewriting the row back to 'active' (status clobbered).
        sqlx::query("UPDATE day_tasks SET status='active' WHERE task_id='T1'")
            .execute(&pool)
            .await
            .unwrap();
        // A reconcile (what the fold calls afterward) re-hides it.
        reconcile(&pool, "2026-07-22", "t2").await.unwrap();
        assert_eq!(
            status_of(&pool, "T1").await.as_deref(),
            Some("dismissed"),
            "the dismiss is re-applied after a refold"
        );
    }

    #[tokio::test]
    async fn merge_folds_segments_and_minutes_and_deletes_the_source() {
        let pool = seeded().await;
        add_task(
            &pool,
            "T1",
            "kept",
            r#"[{"start":"09:00","end":"09:30"}]"#,
            30,
        )
        .await;
        add_task(
            &pool,
            "T2",
            "moved",
            r#"[{"start":"10:00","end":"10:45"}]"#,
            45,
        )
        .await;

        merge_day_task(&pool, "2026-07-22", "T2", "T1", "t1")
            .await
            .unwrap();

        // T2 is gone; T1 carries both segments, the summed minutes, and both summaries.
        assert!(
            status_of(&pool, "T2").await.is_none(),
            "the merged task is deleted"
        );
        let (segs, minutes, summary): (String, i64, String) = sqlx::query_as(
            "SELECT segments_json, minutes, summary FROM day_tasks WHERE task_id='T1'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(minutes, 75, "30 + 45 disjoint minutes");
        assert!(
            segs.contains("09:00") && segs.contains("10:00"),
            "both spans present"
        );
        assert!(
            summary.contains("kept") && summary.contains("moved"),
            "summaries joined"
        );
    }

    #[tokio::test]
    async fn overlapping_merged_segments_count_once() {
        let pool = seeded().await;
        add_task(&pool, "T1", "a", r#"[{"start":"09:00","end":"09:40"}]"#, 40).await;
        add_task(&pool, "T2", "b", r#"[{"start":"09:30","end":"10:00"}]"#, 30).await;

        merge_day_task(&pool, "2026-07-22", "T2", "T1", "t1")
            .await
            .unwrap();
        let minutes: i64 = sqlx::query_scalar("SELECT minutes FROM day_tasks WHERE task_id='T1'")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(minutes, 60, "09:00-10:00 union, the overlap counted once");
    }

    #[tokio::test]
    async fn a_merge_chain_resolves_to_the_final_target() {
        let pool = seeded().await;
        add_task(
            &pool,
            "T1",
            "final",
            r#"[{"start":"09:00","end":"09:10"}]"#,
            10,
        )
        .await;
        add_task(
            &pool,
            "T2",
            "mid",
            r#"[{"start":"10:00","end":"10:10"}]"#,
            10,
        )
        .await;
        add_task(
            &pool,
            "T3",
            "leaf",
            r#"[{"start":"11:00","end":"11:10"}]"#,
            10,
        )
        .await;

        merge_day_task(&pool, "2026-07-22", "T2", "T1", "t1")
            .await
            .unwrap();
        // T2 is now gone; recording T3 -> T2 must resolve through to T1.
        upsert_correction(&pool, "2026-07-22", "T3", MERGED, Some("T2"), "t2")
            .await
            .unwrap();
        reconcile(&pool, "2026-07-22", "t3").await.unwrap();

        assert!(status_of(&pool, "T3").await.is_none(), "T3 folded away");
        let minutes: i64 = sqlx::query_scalar("SELECT minutes FROM day_tasks WHERE task_id='T1'")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(minutes, 30, "all three tasks' minutes landed on T1");
    }

    #[tokio::test]
    async fn merge_into_a_dismissed_task_is_rejected() {
        let pool = seeded().await;
        add_task(&pool, "T1", "a", "[]", 10).await;
        add_task(&pool, "T2", "b", "[]", 10).await;
        dismiss_day_task(&pool, "2026-07-22", "T1", "t1")
            .await
            .unwrap();

        assert!(
            merge_day_task(&pool, "2026-07-22", "T2", "T1", "t2")
                .await
                .is_err(),
            "can't merge into a dismissed task"
        );
        assert!(
            merge_day_task(&pool, "2026-07-22", "T1", "T1", "t2")
                .await
                .is_err(),
            "no self-merge"
        );
    }

    #[tokio::test]
    async fn restore_un_dismisses() {
        let pool = seeded().await;
        add_task(&pool, "T1", "a", "[]", 10).await;
        dismiss_day_task(&pool, "2026-07-22", "T1", "t1")
            .await
            .unwrap();
        assert_eq!(status_of(&pool, "T1").await.as_deref(), Some("dismissed"));

        restore_day_task(&pool, "2026-07-22", "T1", "t2")
            .await
            .unwrap();
        assert_eq!(status_of(&pool, "T1").await.as_deref(), Some("active"));
        // The correction row is gone, so a later reconcile leaves it visible.
        reconcile(&pool, "2026-07-22", "t3").await.unwrap();
        assert_eq!(status_of(&pool, "T1").await.as_deref(), Some("active"));
    }

    #[tokio::test]
    async fn reconcile_is_a_noop_with_no_corrections() {
        let pool = seeded().await;
        add_task(&pool, "T1", "a", "[]", 10).await;
        reconcile(&pool, "2026-07-22", "t1").await.unwrap();
        assert_eq!(status_of(&pool, "T1").await.as_deref(), Some("active"));
    }
}
