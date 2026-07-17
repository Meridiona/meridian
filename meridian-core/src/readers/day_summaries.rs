//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! The read/write side of the daily summary (`day_summaries`, migration 064) —
//! the AI-composed end-of-day review behind the timeline's "Daily summary" button.
//!
//! # What this is
//! One row per local day: the model's narrative, its insight lines, and the
//! visualisation **specs** it chose. This module owns the DB shape only; the
//! collection of the day's evidence, the LLM call, and spec validation live
//! daemon-side in `src/day_summary/` (which reuses these types so the CLI, tray
//! and UI all serialize one wire contract — the same split as
//! [`crate::day_task_worklogs`]).
//!
//! # Panels carry form, never data
//! A [`SummaryPanel`]'s `spec` is a raw Vega-Lite spec whose data is bound BY NAME
//! (`{"data": {"name": "segments"}}`); the frontend injects the real rows at
//! render. Nothing here stores a number the model produced. See migration 064's
//! header for why (the short version: the day's data keeps moving, and a frozen
//! copy would render a chart that silently disagrees with the timeline beside it).
//!
//! # Who calls this
//! - [`get_day_summary`] → the tray `get_day_summary` command → the summary
//!   overlay (shows an existing summary on open). Degrades to `None` on a pre-064
//!   DB rather than erroring.
//! - [`upsert_summary`] → the daemon's `day_summary::generate`.
//!
//! # Related
//! - [`crate::day_tasks`] — the workstreams the summary is composed from.
//! - [`crate::hour_text`] — the per-hour reports it also reads as evidence.

use crate::SqlitePool;
use anyhow::Context;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use tracing::Instrument;

/// One visualisation the model chose: what it is, why that form, and the spec.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SummaryPanel {
    /// Short human title for the panel.
    pub title: String,
    /// The model's stated reason for choosing THIS form for THIS data. Required
    /// of the model on purpose: asking for the reason is what makes it consider
    /// fit at all, instead of reaching for a bar chart every time.
    #[serde(default)]
    pub why: String,
    /// A raw Vega-Lite spec. Validated before it ever lands here — `data` is
    /// always `{"name": "<known dataset>"}`, never inline values.
    pub spec: serde_json::Value,
}

/// A day's composed summary — the wire contract shared by the CLI, tray and UI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaySummary {
    /// Local day, "YYYY-MM-DD".
    pub day: String,
    /// The model's prose on how the day went. Empty on the fallback path.
    pub narrative: String,
    /// Short insight lines.
    pub insights: Vec<String>,
    /// The visualisations, in the order the model chose.
    pub panels: Vec<SummaryPanel>,
    /// Which provider ACTUALLY answered (the resolver may degrade to local).
    pub provider: String,
    /// The model override in force, or "" for the provider's default.
    pub model: String,
    /// True when the model produced no usable panel and the deterministic set was
    /// substituted. Not an error — the screen always renders — but it is the
    /// honest signal of whether this feature is working.
    pub fallback: bool,
    pub generated_at: String,
}

/// What [`upsert_summary`] writes. Separate from [`DaySummary`] so the caller
/// can't accidentally persist a read-back's `day` under a different key.
#[derive(Debug, Clone)]
pub struct SummaryUpsert {
    pub day: String,
    pub narrative: String,
    pub insights: Vec<String>,
    pub panels: Vec<SummaryPanel>,
    pub provider: String,
    pub model: String,
    pub fallback: bool,
    pub generated_at: String,
}

#[derive(FromRow)]
struct RawSummaryRow {
    day_local: String,
    narrative: String,
    panels_json: String,
    insights_json: String,
    provider: String,
    model: String,
    fallback: i64,
    generated_at: String,
}

/// Read the persisted summary for `day`, or `None` when there isn't one.
///
/// A missing `day_summaries` table (a DB that predates migration 064) degrades to
/// `None`, matching every other reader here: an un-migrated DB has no summary, it
/// is not a failure. Malformed stored JSON likewise degrades to empty rather than
/// erroring — a summary is a convenience, and refusing to open the screen because
/// one panel's JSON rotted would be a poor trade.
#[tracing::instrument(skip(pool))]
pub async fn get_day_summary(pool: &SqlitePool, day: &str) -> anyhow::Result<Option<DaySummary>> {
    let row = sqlx::query_as::<_, RawSummaryRow>(
        "SELECT day_local, narrative, panels_json, insights_json, provider, model, \
                fallback, generated_at \
         FROM day_summaries WHERE day_local = ?",
    )
    .bind(day)
    .fetch_optional(pool)
    .instrument(tracing::debug_span!("day_summaries.read.day_summaries"))
    .await;

    let row = match row {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, day, "day_summaries: read skipped (pre-064 DB?)");
            return Ok(None);
        }
    };
    tracing::debug!(
        rows = row.is_some() as i64,
        "day_summaries.read.day_summaries"
    );

    Ok(row.map(|r| DaySummary {
        day: r.day_local,
        narrative: r.narrative,
        insights: serde_json::from_str(&r.insights_json).unwrap_or_default(),
        panels: serde_json::from_str(&r.panels_json).unwrap_or_default(),
        provider: r.provider,
        model: r.model,
        fallback: r.fallback != 0,
        generated_at: r.generated_at,
    }))
}

/// Write (or overwrite) the summary for a day.
///
/// UPSERT on `day_local`: regenerating is an explicit user act and the newest
/// answer wins outright. Each regeneration may legitimately look nothing like the
/// last — that is the feature, not drift.
#[tracing::instrument(skip(pool, up), fields(day = %up.day, panels = up.panels.len()))]
pub async fn upsert_summary(pool: &SqlitePool, up: &SummaryUpsert) -> anyhow::Result<()> {
    let panels_json = serde_json::to_string(&up.panels).context("day_summaries: encode panels")?;
    let insights_json =
        serde_json::to_string(&up.insights).context("day_summaries: encode insights")?;

    sqlx::query(
        "INSERT INTO day_summaries \
            (day_local, narrative, panels_json, insights_json, provider, model, fallback, generated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT(day_local) DO UPDATE SET \
            narrative = excluded.narrative, \
            panels_json = excluded.panels_json, \
            insights_json = excluded.insights_json, \
            provider = excluded.provider, \
            model = excluded.model, \
            fallback = excluded.fallback, \
            generated_at = excluded.generated_at",
    )
    .bind(&up.day)
    .bind(&up.narrative)
    .bind(&panels_json)
    .bind(&insights_json)
    .bind(&up.provider)
    .bind(&up.model)
    .bind(up.fallback as i64)
    .bind(&up.generated_at)
    .execute(pool)
    .instrument(tracing::debug_span!("day_summaries.write.upsert"))
    .await
    .context("day_summaries: upsert")?;

    tracing::info!(day = %up.day, panels = up.panels.len(), fallback = up.fallback, "day summary persisted");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;

    async fn pool_with_summaries() -> SqlitePool {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE day_summaries (day_local TEXT PRIMARY KEY, narrative TEXT NOT NULL \
             DEFAULT '', panels_json TEXT NOT NULL DEFAULT '[]', insights_json TEXT NOT NULL \
             DEFAULT '[]', provider TEXT NOT NULL, model TEXT NOT NULL DEFAULT '', \
             fallback INTEGER NOT NULL DEFAULT 0, generated_at TEXT NOT NULL)",
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    fn upsert_of(day: &str, narrative: &str, titles: &[&str]) -> SummaryUpsert {
        SummaryUpsert {
            day: day.to_string(),
            narrative: narrative.to_string(),
            insights: vec!["Six workstreams, one long thread.".to_string()],
            panels: titles
                .iter()
                .map(|t| SummaryPanel {
                    title: (*t).to_string(),
                    why: "segments show interleaving a bar chart would hide".to_string(),
                    spec: serde_json::json!({"data": {"name": "segments"}, "mark": "bar"}),
                })
                .collect(),
            provider: "claude".to_string(),
            model: "sonnet".to_string(),
            fallback: false,
            generated_at: "2026-07-17T18:00:00Z".to_string(),
        }
    }

    #[tokio::test]
    async fn a_day_with_no_summary_reads_none() {
        let pool = pool_with_summaries().await;
        assert!(get_day_summary(&pool, "2026-07-16")
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn round_trips_panels_and_insights() {
        let pool = pool_with_summaries().await;
        upsert_summary(
            &pool,
            &upsert_of("2026-07-16", "A long day.", &["Where it fragmented"]),
        )
        .await
        .unwrap();

        let got = get_day_summary(&pool, "2026-07-16").await.unwrap().unwrap();
        assert_eq!(got.narrative, "A long day.");
        assert_eq!(got.panels.len(), 1);
        assert_eq!(got.panels[0].title, "Where it fragmented");
        assert_eq!(got.panels[0].spec["data"]["name"], "segments");
        assert_eq!(got.insights.len(), 1);
        assert_eq!(got.provider, "claude");
        assert_eq!(got.model, "sonnet");
        assert!(!got.fallback);
    }

    /// Regenerating replaces the day's summary outright rather than accumulating
    /// rows — the newest answer wins, and it may look nothing like the last.
    #[tokio::test]
    async fn regenerating_overwrites_rather_than_duplicating() {
        let pool = pool_with_summaries().await;
        upsert_summary(&pool, &upsert_of("2026-07-16", "First take.", &["A", "B"]))
            .await
            .unwrap();
        upsert_summary(&pool, &upsert_of("2026-07-16", "Second take.", &["C"]))
            .await
            .unwrap();

        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM day_summaries")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(n, 1, "regenerate must UPSERT, not append");

        let got = get_day_summary(&pool, "2026-07-16").await.unwrap().unwrap();
        assert_eq!(got.narrative, "Second take.");
        assert_eq!(got.panels.len(), 1);
        assert_eq!(got.panels[0].title, "C");
    }

    /// A pre-064 DB has no table at all. Opening the summary screen there must
    /// show "nothing generated yet", not an error.
    #[tokio::test]
    async fn a_pre_064_database_degrades_to_none() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        assert!(get_day_summary(&pool, "2026-07-16")
            .await
            .unwrap()
            .is_none());
    }
}
