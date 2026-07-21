//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! The read/write side of the daily summary (`day_summaries`, migrations 066 + 068)
//! — the end-of-day review behind the timeline's "Daily summary" button.
//!
//! # What this is
//! One row per local day. This module owns the DB shape and the wire contract only;
//! collecting the day's evidence, the LLM call, and folding the model's answer into
//! a score live daemon-side in `src/day_summary/` (which reuses these types so the
//! CLI, tray and UI all serialize one shape — the same split as
//! [`crate::day_task_worklogs`]).
//!
//! # What a summary is made of
//! - A [`DaySummary::headline`] and a `narrative`. The prose is the feature. The
//!   narrative may carry `**emphasis**` spans; the screen renders them and nothing
//!   else, so it is the one markup convention in the contract.
//! - [`DaySummaryInsight`]s — three cards, each a title the model writes itself and
//!   a line under it. Both fields are FREE TEXT, never an enum: a fixed set of
//!   categories would make every day fill the same slots whether or not it had
//!   anything to say in them. The screen colours the cards by position.
//! - [`PlanVerdict`]s — one per planned ticket, when the day had a plan.
//! - [`Adherence`] — the counts and the percentage. **Computed in Rust**, never read
//!   out of the model's text, so the ring and the list beside it cannot disagree.
//! - [`DayTheme`]s — what an unplanned day was about, grouped by the model.
//!
//! There are no chart specs here any more. The screen used to store model-authored
//! Vega-Lite panels; migration 068 dropped them along with the whole chart system.
//!
//! # Who calls this
//! - [`get_day_summary`] → the tray `get_day_summary` command → the summary overlay.
//!   Degrades to `None` on a pre-066 DB rather than erroring.
//! - [`upsert_summary`] → the daemon's `day_summary::generate`.
//!
//! # Related
//! - [`crate::day_tasks`] — the workstreams the summary is composed from.
//! - [`crate::day_evidence::adherence`] — how a [`PlanVerdict`] is decided.
//! - [`crate::hour_text`] — the per-hour reports it also reads as evidence.

use crate::SqlitePool;
use anyhow::Context;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use tracing::Instrument;

/// One observation about the day, as the screen's cards render it.
///
/// **Nothing here is categorised.** Both fields are free text. An earlier shape had
/// a closed `kind` (`achieved` / `overperformed` / `drifted`), and a later one a
/// three-value `tone` — both were slots, and slots get filled whether or not the day
/// had anything to put in them. What a card is about is whatever the model found;
/// the screen colours the three cards by position, so it never needs to be told.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaySummaryInsight {
    /// The card's own heading, in the model's words — three or so at most.
    #[serde(default)]
    pub title: String,
    /// A sentence or two under the title. The substance.
    pub text: String,
}

/// How a planned ticket actually went.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Done,
    Partial,
    NotTouched,
}

impl Outcome {
    /// Parse the model's string, defaulting to the *least* flattering reading.
    ///
    /// An unrecognised outcome is a model that did not follow the contract, and
    /// crediting it as `done` would inflate the one number on the screen.
    pub fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "done" | "complete" | "completed" => Outcome::Done,
            "partial" | "started" | "in_progress" => Outcome::Partial,
            _ => Outcome::NotTouched,
        }
    }

    /// What this outcome contributes to the achievement figure, in half-units, so
    /// the arithmetic stays integral: done = 2, partial = 1, untouched = 0.
    pub fn half_credits(self) -> i64 {
        match self {
            Outcome::Done => 2,
            Outcome::Partial => 1,
            Outcome::NotTouched => 0,
        }
    }
}

/// One planned ticket and what became of it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanVerdict {
    pub task_key: String,
    /// The ticket's title, carried so the screen never has to re-read the board (a
    /// planned ticket can leave it the moment it is closed).
    pub title: String,
    pub outcome: Outcome,
    /// One short line saying WHY this outcome. For a deterministic match that is a
    /// fact ("a worklog was posted against it"); otherwise it is the model's
    /// evidence from the day's own log lines.
    #[serde(default)]
    pub evidence: String,
    /// Measured minutes attributed to it, when work could be tied to it at all.
    #[serde(default)]
    pub minutes: i64,
    /// The `day_tasks` ids this ticket's outcome was read off — a posted worklog's
    /// workstream, or the ones the model cited. Empty on `not_touched`.
    ///
    /// Carried on the wire because the screen shows ONE list of the day's work with
    /// each row marked on-plan or not; without these ids it would have to guess the
    /// join by matching titles, which is exactly the guessing this ledger exists to
    /// remove.
    #[serde(default)]
    pub day_task_ids: Vec<String>,
    /// True when this outcome was established from the database rather than by the
    /// model — a posted worklog, a linked ticket, or a ticket that went terminal.
    /// The screen marks these; the model is not allowed to overturn them.
    #[serde(default)]
    pub certain: bool,
    /// The tracker's own key/url bits, for the ticket chip. Empty when unknown.
    #[serde(default)]
    pub provider: String,
    #[serde(default)]
    pub url: String,
}

/// The day's plan-adherence arithmetic. Every field is computed, never parsed.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Adherence {
    pub planned: i64,
    pub done: i64,
    pub partial: i64,
    pub not_touched: i64,
    /// `round(100 * (done + partial/2) / planned)`, or 0 when nothing was planned.
    pub achievement_pct: i64,
    /// Measured minutes spent on substantial workstreams that no planned ticket
    /// accounts for. Not a reproach - it is usually the most interesting number on
    /// the screen.
    pub unplanned_minutes: i64,
}

/// One thing an *unplanned* day was about, as grouped by the model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DayTheme {
    pub title: String,
    /// The `day_tasks` ids this theme covers. The screen sums their real measured
    /// minutes rather than trusting a figure from the model.
    #[serde(default)]
    pub day_task_ids: Vec<String>,
}

/// A day's composed summary — the wire contract shared by the CLI, tray and UI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaySummary {
    /// Local day, "YYYY-MM-DD".
    pub day: String,
    /// A short warm line above everything. Empty on the fallback path.
    #[serde(default)]
    pub headline: String,
    /// The model's prose on how the day went. May contain `**emphasis**` spans.
    /// Empty on the fallback path.
    pub narrative: String,
    /// Short observations.
    #[serde(default)]
    pub insights: Vec<DaySummaryInsight>,
    /// One per planned ticket; empty when the day had no plan.
    #[serde(default)]
    pub plan: Vec<PlanVerdict>,
    /// The computed adherence figures. Zeroed when the day had no plan.
    #[serde(default)]
    pub adherence: Adherence,
    /// What the day was about, for the no-plan case; empty when there was a plan.
    #[serde(default)]
    pub themes: Vec<DayTheme>,
    /// Which provider ACTUALLY answered (the resolver may degrade to local).
    pub provider: String,
    /// The model override in force, or "" for the provider's default.
    pub model: String,
    /// True when the summary could not be composed at all (the call failed, or the
    /// answer was unparseable). Not an error — the screen still renders what is
    /// deterministically true — but it is the honest signal of whether this feature
    /// is working.
    pub fallback: bool,
    pub generated_at: String,
    /// The newest tracked activity this summary was composed from, RFC3339. The
    /// staleness check compares it against the day as it stands now, which is what
    /// lets the screen quietly recompose instead of showing a summary of a morning
    /// at six in the evening. Empty on rows written before migration 068.
    #[serde(default)]
    pub evidence_at: String,
}

/// What [`upsert_summary`] writes. Separate from [`DaySummary`] so the caller
/// can't accidentally persist a read-back's `day` under a different key.
#[derive(Debug, Clone)]
pub struct SummaryUpsert {
    pub day: String,
    pub headline: String,
    pub narrative: String,
    pub insights: Vec<DaySummaryInsight>,
    pub plan: Vec<PlanVerdict>,
    pub adherence: Adherence,
    pub themes: Vec<DayTheme>,
    pub provider: String,
    pub model: String,
    pub fallback: bool,
    pub generated_at: String,
    pub evidence_at: String,
}

#[derive(FromRow)]
struct RawSummaryRow {
    day_local: String,
    headline: String,
    narrative: String,
    insights_json: String,
    plan_json: String,
    adherence_json: String,
    themes_json: String,
    provider: String,
    model: String,
    fallback: i64,
    generated_at: String,
    evidence_at: Option<String>,
}

/// Read the persisted summary for `day`, or `None` when there isn't one.
///
/// A missing `day_summaries` table (a DB that predates migration 066) degrades to
/// `None`, matching every other reader here: an un-migrated DB has no summary, it
/// is not a failure. Malformed stored JSON likewise degrades to empty rather than
/// erroring — a summary is a convenience, and refusing to open the screen because
/// one column's JSON rotted would be a poor trade. That same tolerance is what
/// carries a pre-068 row (whose `insights_json` held bare strings) to an empty
/// insight list instead of a hard failure.
#[tracing::instrument(skip(pool))]
pub async fn get_day_summary(pool: &SqlitePool, day: &str) -> anyhow::Result<Option<DaySummary>> {
    let row = sqlx::query_as::<_, RawSummaryRow>(
        "SELECT day_local, headline, narrative, insights_json, plan_json, adherence_json, \
                themes_json, provider, model, fallback, generated_at, evidence_at \
         FROM day_summaries WHERE day_local = ?",
    )
    .bind(day)
    .fetch_optional(pool)
    .instrument(tracing::debug_span!("day_summaries.read.day_summaries"))
    .await;

    let row = match row {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, day, "day_summaries: read skipped (pre-066 DB?)");
            return Ok(None);
        }
    };
    tracing::debug!(
        rows = row.is_some() as i64,
        "day_summaries.read.day_summaries"
    );

    Ok(row.map(|r| DaySummary {
        day: r.day_local,
        headline: r.headline,
        narrative: r.narrative,
        insights: serde_json::from_str(&r.insights_json).unwrap_or_default(),
        plan: serde_json::from_str(&r.plan_json).unwrap_or_default(),
        adherence: serde_json::from_str(&r.adherence_json).unwrap_or_default(),
        themes: serde_json::from_str(&r.themes_json).unwrap_or_default(),
        provider: r.provider,
        model: r.model,
        fallback: r.fallback != 0,
        generated_at: r.generated_at,
        evidence_at: r.evidence_at.unwrap_or_default(),
    }))
}

/// How far the day must have moved on before a stored summary is worth recomposing.
///
/// Twenty minutes, not zero. A summary is not wrong the moment another frame
/// lands: a day that has gained a few minutes of the same work is still the day it
/// describes, and recomposing on every open would spend an LLM call and a minute
/// of the user's patience to change a sentence's wording. Twenty minutes is
/// roughly where a new sitting becomes something the summary would have mentioned.
pub const STALE_AFTER_MINUTES: i64 = 20;

/// Whether a summary composed from `stored` is worth recomposing now that the day's
/// newest activity is `current`.
///
/// Both are RFC3339 instants from [`crate::day_evidence::Evidence::evidence_at`].
/// An **empty `stored`** is stale: it means a row written before migration 068, by
/// the previous version of this feature, whose shape the screen no longer renders
/// fully - one silent recompose is exactly right there. An empty or unparseable
/// `current` is NOT stale: it means the day reads as empty right now, and throwing
/// away a good summary because a read came back thin would be the worse failure.
pub fn is_stale(stored: &str, current: &str) -> bool {
    if stored.is_empty() {
        return true;
    }
    let (Ok(a), Ok(b)) = (
        chrono::DateTime::parse_from_rfc3339(stored),
        chrono::DateTime::parse_from_rfc3339(current),
    ) else {
        return false;
    };
    (b - a).num_minutes() >= STALE_AFTER_MINUTES
}

/// Write (or overwrite) the summary for a day.
///
/// UPSERT on `day_local`: regenerating is an explicit user act and the newest
/// answer wins outright. Each regeneration may legitimately look nothing like the
/// last — that is the feature, not drift.
#[tracing::instrument(skip(pool, up), fields(day = %up.day, planned = up.plan.len()))]
pub async fn upsert_summary(pool: &SqlitePool, up: &SummaryUpsert) -> anyhow::Result<()> {
    let insights_json =
        serde_json::to_string(&up.insights).context("day_summaries: encode insights")?;
    let plan_json = serde_json::to_string(&up.plan).context("day_summaries: encode plan")?;
    let adherence_json =
        serde_json::to_string(&up.adherence).context("day_summaries: encode adherence")?;
    let themes_json = serde_json::to_string(&up.themes).context("day_summaries: encode themes")?;

    sqlx::query(
        "INSERT INTO day_summaries \
            (day_local, headline, narrative, insights_json, plan_json, adherence_json, \
             themes_json, provider, model, fallback, generated_at, evidence_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT(day_local) DO UPDATE SET \
            headline = excluded.headline, \
            narrative = excluded.narrative, \
            insights_json = excluded.insights_json, \
            plan_json = excluded.plan_json, \
            adherence_json = excluded.adherence_json, \
            themes_json = excluded.themes_json, \
            provider = excluded.provider, \
            model = excluded.model, \
            fallback = excluded.fallback, \
            generated_at = excluded.generated_at, \
            evidence_at = excluded.evidence_at",
    )
    .bind(&up.day)
    .bind(&up.headline)
    .bind(&up.narrative)
    .bind(&insights_json)
    .bind(&plan_json)
    .bind(&adherence_json)
    .bind(&themes_json)
    .bind(&up.provider)
    .bind(&up.model)
    .bind(up.fallback as i64)
    .bind(&up.generated_at)
    .bind(&up.evidence_at)
    .execute(pool)
    .instrument(tracing::debug_span!("day_summaries.write.upsert"))
    .await
    .context("day_summaries: upsert")?;

    tracing::info!(
        day = %up.day,
        planned = up.plan.len(),
        achievement_pct = up.adherence.achievement_pct,
        fallback = up.fallback,
        "day summary persisted"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;

    /// The post-068 table, as the migration leaves it.
    async fn pool_with_summaries() -> SqlitePool {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE day_summaries (day_local TEXT PRIMARY KEY, narrative TEXT NOT NULL \
             DEFAULT '', insights_json TEXT NOT NULL DEFAULT '[]', provider TEXT NOT NULL, \
             model TEXT NOT NULL DEFAULT '', fallback INTEGER NOT NULL DEFAULT 0, \
             generated_at TEXT NOT NULL, headline TEXT NOT NULL DEFAULT '', \
             plan_json TEXT NOT NULL DEFAULT '[]', adherence_json TEXT NOT NULL DEFAULT '{}', \
             themes_json TEXT NOT NULL DEFAULT '[]', evidence_at TEXT)",
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    fn upsert_of(day: &str, narrative: &str, keys: &[&str]) -> SummaryUpsert {
        SummaryUpsert {
            day: day.to_string(),
            headline: "A good day, one detour".to_string(),
            narrative: narrative.to_string(),
            insights: vec![
                DaySummaryInsight {
                    title: "One long stretch".to_string(),
                    text: "Most of the depth landed in one unbroken stretch.".to_string(),
                },
                DaySummaryInsight {
                    title: "New to you".to_string(),
                    text: "ATTACH does not carry a rekey.".to_string(),
                },
            ],
            plan: keys
                .iter()
                .map(|k| PlanVerdict {
                    task_key: (*k).to_string(),
                    title: format!("{k} title"),
                    outcome: Outcome::Done,
                    evidence: "a worklog was posted against it".to_string(),
                    minutes: 130,
                    day_task_ids: vec!["T1".to_string()],
                    certain: true,
                    provider: "jira".to_string(),
                    url: String::new(),
                })
                .collect(),
            adherence: Adherence {
                planned: keys.len() as i64,
                done: keys.len() as i64,
                partial: 0,
                not_touched: 0,
                achievement_pct: 100,
                unplanned_minutes: 40,
            },
            themes: Vec::new(),
            provider: "claude".to_string(),
            model: "sonnet".to_string(),
            fallback: false,
            generated_at: "2026-07-17T18:00:00Z".to_string(),
            evidence_at: "2026-07-17T17:55:00Z".to_string(),
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
    async fn round_trips_every_composed_field() {
        let pool = pool_with_summaries().await;
        upsert_summary(&pool, &upsert_of("2026-07-16", "A long day.", &["KAN-1"]))
            .await
            .unwrap();

        let got = get_day_summary(&pool, "2026-07-16").await.unwrap().unwrap();
        assert_eq!(got.narrative, "A long day.");
        assert_eq!(got.headline, "A good day, one detour");
        assert_eq!(got.insights.len(), 2);
        assert_eq!(got.insights[1].title, "New to you");
        assert_eq!(got.plan.len(), 1);
        assert_eq!(got.plan[0].task_key, "KAN-1");
        assert_eq!(got.plan[0].outcome, Outcome::Done);
        assert!(got.plan[0].certain);
        assert_eq!(got.adherence.achievement_pct, 100);
        assert_eq!(got.adherence.unplanned_minutes, 40);
        assert_eq!(got.evidence_at, "2026-07-17T17:55:00Z");
        assert!(!got.fallback);
    }

    /// Regenerating replaces the day's summary outright rather than accumulating
    /// rows — the newest answer wins, and it may look nothing like the last.
    #[tokio::test]
    async fn regenerating_overwrites_rather_than_duplicating() {
        let pool = pool_with_summaries().await;
        upsert_summary(
            &pool,
            &upsert_of("2026-07-16", "First take.", &["KAN-1", "KAN-2"]),
        )
        .await
        .unwrap();
        upsert_summary(&pool, &upsert_of("2026-07-16", "Second take.", &["KAN-3"]))
            .await
            .unwrap();

        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM day_summaries")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(n, 1, "regenerate must UPSERT, not append");

        let got = get_day_summary(&pool, "2026-07-16").await.unwrap().unwrap();
        assert_eq!(got.narrative, "Second take.");
        assert_eq!(got.plan.len(), 1);
        assert_eq!(got.plan[0].task_key, "KAN-3");
    }

    /// A pre-066 DB has no table at all. Opening the summary screen there must
    /// show "nothing generated yet", not an error.
    #[tokio::test]
    async fn a_pre_066_database_degrades_to_none() {
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

    /// A row written before 068 stored `insights_json` as bare strings. It must
    /// degrade to an empty insight list, not poison the whole read — the rest of
    /// the summary is still worth showing, and the next generate replaces it.
    #[tokio::test]
    async fn a_pre_068_insight_shape_degrades_to_empty() {
        let pool = pool_with_summaries().await;
        sqlx::query(
            "INSERT INTO day_summaries (day_local, narrative, insights_json, provider, \
             generated_at) VALUES ('2026-07-16', 'Old.', '[\"a bare string\"]', 'claude', 'x')",
        )
        .execute(&pool)
        .await
        .unwrap();

        let got = get_day_summary(&pool, "2026-07-16").await.unwrap().unwrap();
        assert_eq!(got.narrative, "Old.");
        assert!(got.insights.is_empty());
        assert!(got.evidence_at.is_empty());
    }

    #[test]
    fn staleness_needs_a_real_gap_not_just_a_newer_frame() {
        let a = "2026-07-16T10:00:00Z";
        assert!(
            !is_stale(a, "2026-07-16T10:19:00Z"),
            "19 min is the same day"
        );
        assert!(
            is_stale(a, "2026-07-16T10:20:00Z"),
            "20 min is a new sitting"
        );
        // A day that has not moved at all, or has somehow moved backwards, is not
        // stale — recomposing there would only spend a call to reword a sentence.
        assert!(!is_stale(a, a));
        assert!(!is_stale(a, "2026-07-16T09:00:00Z"));
        // Unreadable or absent "now" must never discard a good summary.
        assert!(!is_stale(a, ""));
        assert!(!is_stale(a, "not-a-time"));
        // A pre-068 row has no stamp at all, and its shape is one the screen no
        // longer renders fully — one silent recompose is exactly right.
        assert!(is_stale("", "2026-07-16T10:00:00Z"));
    }

    /// An outcome the model spelled some other way must not be credited. The
    /// achievement figure is the one number on the screen; guessing generously is
    /// how it stops meaning anything.
    #[test]
    fn an_unrecognised_outcome_reads_as_not_touched() {
        assert_eq!(Outcome::parse("done"), Outcome::Done);
        assert_eq!(Outcome::parse("  Partial "), Outcome::Partial);
        assert_eq!(Outcome::parse("mostly done"), Outcome::NotTouched);
        assert_eq!(Outcome::parse(""), Outcome::NotTouched);
    }
}
