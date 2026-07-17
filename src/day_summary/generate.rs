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
//! Every failure degrades to [`fallback_panels`]: a bad LLM call, an unparseable
//! answer, or an answer whose every panel is invalid all end with a rendered
//! screen and `fallback = 1`. The one thing that DOES propagate is a collect
//! failure — if the day's data cannot be read there is nothing to summarise, and
//! pretending otherwise would show a confident empty review of a day that had work
//! in it.
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
use crate::llm::{self, prompts, PromptRequest};
use meridian_core::day_evidence::{self, datasets};

/// Generous: the answer carries prose plus up to four Vega-Lite specs, and a
/// layered spec is verbose. Truncation here reads as a parse failure downstream,
/// which is a confusing way to discover the budget was too small.
const GENERATE_MAX_TOKENS: u32 = 8000;

/// The screen holds four panels. The schema says so too, but a schema is a request
/// on three of five providers, so the cap is enforced here as well.
const MAX_PANELS: usize = 4;

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
/// some providers, so drift is expected rather than exceptional. A missing
/// `narrative` or `insights` costs prose, not the screen — only a total absence of
/// panels is worth reporting as a parse failure, and even that degrades rather than
/// erroring.
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

/// The deterministic panels used when the model gives us nothing usable.
///
/// Plain, correct, and hand-checked against the validator by
/// `tests::the_fallback_panels_are_themselves_valid` — a fallback that fails to
/// render would turn a degraded screen into a broken one, which is the one outcome
/// this whole path exists to prevent.
pub fn fallback_panels() -> Vec<SummaryPanel> {
    vec![
        SummaryPanel {
            title: "When you worked".to_string(),
            why: "the day's sittings on one time axis".to_string(),
            spec: json!({
                "data": {"name": "segments"},
                "mark": {"type": "bar", "cornerRadius": 2},
                "encoding": {
                    "x":  {"field": "start_min", "type": "quantitative",
                           "title": "time of day",
                           "axis": {"labelExpr": "format(floor(datum.value/60),'02') + ':00'"},
                           "scale": {"domain": [0, 1440]}},
                    "x2": {"field": "end_min"},
                    "y":  {"field": "title", "type": "nominal", "title": null},
                    "color": {"field": "title", "type": "nominal", "legend": null}
                }
            }),
        },
        SummaryPanel {
            title: "Where the time went".to_string(),
            why: "the split across kinds of work".to_string(),
            spec: json!({
                "data": {"name": "categories"},
                "transform": [{"calculate": "datum.seconds / 60", "as": "minutes_spent"}],
                "mark": {"type": "arc", "innerRadius": 50},
                "encoding": {
                    "theta": {"field": "minutes_spent", "type": "quantitative"},
                    "color": {"field": "category", "type": "nominal", "title": null}
                }
            }),
        },
        SummaryPanel {
            title: "Top apps".to_string(),
            why: "which tools the day actually ran through".to_string(),
            spec: json!({
                "data": {"name": "apps"},
                "transform": [
                    {"calculate": "datum.seconds / 60", "as": "minutes_spent"},
                    {"window": [{"op": "rank", "as": "r"}], "sort": [{"field": "seconds", "order": "descending"}]},
                    {"filter": "datum.r <= 8"}
                ],
                "mark": {"type": "bar", "cornerRadius": 2},
                "encoding": {
                    "x": {"field": "minutes_spent", "type": "quantitative", "title": "minutes"},
                    "y": {"field": "app", "type": "nominal", "sort": "-x", "title": null}
                }
            }),
        },
    ]
}

/// Render the day's evidence as the user message.
fn build_user_prompt(ev: &day_evidence::Evidence) -> String {
    let mut s = String::new();
    s.push_str(&format!("=== THE DAY: {} ===\n", ev.day));
    s.push_str(&format!("Totals: {}\n", ev.scalars));

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
        s.push_str("\n=== HOUR BY HOUR (prose evidence, not chartable) ===\n");
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

    // Read at call time, like the resolver reads the provider — changing the model
    // in Settings must take effect on the next generate, not the next restart.
    let model = meridian_core::settings::load_runtime_settings()
        .llm_provider_model
        .unwrap_or_default();
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

    let fallback = kept.is_empty();
    let (narrative, insights, panels) = if fallback {
        // No narrative on the fallback path: the prose the model wrote (if any)
        // described panels that no longer exist, and inventing a replacement is
        // exactly what this feature must not do.
        (String::new(), Vec::new(), fallback_panels())
    } else {
        let a = answer.expect("kept is non-empty, so an answer was parsed");
        (a.narrative, a.insights, kept)
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

/// The datasets a panel binds to, for the frontend to inject at render.
///
/// Read live rather than stored with the spec: the day's data keeps moving, and a
/// frozen copy would render a chart that silently disagrees with the timeline
/// beside it. See migration 064's header.
pub async fn panel_data(pool: &SqlitePool, day_local: &str) -> Result<Value> {
    let ev = day_evidence::collect(pool, day_local).await?;
    Ok(Value::Object(ev.datasets))
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

    #[test]
    fn the_fallback_set_fits_the_screen() {
        assert!(fallback_panels().len() <= MAX_PANELS);
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

    /// Missing prose costs prose, not the screen — the panels are the point.
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
