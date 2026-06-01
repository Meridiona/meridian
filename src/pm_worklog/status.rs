// meridian — normalises screenpipe activity into structured app sessions
//
// `meridian worklog-status` — a human-readable PM-worklog report, no SQL needed.
// For a day it prints: the hour ledger (done / pending / stuck), worklogs by
// state, and a per-ticket table with the synthesised Jira comment. Reads
// pm_worklog_hours (driver progress) + pm_worklogs (output rows).

use anyhow::{Context, Result};
use chrono::{DateTime, Local};
use sqlx::{Row, SqlitePool};

use super::config::PmWorklogConfig;
use super::models::JiraUpdate;

/// Aging window (minutes) past hour_end after which a still-`pending` hour is
/// considered genuinely stuck rather than merely settling. Mirrors the driver's
/// default `PM_WORKLOG_READINESS_AGING_MIN`.
const STUCK_AFTER_MIN: f64 = 90.0;

/// Render `window_start` (`...+00:00`) as a local `HH:MM`, or the raw string if
/// it can't be parsed.
fn local_hhmm(iso: &str) -> String {
    match DateTime::parse_from_rfc3339(iso) {
        Ok(dt) => dt.with_timezone(&Local).format("%H:%M").to_string(),
        Err(_) => iso.chars().take(5).collect(),
    }
}

fn fmt_secs(s: i64) -> String {
    let m = s / 60;
    if m >= 60 {
        format!("{}h{:02}m", m / 60, m % 60)
    } else {
        format!("{m}m")
    }
}

/// One-shot CLI: `meridian worklog-status [--day YYYY-MM-DD]`.
/// `day` defaults to the local current date (matches how rows are stamped for
/// daytime hours; late-night hours may fall under the previous UTC date).
pub async fn cli_status(pool: &SqlitePool, day: Option<&str>) {
    let day_utc = match day {
        Some(d) => d.to_string(),
        None => Local::now().format("%Y-%m-%d").to_string(),
    };
    if let Err(e) = print_status(pool, &day_utc).await {
        eprintln!("worklog-status: {e:#}");
    }
}

async fn print_status(pool: &SqlitePool, day_utc: &str) -> Result<()> {
    let cfg = PmWorklogConfig::from_env();

    // ── Hour ledger ────────────────────────────────────────────────────────
    let hour_rows = sqlx::query("SELECT status, hour_end FROM pm_worklog_hours WHERE day_utc = ?")
        .bind(day_utc)
        .fetch_all(pool)
        .await
        .context("read pm_worklog_hours")?;

    let now = Local::now();
    let (mut done, mut pending, mut stuck) = (0u32, 0u32, 0u32);
    for r in &hour_rows {
        let status: String = r.get("status");
        if status == "done" {
            done += 1;
            continue;
        }
        pending += 1;
        let hour_end: String = r.get("hour_end");
        if let Ok(he) = DateTime::parse_from_rfc3339(&hour_end) {
            let mins_over =
                (now.signed_duration_since(he.with_timezone(&Local))).num_minutes() as f64;
            if mins_over > STUCK_AFTER_MIN {
                stuck += 1;
            }
        }
    }

    // ── Worklog rows ───────────────────────────────────────────────────────
    let wl_rows = sqlx::query(
        "SELECT task_key, window_start, state, confidence, time_spent_seconds, \
                posted_worklog_id, payload_json \
         FROM pm_worklogs WHERE day_utc = ? ORDER BY window_start, task_key",
    )
    .bind(day_utc)
    .fetch_all(pool)
    .await
    .context("read pm_worklogs")?;

    let (mut drafted, mut posted, mut skipped, mut failed) = (0u32, 0u32, 0u32, 0u32);
    let mut total_secs: i64 = 0;
    struct Line {
        hour: String,
        task: String,
        state: String,
        conf: f64,
        secs: i64,
        comment: String,
        flags: Vec<String>,
    }
    let mut lines: Vec<Line> = Vec::with_capacity(wl_rows.len());
    for r in &wl_rows {
        let state: String = r.get("state");
        match state.as_str() {
            "posted" => posted += 1,
            "drafted" => drafted += 1,
            "skipped" => skipped += 1,
            "failed" => failed += 1,
            _ => {}
        }
        let secs: i64 = r.try_get("time_spent_seconds").unwrap_or(0);
        total_secs += secs;
        let payload: String = r.get("payload_json");
        let (comment, flags) = match serde_json::from_str::<JiraUpdate>(&payload) {
            Ok(u) => (u.summary, u.risk_flags),
            Err(_) => ("(unparseable payload)".to_string(), vec![]),
        };
        lines.push(Line {
            hour: local_hhmm(&r.get::<String, _>("window_start")),
            task: r.get("task_key"),
            state,
            conf: r.try_get("confidence").unwrap_or(0.0),
            secs,
            comment,
            flags,
        });
    }

    // ── Header ─────────────────────────────────────────────────────────────
    let posting = if cfg.post_enabled {
        "ON (posts to Jira)"
    } else {
        "OFF (dry-run — drafts only)"
    };
    println!();
    println!("  PM Worklog Status — {day_utc} (times shown in local tz)");
    println!("  ─────────────────────────────────────────────────────────────");
    let stuck_note = if stuck > 0 {
        format!("  ⚠ {stuck} STUCK (>{STUCK_AFTER_MIN:.0}m overdue)")
    } else {
        String::new()
    };
    println!("  Hours:    {done} done · {pending} pending{stuck_note}");
    println!(
        "  Worklogs: {drafted} drafted · {posted} posted · {skipped} skipped · {failed} failed   ({} total)",
        fmt_secs(total_secs)
    );
    println!("  Posting:  {posting}");

    if lines.is_empty() {
        println!();
        println!("  (no worklogs for this day)");
        return Ok(());
    }

    // ── Per-ticket table ───────────────────────────────────────────────────
    println!();
    println!(
        "  {:<6}  {:<9}  {:<8}  {:>4}  {:>6}  comment",
        "hour", "ticket", "state", "conf", "time"
    );
    for l in &lines {
        let flag = if l.flags.is_empty() { "" } else { " ⚑" };
        let comment: String = l.comment.chars().take(64).collect();
        println!(
            "  {:<6}  {:<9}  {:<8}  {:>4.2}  {:>6}  {comment}{flag}",
            l.hour,
            l.task,
            l.state,
            l.conf,
            fmt_secs(l.secs)
        );
    }

    // ── Flagged (weak/leaky) worklogs ──────────────────────────────────────
    let flagged: Vec<&Line> = lines
        .iter()
        .filter(|l| l.conf < cfg.min_confidence || !l.flags.is_empty())
        .collect();
    if !flagged.is_empty() {
        println!();
        println!(
            "  ⚑ {} worklog(s) below conf {:.2} or with risk flags — inspect before posting:",
            flagged.len(),
            cfg.min_confidence
        );
        for l in flagged {
            let fl = if l.flags.is_empty() {
                "low_confidence".to_string()
            } else {
                l.flags.join(",")
            };
            println!("      {} {} (conf {:.2}) — {}", l.hour, l.task, l.conf, fl);
        }
    }

    println!();
    Ok(())
}
