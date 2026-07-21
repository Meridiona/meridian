//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Compose a day's summary: collect the evidence, ask the model, fold, score, persist.
//!
//! # The shape of the call
//! ONE LLM call through [`crate::llm::complete`], so it obeys the user's global
//! provider choice like every other prose call in the product. There is no repair
//! round: a second call would double the latency of a ~1 minute action to rescue
//! answers a capable model does not produce in the first place.
//!
//! # Nothing here can fail the screen
//! A bad LLM call or an unparseable answer degrades to `fallback = 1` — no prose,
//! but the plan ledger and its arithmetic still persist, because those are computed
//! from the database and never needed the model. The one thing that DOES propagate
//! is a collect failure: if the day's data cannot be read there is nothing to
//! summarise, and pretending otherwise would show a confident empty review of a day
//! that had work in it.
//!
//! # Where the number comes from
//! Not from here, and not from the model. [`meridian_core::day_evidence::adherence`]
//! folds the locked matches over the model's verdicts and does the arithmetic; this
//! module only parses, hands off, and writes down what comes back. That split is
//! what makes the percentage on the screen reproducible.
//!
//! # Related
//! - [`meridian_core::day_evidence`] — the evidence.
//! - [`meridian_core::day_evidence::adherence`] — the fold and the score.
//! - [`crate::pm_worklog::generate`] — the sibling flow this mirrors.

use anyhow::{Context, Result};
use meridian_core::day_summaries::{
    self, DaySummary, DaySummaryInsight, DayTheme, Outcome, SummaryUpsert,
};
use meridian_core::SqlitePool;
use serde_json::{json, Value};
use tracing::field::Empty;

use crate::llm::config::LlmConfig;
use crate::llm::{self, prompts, PromptRequest};
use meridian_core::day_evidence::{
    self,
    adherence::{self, ModelVerdict},
};
use meridian_core::settings::load_runtime_settings;

/// Generous: the answer carries prose plus a verdict per planned ticket. Truncation
/// reads as a parse failure downstream, which is a confusing way to discover the
/// budget was too small.
const GENERATE_MAX_TOKENS: u32 = 4000;

/// Per-workstream log lines are the richest prose input and the easiest to blow a
/// context window with. Same posture as `SESSION_TEXT_CAP`: cap it, and record what
/// was actually sent rather than what we meant to send.
const MAX_LOG_LINES_PER_WORKSTREAM: usize = 12;

/// An hour report is a paragraph; 24 of them is a lot of tokens for a diminishing
/// return, so the longest are trimmed rather than the set truncated (dropping whole
/// hours would silently lose the parts of the day the model most needs).
const MAX_HOUR_REPORT_CHARS: usize = 600;

/// The plan ticket description the model is shown. Enough to judge what a ticket is
/// about; not so much that ten of them crowd out the day's own log lines, which are
/// the evidence it actually has to reason from.
const MAX_PLAN_DESCRIPTION_CHARS: usize = 240;

/// What the model answered, before folding.
#[derive(Debug, Clone, Default)]
struct Answer {
    headline: String,
    narrative: String,
    insights: Vec<DaySummaryInsight>,
    verdicts: Vec<ModelVerdict>,
    themes: Vec<DayTheme>,
}

/// Parse the model's answer, tolerantly.
///
/// Deliberately more forgiving than the schema, matching
/// `pm_worklog::generate::parse_answer`: a schema is genuinely enforced only on
/// some providers, so drift is expected rather than exceptional.
///
/// `None` means the text was not JSON at all — that, and only that, is the parse
/// failure that triggers the fallback. Every individual field degrades on its own:
/// a missing narrative costs prose, a missing verdict costs one ledger line.
fn parse_answer(text: &str) -> Option<Answer> {
    let v = llm::parse_json_object(text)?;

    let string_at = |key: &str| -> String {
        v.get(key)
            .and_then(|n| n.as_str())
            .unwrap_or_default()
            .trim()
            .to_string()
    };

    let insights = v
        .get("insights")
        .and_then(|i| i.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|i| {
                    // Tolerate the older shapes (a bare string, or an object with
                    // no title) as well as the current one: a provider that
                    // ignores the schema and copies an older example should cost
                    // the card's heading, not the line itself.
                    let text = match i {
                        Value::String(s) => s.trim().to_string(),
                        _ => i.get("text")?.as_str()?.trim().to_string(),
                    };
                    if text.is_empty() {
                        return None;
                    }
                    Some(DaySummaryInsight {
                        title: i
                            .get("title")
                            .and_then(|t| t.as_str())
                            .unwrap_or_default()
                            .trim()
                            .to_string(),
                        text,
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    let verdicts = v
        .get("plan_verdicts")
        .and_then(|p| p.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|p| {
                    // A verdict with no ticket cannot be applied to anything.
                    let task_key = p.get("task_key")?.as_str()?.trim().to_string();
                    if task_key.is_empty() {
                        return None;
                    }
                    Some(ModelVerdict {
                        task_key,
                        // An unrecognised outcome reads as `not_touched` — see
                        // `Outcome::parse`. Guessing generously is how the one
                        // number on the screen stops meaning anything.
                        outcome: Outcome::parse(
                            p.get("outcome")
                                .and_then(|o| o.as_str())
                                .unwrap_or_default(),
                        ),
                        evidence: p
                            .get("evidence")
                            .and_then(|e| e.as_str())
                            .unwrap_or_default()
                            .trim()
                            .to_string(),
                        day_task_ids: p
                            .get("day_task_ids")
                            .and_then(|d| d.as_array())
                            .map(|ids| {
                                ids.iter()
                                    .filter_map(|i| i.as_str())
                                    .map(str::to_string)
                                    .collect()
                            })
                            .unwrap_or_default(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    let themes = v
        .get("themes")
        .and_then(|t| t.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|t| {
                    let title = t.get("title")?.as_str()?.trim().to_string();
                    if title.is_empty() {
                        return None;
                    }
                    Some(DayTheme {
                        title,
                        day_task_ids: t
                            .get("day_task_ids")
                            .and_then(|d| d.as_array())
                            .map(|ids| {
                                ids.iter()
                                    .filter_map(|i| i.as_str())
                                    .map(str::to_string)
                                    .collect()
                            })
                            .unwrap_or_default(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    Some(Answer {
        headline: string_at("headline"),
        narrative: string_at("narrative"),
        insights,
        verdicts,
        themes,
    })
}

/// Render the day's evidence as the user message.
fn build_user_prompt(ev: &day_evidence::Evidence) -> String {
    let mut s = String::new();
    s.push_str(&format!("=== THE DAY: {} ===\n", ev.day));
    s.push_str(&format!("Scalars: {}\n", ev.scalars));
    // Named explicitly because the screen shows these three, verbatim, right next
    // to the prose. The model needs to know they are ALREADY on the page - left to
    // infer it from a bag of scalars, it spends its two sentences reciting numbers
    // the reader can see an inch away.
    s.push_str(
        "\n`focus_s`, `coding_s` and `task_count` are ALREADY DISPLAYED on this screen. \
         Do not read them back. Use them to understand the day.\n",
    );

    if !ev.workstream_logs.is_empty() {
        // The workstream ids are load-bearing on the no-plan branch: `themes`
        // references them, and a theme naming an id that was never shown is
        // dropped at render.
        s.push_str(
            "\n=== WORKSTREAMS - WHAT YOU ACTUALLY DID (the only ids you may reference) ===\n",
        );
        for w in &ev.workstream_logs {
            s.push_str(&format!(
                "\n{} - {} ({} min)\n",
                w.task_id, w.title, w.minutes
            ));
            for line in w.lines.iter().take(MAX_LOG_LINES_PER_WORKSTREAM) {
                s.push_str(&format!("  - {line}\n"));
            }
        }
    }

    if !ev.hour_reports.is_empty() {
        // The framing matters as much as the content. Headed "HOUR BY HOUR", this
        // block reliably produced an hour-by-hour answer - the model mirrors the
        // shape it is handed, and a timestamped list reads as an instruction to
        // narrate a sequence. It is the richest evidence of WHAT was done, so it
        // stays; the hour labels are demoted to what they actually are, a capture
        // artefact.
        s.push_str(
            "\n=== FURTHER EVIDENCE OF WHAT THE WORK INVOLVED ===\n\
             Grouped by hour ONLY because that is how it was captured. The hour labels are \
             not part of the story - do not sequence your answer around them, and never \
             quote one.\n",
        );
        for (h, r) in &ev.hour_reports {
            let trimmed: String = r.chars().take(MAX_HOUR_REPORT_CHARS).collect();
            s.push_str(&format!("{h:02}:00 - {trimmed}\n"));
        }
    }

    if ev.planned {
        s.push_str("\n=== TODAY'S PLAN - what you committed to this morning ===\n");
        s.push_str("Return exactly one entry in `plan_verdicts` for each. `themes` must be [].\n");
        for p in &ev.plan {
            let desc: String = p
                .description
                .chars()
                .take(MAX_PLAN_DESCRIPTION_CHARS)
                .collect();
            s.push_str(&format!("\n{} - {}\n", p.task_key, p.title));
            if let Some(epic) = p.epic.as_deref().filter(|e| !e.is_empty()) {
                s.push_str(&format!("  epic: {epic}\n"));
            }
            if !desc.trim().is_empty() {
                s.push_str(&format!("  about: {}\n", desc.trim()));
            }
        }

        // The locked half. Shown rather than hidden on purpose: these are the
        // day's firmest facts about what got finished, and the model writes better
        // prose knowing them. It is told plainly that arguing is pointless.
        let settled = adherence::prematch(&ev.plan, &ev.tasks);
        if !settled.is_empty() {
            s.push_str(
                "\n=== ALREADY ESTABLISHED (locked - your outcome for these is ignored) ===\n",
            );
            for m in &settled {
                s.push_str(&format!(
                    "{} - done, because {}\n",
                    m.task_key,
                    m.evidence.reason()
                ));
            }
        }
    } else {
        s.push_str(
            "\n=== NO PLAN WAS SET FOR THIS DAY ===\n\
             `plan_verdicts` must be []. Group the workstreams above into `themes` instead.\n",
        );
    }

    s
}

/// Compose and persist the summary for `day_local`.
///
/// Takes no `Config`, unlike its `pm_worklog::generate` sibling: that one needs a
/// PM provider to file against, and this one files nothing. The model override is
/// read from settings at call time, the same way [`crate::llm::resolver`] reads the
/// provider — deliberately uncached, so changing it in Settings takes effect on the
/// next generate rather than the next restart.
// The span name is set explicitly: `#[instrument]` would name it after the fn
// ("generate"), and the dashboards + the house convention key on the dotted
// `<module>.<verb>` form (`worklog.generate` next door does the same).
#[tracing::instrument(name = "day_summary.generate", skip(pool), fields(
    llm_provider = Empty,
    model = Empty,
    planned = Empty,
    plan_items = Empty,
    verdicts_returned = Empty,
    certain_matches = Empty,
    achievement_pct = Empty,
    unplanned_minutes = Empty,
    themes = Empty,
    insights = Empty,
    fallback = Empty,
    prompt_chars = Empty,
))]
pub async fn generate(pool: &SqlitePool, day_local: &str) -> Result<DaySummary> {
    let span = tracing::Span::current();

    // The one hard failure: no evidence, nothing to summarise.
    let ev = day_evidence::collect(pool, day_local)
        .await
        .context("day_summary: collecting the day's evidence")?;
    span.record("planned", ev.planned);
    span.record("plan_items", ev.plan.len());

    let user = build_user_prompt(&ev);
    span.record("prompt_chars", user.len());

    let req = PromptRequest {
        system: prompts::DAILY_SUMMARY,
        user,
        schema: Some(prompts::daily_summary_schema()),
        max_tokens: GENERATE_MAX_TOKENS,
        label: format!("daily-summary {day_local}"),
    };

    // A failed call is not a failed screen: fall back rather than surfacing an
    // error for a feature whose entire job is to feel good to open.
    let (answer, provider) = match llm::complete(&req).await {
        Ok((out, provider)) => {
            span.record("llm_provider", provider.as_str());
            match parse_answer(&out.text) {
                Some(a) => (Some(a), provider.as_str().to_string()),
                None => {
                    tracing::warn!(
                        day = day_local,
                        "daily summary: answer unparseable — falling back"
                    );
                    (None, provider.as_str().to_string())
                }
            }
        }
        Err(e) => {
            tracing::warn!(day = day_local, error = %e, "daily summary: LLM call failed — falling back");
            span.record("llm_provider", "");
            (None, String::new())
        }
    };

    // The model override comes through `LlmConfig` — the ONE place that defines what
    // the global AI setting means — rather than reaching into settings.json for
    // `llm_provider_model` directly. This module has no business knowing which
    // settings key backs the choice or that an absent one means "the provider's
    // default"; that is `llm::config`'s job, and a second reader of the same key is
    // how the two quietly disagree the next time it moves.
    let model = LlmConfig::from_settings(&load_runtime_settings()).model;
    span.record("model", model.as_str());

    let fallback = answer.is_none();
    let a = answer.unwrap_or_default();
    span.record("verdicts_returned", a.verdicts.len());

    // THE FOLD. Runs on BOTH paths, fallback included: the plan ledger is computed
    // from the database, so a failed model call costs the prose and nothing else.
    // A screen that shows "3 of 5, two worklogged" with no narrative is still worth
    // opening; one that shows nothing because a provider was rate-limited is not.
    let ledger = adherence::resolve(&ev.plan, &ev.tasks, &a.verdicts);
    span.record(
        "certain_matches",
        ledger.verdicts.iter().filter(|v| v.certain).count(),
    );
    span.record("achievement_pct", ledger.adherence.achievement_pct);
    span.record("unplanned_minutes", ledger.adherence.unplanned_minutes);

    // A theme pointing at a workstream the model was never shown (or invented) would
    // render as an empty bar, so it is dropped here rather than at paint time.
    let known: std::collections::HashSet<&str> = ev
        .workstream_logs
        .iter()
        .map(|w| w.task_id.as_str())
        .collect();
    let themes: Vec<DayTheme> = a
        .themes
        .into_iter()
        .map(|t| DayTheme {
            title: t.title,
            day_task_ids: t
                .day_task_ids
                .into_iter()
                .filter(|id| known.contains(id.as_str()))
                .collect(),
        })
        .filter(|t| !t.day_task_ids.is_empty())
        .collect();

    // Record every field on BOTH paths, `""`/`0` included. OpenObserve only learns a
    // field once a record carries it, so a dashboard filtering on one errors until
    // some record has it. Same reason worklog.generate stamps an empty
    // matched_task_key on the propose branch. See daily-summary.json.
    span.record("themes", themes.len());
    span.record("insights", a.insights.len());
    span.record("fallback", fallback);

    let now = chrono::Utc::now().to_rfc3339();
    let up = SummaryUpsert {
        day: day_local.to_string(),
        headline: a.headline,
        narrative: a.narrative,
        insights: a.insights,
        plan: ledger.verdicts,
        adherence: ledger.adherence,
        themes,
        provider,
        model,
        fallback,
        generated_at: now,
        evidence_at: ev.evidence_at,
    };
    day_summaries::upsert_summary(pool, &up)
        .await
        .context("day_summary: persisting the summary")?;

    tracing::info!(
        day = day_local,
        planned = ev.planned,
        achievement_pct = up.adherence.achievement_pct,
        themes = up.themes.len(),
        fallback,
        "daily summary composed"
    );

    Ok(DaySummary {
        day: up.day,
        headline: up.headline,
        narrative: up.narrative,
        insights: up.insights,
        plan: up.plan,
        adherence: up.adherence,
        themes: up.themes,
        provider: up.provider,
        model: up.model,
        fallback: up.fallback,
        generated_at: up.generated_at,
        evidence_at: up.evidence_at,
    })
}

/// Read a persisted summary — the CLI's `--get` side.
pub async fn get(pool: &SqlitePool, day_local: &str) -> Result<Option<DaySummary>> {
    day_summaries::get_day_summary(pool, day_local).await
}

/// The deterministic half of what the screen renders: the day's aggregate datasets
/// and its headline scalars.
///
/// Read live rather than stored alongside the summary: the day keeps moving, and a
/// frozen copy would disagree with the timeline beside it.
///
/// Mirrors the tray's `get_day_summary_data` exactly — the tray reads `day_evidence`
/// directly rather than spawning this, so the two shapes are kept identical
/// deliberately: this is the debugging view of what the screen is given
/// (`meridian day-summary-data --day X | jq .scalars`), and it is worth nothing if
/// it shows something else.
pub async fn panel_data(pool: &SqlitePool, day_local: &str) -> Result<Value> {
    let ev = day_evidence::collect(pool, day_local).await?;
    Ok(json!({
        "datasets": Value::Object(ev.datasets),
        "scalars": ev.scalars,
        "evidence_at": ev.evidence_at,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_well_formed_planned_answer() {
        let a = parse_answer(
            r#"{"headline": "A good day, one detour",
                "narrative": "You closed the rework and **the triage bug pulled you sideways**.",
                "insights": [{"title": "One long stretch", "text": "Most of the depth landed in one go."},
                             {"title": "New to you", "text": "ATTACH does not carry a rekey."},
                             {"title": "Blank", "text": "  "}],
                "plan_verdicts": [{"task_key": "KAN-1", "outcome": "done", "evidence": "shipped it"},
                                  {"task_key": "KAN-2", "outcome": "partial", "evidence": "started"}],
                "themes": []}"#,
        )
        .unwrap();
        assert_eq!(a.headline, "A good day, one detour");
        // Blank insight lines are dropped rather than rendered as empty rows.
        assert_eq!(a.insights.len(), 2);
        assert_eq!(a.insights[1].title, "New to you");
        assert_eq!(a.verdicts.len(), 2);
        assert_eq!(a.verdicts[1].outcome, Outcome::Partial);
    }

    #[test]
    fn parses_a_well_formed_unplanned_answer() {
        let a = parse_answer(
            r#"{"headline": "One problem, all the way down",
                "narrative": "n", "insights": [],
                "plan_verdicts": [],
                "themes": [{"title": "Session distiller rework", "day_task_ids": ["T1", "T3"]},
                           {"title": "", "day_task_ids": ["T9"]}]}"#,
        )
        .unwrap();
        assert!(a.verdicts.is_empty());
        // An untitled theme is not a theme.
        assert_eq!(a.themes.len(), 1);
        assert_eq!(a.themes[0].day_task_ids, vec!["T1", "T3"]);
    }

    /// An outcome outside the enum must not be credited — the parser routes it to
    /// the least flattering reading rather than dropping the ledger line, so the
    /// ticket still appears and the score still adds up.
    #[test]
    fn an_invented_outcome_reads_as_not_touched() {
        let a = parse_answer(
            r#"{"plan_verdicts":[{"task_key":"KAN-1","outcome":"mostly done","evidence":"e"}]}"#,
        )
        .unwrap();
        assert_eq!(a.verdicts[0].outcome, Outcome::NotTouched);
    }

    /// A provider that ignores the schema and writes bare strings costs the card's
    /// heading, not the whole insight list.
    #[test]
    fn tolerates_the_older_bare_string_insight_shape() {
        let a = parse_answer(r#"{"insights":["one line","another"]}"#).unwrap();
        assert_eq!(a.insights.len(), 2);
        assert!(a.insights[0].title.is_empty());
        assert_eq!(a.insights[0].text, "one line");
    }

    /// Copilot fences its JSON and Cursor wraps it in prose; the shared tolerant
    /// parser handles both, and this pins that we go through it.
    #[test]
    fn parses_a_fenced_answer() {
        let a = parse_answer("Here you go:\n```json\n{\"narrative\":\"x\"}\n```").unwrap();
        assert_eq!(a.narrative, "x");
    }

    #[test]
    fn a_non_json_answer_does_not_parse() {
        assert!(parse_answer("I could not do that.").is_none());
    }

    /// Drift on any single field costs that field, never the screen.
    #[test]
    fn tolerates_a_missing_everything() {
        let a = parse_answer(r#"{"narrative": "just prose"}"#).unwrap();
        assert_eq!(a.narrative, "just prose");
        assert!(a.headline.is_empty());
        assert!(a.insights.is_empty());
        assert!(a.verdicts.is_empty());
        assert!(a.themes.is_empty());
    }

    /// A verdict with no ticket cannot be applied to anything, and one with a blank
    /// key would silently match nothing while looking like it did.
    #[test]
    fn drops_verdicts_without_a_ticket() {
        let a = parse_answer(
            r#"{"plan_verdicts":[{"outcome":"done","evidence":"e"},
                                 {"task_key":"  ","outcome":"done","evidence":"e"},
                                 {"task_key":"KAN-1","outcome":"done","evidence":"e"}]}"#,
        )
        .unwrap();
        assert_eq!(a.verdicts.len(), 1);
        assert_eq!(a.verdicts[0].task_key, "KAN-1");
    }
}
