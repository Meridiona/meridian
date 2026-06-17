//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Route: process ONE (task, hour) — collect the bundle, synthesise (the single
// LLM hop), ground it, and persist a DRAFTED worklog row. This stage never posts
// to Jira: every worklog waits in `drafted` for a human to review/edit/approve
// in the dashboard, after which the `post` sweep is the sole path to real Jira.
// Idempotent: the row UPSERT keyed on (task, day, cycle) replaces a still-DRAFTED
// row on a re-run but leaves an `approved`/`posted` row untouched (see `db.rs`).

use anyhow::Result;
use sqlx::SqlitePool;

use super::config::PmWorklogConfig;
use super::models::{SessionBundle, UpdateState};
use super::{collect, db, ground, synth};

/// One worklog covers exactly one hour, so time logged is capped here.
const WINDOW_SECONDS: i64 = 3600;

/// What happened for one task in one hour.
#[derive(Debug, Clone)]
pub struct TaskOutcome {
    pub task_key: String,
    pub state: UpdateState,
    pub reason: String,
    pub pm_worklog_id: Option<i64>,
}

/// Collect → synthesise → ground → draft one task's worklog for one hour window.
/// A synth failure leaves nothing persisted — the next driver pass retries this
/// hour/task. The drafted row is what the dashboard shows for approval.
///
/// Wrapped in a `worklog_draft` span so this whole draft cycle is one trace in
/// OpenObserve (and the synth call below inherits its traceparent, linking the
/// Python `synthesise_worklog` trace back here). Outcome fields are recorded as
/// the cycle progresses.
#[tracing::instrument(
    name = "worklog_draft",
    skip_all,
    fields(
        task_key = %task_key,
        cycle_index = cycle_index,
        window_start = %hour_start_iso,
        session_count = tracing::field::Empty,
        real_seconds = tracing::field::Empty,
        worklog_state = tracing::field::Empty,
        confidence = tracing::field::Empty,
        coverage = tracing::field::Empty,
        time_spent_seconds = tracing::field::Empty,
        pm_worklog_id = tracing::field::Empty,
    )
)]
pub async fn process_task(
    pool: &SqlitePool,
    cfg: &PmWorklogConfig,
    task_key: &str,
    hour_start_iso: &str,
    hour_end_iso: &str,
    day_utc: &str,
    cycle_index: i64,
) -> Result<TaskOutcome> {
    let span = tracing::Span::current();
    // 1. Collect.
    let bundle = collect::fetch_session_bundle(
        pool,
        task_key,
        hour_start_iso,
        hour_end_iso,
        cycle_index,
        day_utc,
    )
    .await?;

    span.record("session_count", bundle.sessions.len() as i64);
    span.record("real_seconds", bundle.real_seconds);

    if bundle.sessions.is_empty() {
        span.record("worklog_state", "skipped");
        return Ok(TaskOutcome {
            task_key: task_key.to_string(),
            state: UpdateState::Skipped,
            reason: "no classified task sessions in window".to_string(),
            pm_worklog_id: None,
        });
    }

    // Lineage: emit one child span per contributing session under the
    // worklog_draft trace. Each session node reconstructs its classification
    // verdict inline (task/confidence/type, reasoning, category, summary,
    // dimensions — from stored data) AND carries OTel Links to the original
    // CLASSIFICATION (MLX `classify_session`) and FORMATION (ETL) traces for the
    // raw LLM I/O. Combined with the synth's inlined worklog_input/output, the
    // whole worklog reads as one navigable trace.
    let session_ids: Vec<i64> = bundle.sessions.iter().map(|s| s.id).collect();
    let dims = collect::fetch_session_dimensions(pool, &session_ids)
        .await
        .unwrap_or_default();
    record_session_lineage(&bundle, &dims);

    // 2. Synthesise (gated LLM hop). Runs inside this span, so synth.rs picks up
    // the `worklog_draft` traceparent and links the synth trace back here.
    let mut update = synth::synthesise(&bundle, cfg).await?;

    // Authoritative scalars from the bundle — never trust the LLM for these.
    update.task_key = bundle.task_key.clone();
    update.window_start = bundle.window_start.clone();
    update.window_end = bundle.window_end.clone();
    update.cycle_index = cycle_index;
    // time_spent = idle-discounted real_seconds, but capped at the window length:
    // overlapping coding-agent + screen sessions can sum past the hour, and you
    // cannot log more than one real hour in a one-hour window.
    update.time_spent_seconds = bundle.real_seconds.min(WINDOW_SECONDS);

    // 3. Ground.
    let grounded = ground::ground(update, &bundle, cfg.min_confidence);

    // 4. Decide state: an empty summary is unactionable, so skip; else draft.
    let state = if grounded.update.summary.trim().is_empty() {
        UpdateState::Skipped
    } else {
        UpdateState::Drafted
    };

    let (id_min, id_max) = bundle.session_id_bounds();
    let pm_worklog_id =
        db::upsert_pm_worklog(pool, &grounded, state, day_utc, id_min, id_max).await?;

    span.record("worklog_state", state.as_str());
    span.record("confidence", grounded.update.confidence);
    span.record("coverage", grounded.coverage);
    span.record("time_spent_seconds", grounded.update.time_spent_seconds);
    span.record("pm_worklog_id", pm_worklog_id);

    let reason = match state {
        UpdateState::Skipped => "skipped (empty summary after grounding)".to_string(),
        _ => format!(
            "drafted (conf={:.2}, coverage={:.2}, real_s={}) — awaiting UI approval",
            grounded.update.confidence, grounded.coverage, bundle.real_seconds
        ),
    };

    Ok(TaskOutcome {
        task_key: task_key.to_string(),
        state,
        reason,
        pm_worklog_id: Some(pm_worklog_id),
    })
}

/// Emit a `contributing_sessions` span with one `session` child per session in
/// the bundle. Each `session` node:
///   • reconstructs the classification verdict INLINE as child spans
///     (`classification_verdict` / `reasoning` / `category` / `session_summary`
///     / `dimensions`) from stored data — the meaningful classification content,
///     visible without leaving the worklog trace;
///   • carries OTel span Links to the original CLASSIFICATION (`classify_session`)
///     and FORMATION (ETL) traces, where the raw LLM prompt/output live.
/// Purely synchronous (no `.await`) so entered spans never cross an await point.
fn record_session_lineage(
    bundle: &SessionBundle,
    dims: &std::collections::HashMap<i64, std::collections::BTreeMap<String, Vec<String>>>,
) {
    use opentelemetry::KeyValue;
    use tracing_opentelemetry::OpenTelemetrySpanExt;

    let parent = tracing::info_span!(
        "contributing_sessions",
        session_count = bundle.sessions.len() as i64,
    );
    let _parent_enter = parent.enter();

    for s in &bundle.sessions {
        let sess_span = tracing::info_span!(
            "session",
            session_id = s.id,
            app_name = %s.app_name,
            category = s.category.as_deref().unwrap_or("-"),
            session_type = s.task_session_type.as_deref().unwrap_or("-"),
            confidence = s.task_confidence,
            duration_s = s.duration_s,
            started_at = %s.started_at,
            text_source = s.text_source.as_deref().unwrap_or("-"),
            has_classify_trace = s.classify_traceparent.is_some(),
            has_formation_trace = s.formation_traceparent.is_some(),
        );
        let _enter = sess_span.enter();

        // Links to the original full traces (raw LLM input/output live there).
        if let Some(sc) = s
            .classify_traceparent
            .as_deref()
            .and_then(crate::observability::span_context_from_traceparent)
        {
            sess_span.add_link_with_attributes(
                sc,
                vec![
                    KeyValue::new("link.kind", "classification"),
                    KeyValue::new("session_id", s.id),
                ],
            );
        }
        if let Some(sc) = s
            .formation_traceparent
            .as_deref()
            .and_then(crate::observability::span_context_from_traceparent)
        {
            sess_span.add_link_with_attributes(
                sc,
                vec![
                    KeyValue::new("link.kind", "formation"),
                    KeyValue::new("session_id", s.id),
                ],
            );
        }

        // Verdict + content as inline child spans (each in its own scope so they
        // are SIBLINGS under `session`, not nested in each other).
        {
            let _v = tracing::info_span!(
                "classification_verdict",
                task_key = %bundle.task_key,
                confidence = s.task_confidence,
                session_type = s.task_session_type.as_deref().unwrap_or("-"),
                category = s.category.as_deref().unwrap_or("-"),
            )
            .entered();
        }
        if let Some(reason) = s.task_reasoning.as_deref().filter(|t| !t.is_empty()) {
            let _r = tracing::info_span!("reasoning", value = %reason, chars = reason.len() as i64)
                .entered();
        }
        if let Some(expl) = s.category_explanation.as_deref().filter(|t| !t.is_empty()) {
            let _c = tracing::info_span!(
                "category",
                category = %s.category.as_deref().unwrap_or("-"),
                explanation = %expl,
                chars = expl.len() as i64,
            )
            .entered();
        }
        // Prefer the full session_summary; fall back to the excerpt the synth saw.
        let summary = s
            .session_summary
            .as_deref()
            .filter(|t| !t.is_empty())
            .unwrap_or(s.excerpt.as_str());
        if !summary.is_empty() {
            let _s =
                tracing::info_span!("session_summary", value = %summary, chars = summary.len() as i64)
                    .entered();
        }
        if let Some(dmap) = dims.get(&s.id).filter(|m| !m.is_empty()) {
            let json = serde_json::to_string(dmap).unwrap_or_default();
            let keys = dmap.keys().cloned().collect::<Vec<_>>().join(", ");
            let _d = tracing::info_span!("dimensions", value = %json, keys = %keys).entered();
        }
    }
}
