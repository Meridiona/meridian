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
//! - [`get_day_summary_data`] — the rows each panel's spec binds to.
//!
//! # Why the data is a separate command
//! A stored panel carries FORM only: its spec says `{"data": {"name": "segments"}}`
//! and nothing else. The rows are read live here and injected at render, so a chart
//! can never drift from the timeline beside it (migration 064 has the full why).
//! That also means these two commands are always called together.
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
/// This does not fail on a bad answer: the CLI falls back to a deterministic panel
/// set rather than erroring, so an `Err` here means the DB or the spawn, never the
/// model. The returned summary's `fallback` flag is what says the model came up
/// short.
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
        panels = summary.panels.len(),
        fallback = summary.fallback,
        "day-summary served"
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
        .map_err(|e| {
            tracing::warn!(error = %e, "get_day_summary failed");
            e.to_string()
        })
}

/// Everything the summary screen renders besides the model's own words:
/// `{datasets: {segments: [...], …}, scalars: {focus_s, coding_s, task_count, …}}`.
///
/// **The datasets** are what each panel's spec binds to by name — a stored spec
/// carries form only, so the rows are fetched here and injected at render.
///
/// **The scalars** are the screen's headline numbers, and they come from here rather
/// than being derived in the frontend on purpose: `focus_s` and `coding_s` must be
/// the SAME values the home page shows, and `coding_s` in particular is agent time
/// folded into the coding category — a rule that already exists once in
/// [`meridian_core::day_evidence`] and would be a second, drifting copy if the
/// screen recomputed it.
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
        .map_err(|e| {
            tracing::warn!(error = %e, "get_day_summary_data failed");
            e.to_string()
        })?;
    tracing::info!(day = %date, datasets = ev.datasets.len(), "day-summary data served");
    Ok(serde_json::json!({
        "datasets": serde_json::Value::Object(ev.datasets),
        "scalars": ev.scalars,
    }))
}
