//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Rebuild a prose stage's exact [`PromptRequest`] from stored pipeline inputs.
//!
//! Strictly read-only: reads `pm_worklog_hours` (distilled hour text / hour report),
//! `app_sessions` (via `hour_db`), `day_tasks` and `pm_tasks` (via
//! `generate_request`) — and writes nothing. The built request is snapshotted onto
//! the experiment row ([`snapshot`]) so [`super::runner::exec`] can replay it later
//! (and across resumes) byte-identically; the system prompt / schema / token
//! ceiling are re-derived from the process ([`super::ExperimentProcess::contract`]).
//!
//! # Who calls this
//! [`super::cli`] at create time; [`from_snapshot`] at exec time.

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use sqlx::SqlitePool;

use crate::llm::PromptRequest;
use crate::pm_worklog::generate::generate_request;
use crate::worklog_pipeline::{
    hour::report_request,
    hour_bounds, hour_db,
    hour_input::{compose_report_input, hour_span_minutes},
    task_db,
    workstream::workstream_request,
    workstream_state::build_state_json,
};

use super::{day_state, ExperimentInput, ExperimentProcess, ExperimentSpec};

/// The variant-independent request plus whatever the renderer needs later
/// (`render_ctx`, e.g. the hour's measured span for the report's minute clamp).
pub struct BuiltRequest {
    pub req: PromptRequest,
    pub render_ctx: Value,
}

/// The one entry `cli::create_experiment` uses: the `input_json` snapshot for any
/// process. Single-request processes snapshot their [`BuiltRequest`]; the day fold
/// snapshots the day's ordered `(hour, report)` chain instead (it is N requests,
/// one per stored hour — see the runner's day-fold branch).
pub async fn build_input_json(pool: &SqlitePool, spec: &ExperimentSpec) -> Result<String> {
    if let (ExperimentProcess::DayFold, ExperimentInput::Day(day)) = (&spec.process, &spec.input) {
        return day_fold_input_json(pool, day).await;
    }
    let built = build(pool, spec).await?;
    Ok(snapshot(&built))
}

/// Assemble the exact [`PromptRequest`] the pipeline would send for `spec`'s input.
/// Bails with an actionable message when the input was never produced (an hour with
/// no stored `hour_text` / `hour_report`, an unknown day-task).
pub async fn build(pool: &SqlitePool, spec: &ExperimentSpec) -> Result<BuiltRequest> {
    match (&spec.process, &spec.input) {
        (ExperimentProcess::HourReport, ExperimentInput::Hour(label)) => {
            build_hour_report(pool, label).await
        }
        (ExperimentProcess::WorkstreamFold, ExperimentInput::Hour(label)) => {
            build_workstream_fold(pool, label).await
        }
        (ExperimentProcess::WorklogGenerate, ExperimentInput::DayTask { day, task_id }) => {
            let (req, n_candidates) = generate_request(pool, day, task_id).await?;
            Ok(BuiltRequest {
                req,
                render_ctx: json!({ "n_candidates": n_candidates }),
            })
        }
        (p, i) => bail!(
            "process {} does not take input {:?} - hour processes want --hour, \
             worklog-generate wants --day + --task-id, day-fold wants --day",
            p.as_str(),
            i.ref_str()
        ),
    }
}

/// Hour report: stored distilled body + re-fetched timeline/coding rows through the
/// same `compose_report_input` the pipeline uses. The distillation itself is NOT
/// re-run (it is provider-independent) — an undistilled hour is not replayable.
async fn build_hour_report(pool: &SqlitePool, label: &str) -> Result<BuiltRequest> {
    let b = hour_bounds(label)?;
    let body = stored_hour_column(pool, &b.hs, "hour_text")
        .await?
        .with_context(|| {
            format!(
                "hour {label} has no stored distilled text - only hours the pipeline already \
             distilled can be replayed (run `meridian worklog-hour {label}` first)"
            )
        })?;

    let timeline = hour_db::fetch_hour_timeline(pool, &b.hs, &b.he).await;
    let coding = hour_db::fetch_coding_summaries(pool, &b.hs, &b.he).await;
    let composed = compose_report_input(&body, &timeline, &coding);
    if composed.text.trim().is_empty() {
        bail!("hour {label} composes to an empty report input - nothing to replay");
    }

    Ok(BuiltRequest {
        req: report_request(composed.text, label),
        render_ctx: json!({ "span_min": hour_span_minutes(&timeline) }),
    })
}

/// Workstream fold: the hour's stored activity report + the day's CURRENT prior
/// task state. Fidelity caveat (module docs): the original fold-time prior state is
/// not archived, so a replayed fold sees today's state — identical for every
/// variant, which is what a comparison needs.
async fn build_workstream_fold(pool: &SqlitePool, label: &str) -> Result<BuiltRequest> {
    let b = hour_bounds(label)?;
    let report = stored_hour_column(pool, &b.hs, "hour_report")
        .await?
        .with_context(|| {
            format!(
                "hour {label} has no stored activity report - only hours the pipeline already \
             reported can be fold-replayed (run `meridian worklog-hour {label}` first)"
            )
        })?;

    let prior = task_db::fetch_state(pool, &b.day_local).await;
    let state_json = build_state_json(&prior);
    Ok(BuiltRequest {
        req: workstream_request(&state_json, label, &report),
        // The prior working set rides along so the renderer can APPLY each
        // variant's placements and show the resulting day-task timeline (not
        // just the raw placements JSON).
        render_ctx: json!({
            "day": b.day_local,
            "prior": day_state::rows_to_json(&prior),
        }),
    })
}

/// The day fold's `input_json`: every hour of `day` (local) that has a stored
/// activity report, in chronological order. Bails when the day has none — an
/// unprocessed day has nothing to fold.
async fn day_fold_input_json(pool: &SqlitePool, day: &str) -> Result<String> {
    let mut hours: Vec<Value> = Vec::new();
    for h in 0..24 {
        let label = format!("{day}T{h:02}");
        // A DST-skipped hour simply doesn't exist locally; skip it, don't fail the day.
        let Ok(b) = hour_bounds(&label) else { continue };
        if let Some(report) = stored_hour_column(pool, &b.hs, "hour_report").await? {
            hours.push(json!({ "label": label, "report": report }));
        }
    }
    if hours.is_empty() {
        bail!(
            "day {day} has no stored hour reports - only days the pipeline already \
             processed can be day-folded"
        );
    }
    Ok(json!({ "label": format!("day-fold {day}"), "day": day, "hours": hours }).to_string())
}

/// Decode a day-fold `input_json` back into `(day, [(hour_label, report)])`.
pub fn day_fold_from_snapshot(input_json: &str) -> Result<(String, Vec<(String, String)>)> {
    let v: Value = serde_json::from_str(input_json).context("parsing day-fold input_json")?;
    let day = v
        .get("day")
        .and_then(Value::as_str)
        .context("day-fold input_json has no day")?
        .to_string();
    let hours = v
        .get("hours")
        .and_then(Value::as_array)
        .context("day-fold input_json has no hours")?
        .iter()
        .filter_map(|h| {
            Some((
                h.get("label")?.as_str()?.to_string(),
                h.get("report")?.as_str()?.to_string(),
            ))
        })
        .collect::<Vec<_>>();
    if hours.is_empty() {
        bail!("day-fold input_json has an empty hours list");
    }
    Ok((day, hours))
}

/// One nullable text column off the hour's ledger row. `Ok(None)` = row missing or
/// value empty (both mean "not replayable" to the callers).
async fn stored_hour_column(
    pool: &SqlitePool,
    hs: &str,
    column: &'static str,
) -> Result<Option<String>> {
    // `column` is one of two static names — never user input.
    let sql = format!("SELECT COALESCE({column}, '') FROM pm_worklog_hours WHERE hour_start = ?");
    let row: Option<(String,)> = sqlx::query_as(&sql)
        .bind(hs)
        .fetch_optional(pool)
        .await
        .with_context(|| format!("reading pm_worklog_hours.{column}"))?;
    Ok(row.map(|(v,)| v).filter(|v| !v.trim().is_empty()))
}

/// Serialise the variant-independent parts for the experiment row's `input_json`.
pub fn snapshot(built: &BuiltRequest) -> String {
    json!({
        "user": built.req.user,
        "label": built.req.label,
        "render_ctx": built.render_ctx,
    })
    .to_string()
}

/// Rebuild the request from a stored snapshot + the process contract. Returns the
/// request and the `render_ctx` the renderer needs.
pub fn from_snapshot(
    process: ExperimentProcess,
    input_json: &str,
) -> Result<(PromptRequest, Value)> {
    let v: Value = serde_json::from_str(input_json).context("parsing experiment input_json")?;
    let user = v
        .get("user")
        .and_then(Value::as_str)
        .context("experiment input_json has no user text")?
        .to_string();
    let label = v
        .get("label")
        .and_then(Value::as_str)
        .unwrap_or("llm-experiment")
        .to_string();
    let render_ctx = v.get("render_ctx").cloned().unwrap_or(json!({}));

    let (system, schema, max_tokens) = process.contract();
    Ok((
        PromptRequest {
            system,
            user,
            schema,
            max_tokens,
            label,
        },
        render_ctx,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_round_trips_through_from_snapshot() {
        let built = BuiltRequest {
            req: report_request("the hour's input".into(), "2026-07-15T14"),
            render_ctx: json!({ "span_min": 42 }),
        };
        let snap = snapshot(&built);
        let (req, ctx) = from_snapshot(ExperimentProcess::HourReport, &snap).unwrap();
        assert_eq!(req.user, "the hour's input");
        assert_eq!(req.label, "activity-report 2026-07-15T14");
        assert_eq!(req.system, built.req.system);
        assert_eq!(req.max_tokens, built.req.max_tokens);
        assert!(req.schema.is_none());
        assert_eq!(ctx["span_min"], 42);
    }

    #[test]
    fn from_snapshot_rejects_a_snapshot_without_user_text() {
        assert!(from_snapshot(ExperimentProcess::HourReport, "{}").is_err());
        assert!(from_snapshot(ExperimentProcess::HourReport, "not json").is_err());
    }
}
