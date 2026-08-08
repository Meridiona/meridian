//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! The daily-summary commands (tray side) — the AI-composed end-of-day review.
//!
//! # What this is
//! Three commands behind the timeline's "Daily summary" button:
//! - [`generate_day_summary`] — compose (or recompose) the day's summary. An LLM
//!   call, so it **shells out to the `meridian` CLI**: the chosen provider and its
//!   auth live daemon-side (`settings.json` / `~/.meridian/.env`), exactly as
//!   [`crate::commands::generate_day_task_worklog`] does.
//! - [`get_day_summary`] — read a stored summary on screen open. A plain DB read,
//!   so it is a **direct meridian-core call**, not a spawn: this runs on every open
//!   and a process launch per open would be felt.
//! - [`get_day_summary_data`] — the deterministic half of the screen, plus the
//!   staleness verdict.
//!
//! # Why the data is a separate command
//! The composed summary is a cached artefact; the day underneath it keeps moving.
//! Reading the live figures separately is what lets the screen show today's real
//! focus total beside prose written an hour ago, and it is where the staleness
//! check lives — `get_day_summary_data` already collects the evidence, so it can
//! answer "has this day moved on since the summary was written" for the price of one
//! extra indexed row rather than a second full collect.
//!
//! # Who calls this
//! Registered in `lib.rs`'s `invoke_handler!`; consumed by
//! `ui/components/summary/DaySummaryOverlay.tsx` via `ui/lib/bridge.ts`.
//!
//! # Related
//! - `src/day_summary/generate.rs` — the CLI-side engine [`generate_day_summary`]
//!   spawns, and the source of the JSON shape it parses.
//! - [`meridian_core::day_evidence`] — what [`get_day_summary_data`] returns.
//! - [`crate::commands::worklog_generate`] — the sibling CLI-spawning command, same
//!   spawn/[`crate::install::cli_cwd`]/timeout pattern.

use std::time::Duration;
use tauri::State;

use super::cli_exec::run_meridian_json;

/// Compose (or recompose) the day's summary. Spawns `meridian day-summary --day
/// <d>` (argv, no shell) with the same 150 s budget as its worklog sibling — one
/// LLM call, one budget.
///
/// This does not fail on a bad answer: the CLI keeps the deterministically computed
/// plan ledger and drops only the prose, so an `Err` here means the DB or the spawn,
/// never the model. The returned summary's `fallback` flag is what says the model
/// came up short.
#[tauri::command]
#[tracing::instrument]
pub async fn generate_day_summary(
    day: String,
) -> Result<meridian_core::day_summaries::DaySummary, String> {
    if day.is_empty() {
        return Err("day is required".to_string());
    }
    let summary: meridian_core::day_summaries::DaySummary = run_meridian_json(
        &["day-summary", "--day", &day],
        Duration::from_secs(150),
        "day-summary",
    )
    .await?;
    tracing::info!(
        %day,
        provider = %summary.provider,
        model = %summary.model,
        planned = summary.adherence.planned,
        achievement_pct = summary.adherence.achievement_pct,
        fallback = summary.fallback,
        "day-summary served"
    );
    Ok(summary)
}

/// The on-demand "Generate now" path: draft the day's worklogs FIRST, then compose
/// the summary — the same work the scheduled end-of-day pass does, run because the
/// user pressed the button rather than because the clock reached their chosen time.
/// Spawns `meridian day-summary --day <d> --now`.
///
/// A far longer budget than [`generate_day_summary`]: the `--now` flag drafts a
/// worklog per qualifying day-task, each its own LLM call, in sequence — minutes,
/// not the single call the bare compose is. The screen shows its "Composing…" state
/// throughout, so the wait is visible rather than a frozen click.
#[tauri::command]
#[tracing::instrument]
pub async fn generate_day_summary_now(
    day: String,
) -> Result<meridian_core::day_summaries::DaySummary, String> {
    if day.is_empty() {
        return Err("day is required".to_string());
    }
    let summary: meridian_core::day_summaries::DaySummary = run_meridian_json(
        &["day-summary", "--day", &day, "--now"],
        Duration::from_secs(600),
        "day-summary-now",
    )
    .await?;
    tracing::info!(
        %day,
        provider = %summary.provider,
        planned = summary.adherence.planned,
        done = summary.adherence.done,
        fallback = summary.fallback,
        "day-summary (generate now) served"
    );
    Ok(summary)
}

/// Read the stored summary for a day, or `None` when it has never been generated.
#[tauri::command]
#[tracing::instrument(skip(pool))]
pub async fn get_day_summary(
    pool: State<'_, Option<meridian_core::SqlitePool>>,
    day: Option<String>,
) -> Result<Option<meridian_core::day_summaries::DaySummary>, String> {
    let Some(pool) = pool.inner() else {
        return Err("meridian.db is not open yet".to_string());
    };
    let date = day.unwrap_or_else(meridian_core::date::today_string);
    meridian_core::day_summaries::get_day_summary(pool, &date)
        .await
        .map_err(|e| crate::cmd_err!(e, "get_day_summary failed"))
}

/// Everything the summary screen renders that does not come from the model:
/// `{datasets, scalars: {focus_s, coding_s, task_count, planned, …}, evidence_at,
/// stale}`.
///
/// **The scalars** come from here rather than being derived in the frontend on
/// purpose: `focus_s` and `coding_s` must be the SAME values the home page shows,
/// and `coding_s` in particular is agent time folded into the coding category — a
/// rule that already exists once in [`meridian_core::day_evidence`] and would be a
/// second, drifting copy if the screen recomputed it. `planned` is the branch
/// between the two versions of the screen.
///
/// **`stale`** says the day has moved on far enough since the stored summary was
/// composed that it is worth recomposing (see
/// [`meridian_core::day_summaries::is_stale`]). It is answered here, rather than by
/// a fourth command, because this one has already paid for the evidence collect —
/// and it is answered in Rust rather than by the frontend comparing two timestamps,
/// because "far enough" is a product rule, not a rendering detail.
///
/// The prose evidence ([`meridian_core::day_evidence::Evidence`]'s hour reports and
/// workstream logs) is deliberately NOT returned: it is the model's input, not the
/// frontend's, and shipping it to a screen that never renders it is pure weight.
#[tauri::command]
#[tracing::instrument(skip(pool))]
pub async fn get_day_summary_data(
    pool: State<'_, Option<meridian_core::SqlitePool>>,
    day: Option<String>,
) -> Result<serde_json::Value, String> {
    let Some(pool) = pool.inner() else {
        return Err("meridian.db is not open yet".to_string());
    };
    let date = day.unwrap_or_else(meridian_core::date::today_string);
    let ev = meridian_core::day_evidence::collect(pool, &date)
        .await
        .map_err(|e| crate::cmd_err!(e, "get_day_summary_data failed"))?;

    // A missing summary is NOT stale: there is nothing to recompose, and the screen
    // shows its own "compose this day" state instead. Only an existing one can go
    // out of date.
    let stale = match meridian_core::day_summaries::get_day_summary(pool, &date).await {
        Ok(Some(s)) => meridian_core::day_summaries::is_stale(&s.evidence_at, &ev.evidence_at),
        _ => false,
    };

    tracing::info!(
        day = %date,
        datasets = ev.datasets.len(),
        planned = ev.planned,
        stale,
        "day-summary data served"
    );
    Ok(serde_json::json!({
        "datasets": serde_json::Value::Object(ev.datasets),
        "scalars": ev.scalars,
        "evidence_at": ev.evidence_at,
        "stale": stale,
    }))
}
