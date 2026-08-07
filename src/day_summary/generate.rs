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
//! A bad LLM call or an unparseable answer degrades to `fallback = 1` — no cards,
//! but the plan ledger and its arithmetic still persist, because they are resolved
//! in `day_evidence::collect` from the worklog matches and never needed the model.
//! The one thing that DOES propagate is a collect failure: if the day's data cannot
//! be read there is nothing to summarise.
//!
//! # Where the number comes from
//! Not from here, and not from the model.
//! [`meridian_core::day_evidence::adherence::resolve_deterministic`] settles the
//! whole ledger from the worklog match rows; this module reads `ev.ledger`,
//! composes the prose beside it, and writes both down. The model is handed the
//! outcome as GIVEN and never returns a verdict, a count, or a duration.
//!
//! # Related
//! - [`meridian_core::day_evidence`] — the evidence and the resolved ledger.
//! - [`meridian_core::day_evidence::adherence`] — the deterministic resolve.
//! - [`crate::pm_worklog::generate`] — the sibling flow this mirrors.
//! - [`super::auto`] — composes this once a day at the user's end-of-day time.

use anyhow::{Context, Result};
use meridian_core::day_summaries::{self, DaySummary, DaySummaryInsight, Outcome, SummaryUpsert};
use meridian_core::SqlitePool;
use serde_json::{json, Value};
use tracing::field::Empty;

use crate::llm::config::LlmConfig;
use crate::llm::{self, prompts, PromptRequest};
use meridian_core::day_evidence::{self, adherence::DayLedger};
use meridian_core::settings::load_runtime_settings;

/// Generous headroom for the headline and three cards. Truncation reads as a parse
/// failure downstream, which is a confusing way to discover the budget was too small.
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

/// What the model answered. Just the prose now - the plan ledger is resolved in
/// [`meridian_core::day_evidence`] with no model input.
#[derive(Debug, Clone, Default)]
struct Answer {
    headline: String,
    insights: Vec<DaySummaryInsight>,
    /// The standup lines, one per bullet. Empty is a legitimate answer (a provider
    /// that ignored the field, or a day with nothing worth saying out loud) and
    /// costs the block, not the screen.
    standup: Vec<String>,
}

/// Parse the model's answer, tolerantly.
///
/// Deliberately more forgiving than the schema, matching
/// `pm_worklog::generate::parse_answer`: a schema is genuinely enforced only on
/// some providers, so drift is expected rather than exceptional.
///
/// `None` means the text was not JSON at all — that, and only that, is the parse
/// failure that triggers the fallback. Every field degrades on its own: a missing
/// headline costs the headline, a blank card costs that card.
fn parse_answer(text: &str) -> Option<Answer> {
    let v = llm::parse_json_object(text)?;

    let headline = v
        .get("headline")
        .and_then(|n| n.as_str())
        .unwrap_or_default()
        .trim()
        .to_string();

    let insights = v
        .get("insights")
        .and_then(|i| i.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|i| {
                    // Tolerate a bare string (a provider ignoring the schema) as
                    // well as the object: it costs the card's heading, not the line.
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

    // Tolerated two ways, matching how `insights` is read: an array of strings is
    // the contract, but a provider that ignores the schema and returns one blob of
    // text is split on newlines rather than thrown away - the lines are in there,
    // and the only thing lost is the model's own choice of where they break. Any
    // leading bullet character it added is stripped, because the screen draws those.
    let standup = match v.get("standup") {
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(|l| l.as_str())
            .map(clean_standup_line)
            .filter(|l| !l.is_empty())
            .collect(),
        Some(Value::String(s)) => s
            .lines()
            .map(clean_standup_line)
            .filter(|l| !l.is_empty())
            .collect(),
        _ => Vec::new(),
    };

    Some(Answer {
        headline,
        insights,
        standup,
    })
}

/// Strip the list markers a model adds when it forgets the screen draws them, and
/// trim. Leaves the line's own punctuation alone.
fn clean_standup_line(line: &str) -> String {
    line.trim()
        .trim_start_matches(['-', '*', '•', '·'])
        .trim()
        .to_string()
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
        // The workstreams are the evidence the insight cards are written from - what
        // was actually done, in the day's own words.
        s.push_str("\n=== WORKSTREAMS - WHAT YOU ACTUALLY DID ===\n");
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
        // The DECIDED outcome, not the raw plan. The model is told exactly which
        // tickets were done and which were not, already resolved from the worklog
        // matches - it describes this in card 1 and never recomputes it. Titles and
        // descriptions ride along so it can speak to what a ticket was ABOUT.
        let ad = &ev.ledger.adherence;
        s.push_str(&format!(
            "\n=== PLAN OUTCOME (ALREADY DECIDED - describe it, never recompute it) ===\n\
             {} of {} committed tickets done.\n",
            ad.done, ad.planned
        ));
        let plan_by_key: std::collections::HashMap<&str, &_> =
            ev.plan.iter().map(|p| (p.task_key.as_str(), p)).collect();
        for v in &ev.ledger.verdicts {
            let state = if v.outcome == Outcome::Done {
                "DONE"
            } else {
                "not started"
            };
            s.push_str(&format!("\n{} - {} ({})\n", v.task_key, v.title, state));
            if let Some(p) = plan_by_key.get(v.task_key.as_str()) {
                if let Some(epic) = p.epic.as_deref().filter(|e| !e.is_empty()) {
                    s.push_str(&format!("  epic: {epic}\n"));
                }
                let desc: String = p
                    .description
                    .chars()
                    .take(MAX_PLAN_DESCRIPTION_CHARS)
                    .collect();
                if !desc.trim().is_empty() {
                    s.push_str(&format!("  about: {}\n", desc.trim()));
                }
            }
        }
        if ad.unplanned_minutes > 0 {
            s.push_str(&format!(
                "\nPlus {} minutes on work that was not on the plan - credit it as a good thing.\n",
                ad.unplanned_minutes
            ));
        }
    } else {
        s.push_str(
            "\n=== NO PLAN WAS SET FOR THIS DAY ===\n\
             Describe the shape of the day from the workstreams in card 1 - one deep thing, \
             or several threads at once.\n",
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
    done = Empty,
    achievement_pct = Empty,
    unplanned_minutes = Empty,
    insights = Empty,
    standup_lines = Empty,
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

    // The ledger is DETERMINISTIC and already resolved in `collect` — it never
    // depended on the model. So a failed LLM call costs only the prose: the ring,
    // the counts, and the checklist still render from `ev.ledger`. A screen showing
    // "4 of 6 done" with no cards is worth opening; one blank because a provider was
    // rate-limited is not.
    let DayLedger {
        verdicts,
        adherence,
    } = ev.ledger;
    span.record("done", adherence.done);
    span.record("achievement_pct", adherence.achievement_pct);
    span.record("unplanned_minutes", adherence.unplanned_minutes);
    span.record("insights", a.insights.len());
    span.record("standup_lines", a.standup.len());
    span.record("fallback", fallback);

    let now = chrono::Utc::now().to_rfc3339();
    let up = SummaryUpsert {
        day: day_local.to_string(),
        headline: a.headline,
        // Narrative and themes are gone from the screen; the fields persist empty
        // for wire back-compat (no migration) and a pre-this-change row degrades
        // the same way.
        narrative: String::new(),
        insights: a.insights,
        plan: verdicts,
        adherence,
        themes: Vec::new(),
        standup: a.standup,
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
        done = up.adherence.done,
        achievement_pct = up.adherence.achievement_pct,
        insights = up.insights.len(),
        standup_lines = up.standup.len(),
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
        standup: up.standup,
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
    fn parses_headline_and_cards() {
        let a = parse_answer(
            r#"{"headline": "A good day, one detour",
                "insights": [{"title": "One long stretch", "text": "Most of the depth landed in one go."},
                             {"title": "New to you", "text": "ATTACH does not carry a rekey."},
                             {"title": "Blank", "text": "  "}]}"#,
        )
        .unwrap();
        assert_eq!(a.headline, "A good day, one detour");
        // Blank insight lines are dropped rather than rendered as empty rows.
        assert_eq!(a.insights.len(), 2);
        assert_eq!(a.insights[1].title, "New to you");
    }

    /// The plan is not the model's to return any more; if a stale prompt makes it
    /// emit verdicts or themes, they are simply ignored, not parsed.
    #[test]
    fn ignores_any_leftover_plan_fields() {
        let a = parse_answer(
            r#"{"headline": "h", "insights": [{"title":"a","text":"b"}],
                "plan_verdicts": [{"task_key":"KAN-1","outcome":"done"}],
                "themes": [{"title":"x","day_task_ids":["T1"]}]}"#,
        )
        .unwrap();
        assert_eq!(a.headline, "h");
        assert_eq!(a.insights.len(), 1);
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
        let a = parse_answer("Here you go:\n```json\n{\"headline\":\"x\"}\n```").unwrap();
        assert_eq!(a.headline, "x");
    }

    #[test]
    fn a_non_json_answer_does_not_parse() {
        assert!(parse_answer("I could not do that.").is_none());
    }

    /// Drift on any single field costs that field, never the screen.
    #[test]
    fn tolerates_a_missing_everything() {
        let a = parse_answer(r#"{"headline": "just a line"}"#).unwrap();
        assert_eq!(a.headline, "just a line");
        assert!(a.insights.is_empty());
        // A provider that ignores the standup field costs the block, not the screen.
        assert!(a.standup.is_empty());
    }

    #[test]
    fn parses_the_standup_lines() {
        let a = parse_answer(
            r#"{"headline": "h",
                "standup": ["Shipped the feed (MER-475).",
                            "Fixed the logout bug - ticket to file.",
                            "Next up: onboarding polish."]}"#,
        )
        .unwrap();
        assert_eq!(a.standup.len(), 3);
        assert_eq!(a.standup[0], "Shipped the feed (MER-475).");
        assert!(a.standup[2].starts_with("Next up:"));
    }

    /// The screen draws the bullets, so a model that adds its own must not end up
    /// rendering "• - Shipped…".
    #[test]
    fn strips_bullet_characters_the_model_added_itself() {
        let a = parse_answer(r#"{"standup": ["- Shipped the feed.", "• Fixed a bug.", "  "]}"#)
            .unwrap();
        assert_eq!(a.standup, vec!["Shipped the feed.", "Fixed a bug."]);
    }

    /// A provider with no schema mechanism may answer with one blob instead of an
    /// array. The lines are still in there - split them rather than dropping the
    /// whole block.
    #[test]
    fn tolerates_a_standup_returned_as_one_string() {
        let a = parse_answer(
            "{\"standup\": \"- Shipped the feed.\\n- Fixed a bug.\\n\\n- Next up: polish.\"}",
        )
        .unwrap();
        assert_eq!(a.standup.len(), 3);
        assert_eq!(a.standup[1], "Fixed a bug.");
    }
}
