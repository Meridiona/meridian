//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Compose a day's summary: collect the evidence, ask the model, validate, persist.
//!
//! # The shape of the call
//! ONE LLM call through [`crate::llm::complete`], so it obeys the user's global
//! provider choice like every other prose call in the product. There is no repair
//! round: a second call would double the latency of a ~1 minute action to rescue
//! answers that a capable model does not produce in the first place. An invalid
//! panel is dropped and counted; the drop reasons on the span are what say whether
//! that judgement is holding.
//!
//! # Nothing here can fail the screen
//! A bad LLM call or an unparseable answer degrades to [`fallback_panels`] with
//! `fallback = 1`. The one thing that DOES propagate is a collect failure — if the
//! day's data cannot be read there is nothing to summarise, and pretending
//! otherwise would show a confident empty review of a day that had work in it.
//!
//! **`fallback` does NOT mean "no charts".** Zero panels is a correct and common
//! answer: the prose is the feature and the prompt says charts are optional, so a
//! summary with a good narrative and no chart is the normal outcome, not a
//! degraded one. An answer whose panels were all invalid keeps its narrative and
//! simply shows no chart — only a summary that could not be composed AT ALL falls
//! back.
//!
//! # Related
//! - [`meridian_core::day_evidence`] — the evidence.
//! - [`super::validate`] — the two-layer spec check.
//! - [`crate::pm_worklog::generate`] — the sibling flow this mirrors.

use anyhow::{Context, Result};
use meridian_core::day_summaries::{self, DaySummary, SummaryPanel, SummaryUpsert};
use meridian_core::SqlitePool;
use serde_json::{json, Value};
use tracing::field::Empty;

use super::validate;
use crate::llm::config::LlmConfig;
use crate::llm::{self, prompts, PromptRequest};
use meridian_core::day_evidence::{self, datasets};
use meridian_core::settings::load_runtime_settings;

/// Generous: the answer carries prose plus up to two Vega-Lite specs, and a layered
/// spec is verbose. Truncation here reads as a parse failure downstream, which is a
/// confusing way to discover the budget was too small.
const GENERATE_MAX_TOKENS: u32 = 8000;

/// At most two, and often none. The schema says so too, but a schema is a request
/// on three of five providers, so the cap is enforced here as well.
///
/// This was 4, and 4 is how the screen first shipped looking like a monitoring
/// dashboard: a pie of categories, a bar of totals, and two more that restated
/// them. Panels are optional now — the prose is the feature and a chart has to earn
/// its place beside it.
const MAX_PANELS: usize = 2;

/// Per-workstream log lines are the richest prose input and the easiest to blow a
/// context window with. Same posture as `SESSION_TEXT_CAP`: cap it, and record what
/// was actually sent rather than what we meant to send.
const MAX_LOG_LINES_PER_WORKSTREAM: usize = 12;

/// An hour report is a paragraph; 24 of them is a lot of tokens for a diminishing
/// return, so the longest are trimmed rather than the set truncated (dropping whole
/// hours would silently lose the parts of the day the model most needs).
const MAX_HOUR_REPORT_CHARS: usize = 600;

/// What the model answered, before validation.
#[derive(Debug, Clone)]
struct Answer {
    narrative: String,
    insights: Vec<String>,
    panels: Vec<SummaryPanel>,
}

/// Parse the model's answer, tolerantly.
///
/// Deliberately more forgiving than the schema, matching
/// `pm_worklog::generate::parse_answer`: a schema is genuinely enforced only on
/// some providers, so drift is expected rather than exceptional.
///
/// `None` means the text was not JSON at all — that, and only that, is the parse
/// failure that triggers the fallback. An empty `panels` array is a perfectly good
/// answer (charts are optional), and a missing `narrative` costs prose rather than
/// the screen.
fn parse_answer(text: &str) -> Option<Answer> {
    let v = llm::parse_json_object(text)?;

    let narrative = v
        .get("narrative")
        .and_then(|n| n.as_str())
        .unwrap_or_default()
        .trim()
        .to_string();

    let insights = v
        .get("insights")
        .and_then(|i| i.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|s| s.as_str())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default();

    let panels = v
        .get("panels")
        .and_then(|p| p.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|p| {
                    // A panel with no spec is not a panel. Everything else is
                    // recoverable: an untitled chart still shows its point.
                    let spec = p.get("spec")?.clone();
                    if !spec.is_object() {
                        return None;
                    }
                    Some(SummaryPanel {
                        title: p
                            .get("title")
                            .and_then(|t| t.as_str())
                            .unwrap_or("")
                            .trim()
                            .to_string(),
                        why: p
                            .get("why")
                            .and_then(|w| w.as_str())
                            .unwrap_or("")
                            .trim()
                            .to_string(),
                        spec,
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    Some(Answer {
        narrative,
        insights,
        panels,
    })
}

/// The deterministic panel used when the model gives us nothing usable.
///
/// ONE panel, not a set. This path runs when there is no narrative to show, and the
/// honest response to "the model could not tell you about your day" is a plain
/// picture of what happened - not three charts arranged to look like an answer. It
/// was a set of three (sittings + a category pie + an app bar) back when panels
/// were mandatory, and it was as much of a dashboard as the thing it was standing
/// in for.
///
/// Hand-checked against the validator by `tests::the_fallback_panels_are_themselves_valid`:
/// a fallback that fails to render would turn a degraded screen into a broken one,
/// which is the one outcome this whole path exists to prevent.
pub fn fallback_panels() -> Vec<SummaryPanel> {
    vec![SummaryPanel {
        title: "What the day looked like".to_string(),
        why: "the day's sittings on one time axis".to_string(),
        spec: json!({
            "data": {"name": "segments"},
            "mark": {"type": "bar", "cornerRadius": 3},
            "encoding": {
                "x":  {"field": "start_min", "type": "quantitative",
                       "title": null,
                       "axis": {"labelExpr": "format(floor(datum.value/60),'02') + ':00'"},
                       "scale": {"domain": [0, 1440]}},
                "x2": {"field": "end_min"},
                "y":  {"field": "title", "type": "nominal", "title": null},
                "tooltip": [
                    {"field": "title", "type": "nominal", "title": "work"},
                    {"field": "minutes", "type": "quantitative", "title": "minutes"}
                ]
            }
        }),
    }]
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
        "\n`focus_s`, `coding_s` and `task_count` are ALREADY DISPLAYED on this screen as \
         FOCUS, CODING and the count of things you did. Do not read them back. Use them to \
         understand the day.\n",
    );

    s.push_str("\n=== DATASETS (the only names and fields you may reference) ===\n");
    s.push_str(&datasets::describe());

    s.push_str("\n=== THE DATA ===\n");
    for (name, rows) in &ev.datasets {
        s.push_str(&format!("\n{name} = {rows}\n"));
    }

    if !ev.workstream_logs.is_empty() {
        s.push_str("\n=== WHAT EACH WORKSTREAM INVOLVED (prose evidence, not chartable) ===\n");
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
            "\n=== EVIDENCE OF WHAT THE WORK INVOLVED (prose, not chartable) ===\n\
             Grouped by hour ONLY because that is how it was captured. The hour labels are \
             not part of the story - do not sequence your answer around them, and never \
             quote one.\n",
        );
        for (h, r) in &ev.hour_reports {
            let trimmed: String = r.chars().take(MAX_HOUR_REPORT_CHARS).collect();
            s.push_str(&format!("{h:02}:00 - {trimmed}\n"));
        }
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
    panels_returned = Empty,
    panels_kept = Empty,
    panels_dropped = Empty,
    drop_reasons = Empty,
    fallback = Empty,
    prompt_chars = Empty,
))]
pub async fn generate(pool: &SqlitePool, day_local: &str) -> Result<DaySummary> {
    let span = tracing::Span::current();

    // The one hard failure: no evidence, nothing to summarise.
    let ev = day_evidence::collect(pool, day_local)
        .await
        .context("day_summary: collecting the day's evidence")?;

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
    // how the two quietly disagree the next time it moves. Read at call time, like
    // the resolver reads the provider, so changing it in Settings takes effect on the
    // next generate rather than the next restart.
    let model = LlmConfig::from_settings(&load_runtime_settings()).model;
    span.record("model", model.as_str());

    // ── validate ──────────────────────────────────────────────────────────────
    let returned = answer.as_ref().map(|a| a.panels.len()).unwrap_or(0);
    let mut kept: Vec<SummaryPanel> = Vec::new();
    let mut reasons: Vec<String> = Vec::new();

    if let Some(a) = &answer {
        for p in &a.panels {
            match validate::check(p) {
                Ok(()) => kept.push(p.clone()),
                Err(r) => {
                    tracing::warn!(
                        day = day_local,
                        title = %p.title,
                        reason = %r,
                        tag = r.tag(),
                        "daily summary: dropping an invalid panel"
                    );
                    reasons.push(r.tag().to_string());
                }
            }
        }
    }
    kept.truncate(MAX_PANELS);

    // ZERO PANELS IS A CORRECT ANSWER, not a failure. The prompt says charts are
    // optional and most days genuinely need none, so the fallback trigger is the
    // ANSWER being absent — a failed call or an unparseable one — never an empty
    // panel list. This read `kept.is_empty()` when panels were mandatory; leaving
    // it that way would throw away a perfectly good narrative for exactly the days
    // the model judged best told in words alone, which is the common case.
    let fallback = answer.is_none();
    let (narrative, insights, panels) = match answer {
        Some(a) => (a.narrative, a.insights, kept),
        // No narrative on the fallback path: there is no prose to show, and writing
        // a replacement is exactly what this feature must not do. One honest chart
        // of what happened, and nothing claimed about it.
        None => (String::new(), Vec::new(), fallback_panels()),
    };

    // Record every field on BOTH paths, `""`/`0` included. OpenObserve only learns
    // a field once a record carries it, so a dashboard filtering on `drop_reasons`
    // errors until some record has one. Same reason worklog.generate stamps an
    // empty matched_task_key on the propose branch. See daily-summary.json.
    span.record("panels_returned", returned);
    span.record("panels_kept", panels.len());
    span.record("panels_dropped", reasons.len());
    span.record("drop_reasons", reasons.join(","));
    span.record("fallback", fallback);

    let now = chrono::Utc::now().to_rfc3339();
    let up = SummaryUpsert {
        day: day_local.to_string(),
        narrative,
        insights,
        panels,
        provider,
        model,
        fallback,
        generated_at: now,
    };
    day_summaries::upsert_summary(pool, &up)
        .await
        .context("day_summary: persisting the summary")?;

    tracing::info!(
        day = day_local,
        panels = up.panels.len(),
        dropped = reasons.len(),
        fallback,
        "daily summary composed"
    );

    Ok(DaySummary {
        day: up.day,
        narrative: up.narrative,
        insights: up.insights,
        panels: up.panels,
        provider: up.provider,
        model: up.model,
        fallback: up.fallback,
        generated_at: up.generated_at,
    })
}

/// Read a persisted summary — the CLI's `--get` side.
pub async fn get(pool: &SqlitePool, day_local: &str) -> Result<Option<DaySummary>> {
    day_summaries::get_day_summary(pool, day_local).await
}

/// What the screen renders besides the model's words: the datasets each panel binds
/// to, and the headline scalars shown beside the prose.
///
/// Read live rather than stored with the spec: the day's data keeps moving, and a
/// frozen copy would render a chart that silently disagrees with the timeline
/// beside it. See migration 064's header.
///
/// Mirrors the tray's `get_day_summary_data` exactly — the tray reads
/// `day_evidence` directly rather than spawning this, so the two shapes are kept
/// identical deliberately: this is the debugging view of what the screen is given
/// (`meridian day-summary-data --day X | jq .scalars`), and it is worth nothing if
/// it shows something else.
pub async fn panel_data(pool: &SqlitePool, day_local: &str) -> Result<Value> {
    let ev = day_evidence::collect(pool, day_local).await?;
    Ok(json!({
        "datasets": Value::Object(ev.datasets),
        "scalars": ev.scalars,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The fallback is the safety net under every other failure path. If it does
    /// not itself pass the validator, a degraded screen becomes a broken one.
    #[test]
    fn the_fallback_panels_are_themselves_valid() {
        for p in fallback_panels() {
            assert_eq!(validate::check(&p), Ok(()), "fallback panel {:?}", p.title);
        }
    }

    /// The fallback stands in for a summary that could not be written. One honest
    /// chart is the whole of it — a set of three was itself a dashboard, which is
    /// the thing this screen must not be.
    #[test]
    fn the_fallback_is_a_single_panel() {
        assert_eq!(fallback_panels().len(), 1);
        assert!(fallback_panels().len() <= MAX_PANELS);
    }

    /// Zero panels is a CORRECT answer, so an answer that parsed must never be
    /// treated as a fallback just because the model chose no chart. Pins the rule
    /// the `fallback` flag encodes; `kept.is_empty()` here would silently discard
    /// the narrative on exactly the days best told in words alone.
    #[test]
    fn an_answer_with_no_panels_is_still_an_answer() {
        let a = parse_answer(
            r#"{"narrative":"A deep day on one thing.","insights":["x"],"panels":[]}"#,
        )
        .expect("an empty panel list is valid, not a parse failure");
        assert!(a.panels.is_empty());
        assert_eq!(a.narrative, "A deep day on one thing.");
    }

    #[test]
    fn parses_a_well_formed_answer() {
        let a = parse_answer(
            r#"{"narrative": "A long day.",
                "insights": ["Six threads, one long.", "  "],
                "panels": [{"title": "T", "why": "W", "spec": {"mark": "bar"}}]}"#,
        )
        .unwrap();
        assert_eq!(a.narrative, "A long day.");
        // Blank insight lines are dropped rather than rendered as empty rows.
        assert_eq!(a.insights, vec!["Six threads, one long."]);
        assert_eq!(a.panels.len(), 1);
        assert_eq!(a.panels[0].title, "T");
    }

    /// Copilot fences its JSON and Cursor wraps it in prose; the shared tolerant
    /// parser handles both, and this pins that we go through it.
    #[test]
    fn parses_a_fenced_answer() {
        let a = parse_answer(
            "Here you go:\n```json\n{\"narrative\":\"x\",\"insights\":[],\"panels\":[]}\n```",
        )
        .unwrap();
        assert_eq!(a.narrative, "x");
    }

    #[test]
    fn a_non_json_answer_does_not_parse() {
        assert!(parse_answer("I could not do that.").is_none());
    }

    /// Drift on the prose fields costs prose, not the screen. (Note the direction:
    /// the prose is the feature and the panels are optional, so this is the
    /// expensive kind of drift — but degrading beats erroring either way.)
    #[test]
    fn tolerates_a_missing_narrative_and_insights() {
        let a = parse_answer(r#"{"panels": [{"spec": {"mark": "bar"}}]}"#).unwrap();
        assert_eq!(a.narrative, "");
        assert!(a.insights.is_empty());
        assert_eq!(a.panels.len(), 1);
    }

    /// A "panel" with no spec, or a non-object spec, is not a panel.
    #[test]
    fn drops_panels_without_a_usable_spec() {
        let a = parse_answer(
            r#"{"narrative":"x","insights":[],
                "panels":[{"title":"no spec"},
                          {"title":"string spec","spec":"bar"},
                          {"title":"ok","spec":{"mark":"bar"}}]}"#,
        )
        .unwrap();
        assert_eq!(a.panels.len(), 1);
        assert_eq!(a.panels[0].title, "ok");
    }
}
