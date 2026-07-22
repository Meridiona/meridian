//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! The Workstream Builder — fold one hour's activity report into the running set of
//! day-level tasks (workstreams) shown on the timeline.
//!
//! Anchored incremental fold (the validated design): the current tasks are handed
//! to the model as DATA — stable ANCHORS with their full title, summary, and
//! segments — and the model returns only THIS hour's placements (match an existing
//! anchor or open a new task), never a rewrite of the whole set. Code then folds
//! those placements onto the prior state ([`super::workstream_sanitize`]), so a bad
//! or empty answer can only fail to add, never reshuffle or drop earlier hours.
//! **The model owns judgement** — which task this hour's work belongs to, when to
//! open a new one, what counts as work vs. leisure, how to group this hour's time
//! into readable segments, and how to tell each task's short story. The one task it
//! places into gets its summary **rewritten** into a tight 3–6 bullet whole-story
//! arc (not a growing per-hour log); untouched tasks pass through verbatim. **Code
//! owns plumbing** — parse ([`super::workstream_parse`]), fold
//! ([`super::workstream_sanitize`]), and code-owned time
//! ([`super::segment`]/[`super::workstream_state`]): a task's `minutes` and `hours`
//! are always derived from its segments, never from the model.
//!
//! Idempotency: if a prior segment STARTS inside this hour, the fold is a no-op
//! ([`hour_already_folded`]); re-applying an hour is also a no-op through segment
//! coalescing (and it re-writes the same bounded story). Safety: an unparseable or
//! empty answer carries the prior state forward untouched, and an empty summary never
//! blanks a task's story.
//!
//! This is thin orchestration; the real work lives in the sibling modules:
//! [`super::segment`], [`super::workstream_parse`], [`super::workstream_sanitize`],
//! [`super::workstream_state`].

use anyhow::Result;
use chrono::Utc;
use sqlx::SqlitePool;
use tracing::Instrument;

use crate::llm::{self, prompts, PromptRequest};

use super::task_db;
use super::workstream_parse::parse_placements;
use super::workstream_sanitize::apply_placements;
use super::workstream_state::{build_state_json, to_rows};

/// The corrected task set is a short JSON object; 2048 tokens is ample for a handful
/// of tasks each with a few summary lines and their segments. `pub(crate)` so the
/// LLM-Lab replay ([`crate::llm_experiment`]) rebuilds the identical request contract.
pub(crate) const WORKSTREAM_MAX_TOKENS: u32 = 2048;

/// The fold's exact [`PromptRequest`] — extracted from [`run`] so the LLM-Lab replay
/// ([`crate::llm_experiment`]) fans the byte-identical request across arbitrary
/// providers. `state_json` is [`super::workstream_state::build_state_json`] over the
/// prior tasks; `report` is the hour's activity report.
pub(crate) fn workstream_request(
    state_json: &str,
    hour_label: &str,
    report: &str,
) -> PromptRequest {
    let user = format!(
        "=== CURRENT TASKS (anchors from earlier hours - match this hour's work to these by their title and summary; rewrite the whole-story summary of any task you place work into, and leave every other task exactly as it is) ===\n\
         {state_json}\n\n\
         === NEW ACTIVITY - HOUR {hour_label} (place this hour's work only) ===\n{report}"
    );
    PromptRequest {
        system: prompts::WORKSTREAM,
        user,
        schema: Some(prompts::workstream_schema()),
        max_tokens: WORKSTREAM_MAX_TOKENS,
        label: format!("workstream {hour_label}"),
    }
}

/// Does a prior segment **start** inside `hour_label` (`YYYY-MM-DDTHH`, local)? One of the
/// two idempotency signals in [`run`] (the other is [`task_db::hour_folded_marker_exists`]).
///
/// Judged by segment starts — NOT by `DayTaskRow::hours` (`hours_json`), which is derived
/// from segments *touching* an hour: a segment that merely spills past the boundary
/// (`14:44-15:02`, `16:20-17:01`) marks the NEXT hour as touched, and guarding on that
/// skipped the next hour's real fold, silently dropping its work (observed live twice on
/// 2026-07-16, hours 15 and 17). A fold's own placements start inside the hour being
/// folded, so segment starts are the honest signal.
///
/// Its blind spot — a folded hour whose only segment coalesced backward into the prior
/// hour, so nothing starts in this hour's window — is exactly what the marker-row signal
/// covers; the two are OR'd. An unparseable hour suffix fails open (fold runs — the merge
/// is retry-safe) rather than risking a silent drop.
fn hour_already_folded(prior: &[task_db::DayTaskRow], hour_label: &str) -> bool {
    let Some(hh) = hour_label
        .rsplit('T')
        .next()
        .and_then(|h| h.parse::<i64>().ok())
    else {
        return false;
    };
    let (lo, hi) = (hh * 60, (hh + 1) * 60);
    prior
        .iter()
        .flat_map(|t| &t.segments)
        .any(|s| s.start_min >= lo && s.start_min < hi)
}

/// Fold the hour's report into the day's tasks. `day_local` is `YYYY-MM-DD`,
/// `hour_label` is `YYYY-MM-DDTHH` (local). `report` is the hour's human-readable
/// activity report (the `HH:MM-HH:MM  N min  …` lines from the first call).
///
/// Returns `Err` only on a real fault (the fold provider is down with no fallback),
/// so the driver leaves the hour pending and retries — the idempotency guard keeps
/// that retry from double-counting. An empty report, an unparseable answer, or an
/// answer that would wipe the day is a clean no-op that preserves prior state.
pub async fn run(pool: &SqlitePool, day_local: &str, hour_label: &str, report: &str) -> Result<()> {
    let prior = task_db::fetch_state(pool, day_local).await;

    // Idempotency: this hour is already folded in — nothing to do. Two independent signals,
    // OR'd so either alone suffices:
    //   1. a per-hour marker row (written after this hour's fold succeeds) — geometry-
    //      independent, so it survives segment coalescing that moves a folded hour's
    //      segment start back into the prior hour;
    //   2. a segment *starting* inside this hour — covers the narrow window where a fold
    //      completed but the daemon died before the marker write (retry finds the segment).
    if hour_already_folded(&prior, hour_label)
        || task_db::hour_folded_marker_exists(pool, day_local, hour_label).await
    {
        tracing::info!(
            hour = hour_label,
            "worklog: hour already in workstreams — build skipped"
        );
        return Ok(());
    }
    if report.trim().is_empty() {
        tracing::debug!(
            hour = hour_label,
            "worklog: empty report — no day-task fold"
        );
        return Ok(());
    }

    let span = tracing::info_span!(
        "worklog.workstream.build",
        hour = hour_label,
        llm_provider = tracing::field::Empty,
        n_tasks = tracing::field::Empty,
        n_segments = tracing::field::Empty
    );

    async {
        let state_json = build_state_json(&prior);
        let req = workstream_request(&state_json, hour_label, report);

        let (out, provider) = llm::complete(&req)
            .await
            .map_err(|e| anyhow::anyhow!("day-task fold failed: {e}"))?;
        tracing::Span::current().record("llm_provider", provider.as_str());

        // This hour's placements: valid entries are folded onto the prior state.
        // An unparseable or empty answer means "nothing to place this hour" — the
        // fold then leaves the prior tasks exactly as they were.
        let placements = match parse_placements(&out.text) {
            Some(p) if !p.is_empty() => p,
            _ => {
                tracing::warn!(
                    hour = hour_label,
                    "worklog: no usable placements this hour — prior state kept"
                );
                Vec::new()
            }
        };

        let sanitized = apply_placements(placements, &prior);

        // The working set starts from prior, so it is only empty when the day was
        // already empty and this hour placed nothing — nothing to write.
        if sanitized.is_empty() {
            tracing::debug!(hour = hour_label, "worklog: no tasks to write this hour");
            return Ok(());
        }

        let n_segments: usize = sanitized.iter().map(|t| t.segments.len()).sum();
        tracing::Span::current().record("n_tasks", sanitized.len());
        tracing::Span::current().record("n_segments", n_segments);

        let now = Utc::now().to_rfc3339();
        let rows = to_rows(&sanitized, day_local, &prior, &now);
        task_db::replace_day_tasks(pool, day_local, &rows, &now).await?;

        // Re-apply the user's dismiss/merge corrections onto the freshly-rewritten
        // day: the fold re-derives grouping from scratch, so without this it would
        // resurrect a dismissed workstream or re-split a merge. Best-effort - a
        // reconcile failure must never fail the hour's fold.
        if let Err(e) =
            meridian_core::day_task_corrections::reconcile(pool, day_local, &now).await
        {
            tracing::warn!(day = day_local, error = %e, "worklog: day-task correction reconcile failed after fold");
        }

        tracing::info!(
            hour = hour_label,
            n_tasks = rows.len(),
            n_segments,
            provider = provider.as_str(),
            "worklog: workstreams built"
        );
        Ok::<(), anyhow::Error>(())
    }
    .instrument(span)
    .await
}

#[cfg(test)]
mod tests {
    use super::super::segment::Segment;
    use super::*;
    use crate::worklog_pipeline::task_db::DayTaskRow;

    fn row_with_segments(segments: Vec<Segment>) -> DayTaskRow {
        DayTaskRow {
            task_id: "T1".into(),
            title: "A task".into(),
            summary: "Did things".into(),
            // Deliberately what hours_touched would derive — the guard must NOT read it.
            hours: vec!["2026-07-16T16".into(), "2026-07-16T17".into()],
            segments,
            minutes: 41,
            status: "active".into(),
            linked_ticket: None,
            created_at: "2026-07-16T10:00:00Z".into(),
        }
    }

    fn seg(start_min: i64, end_min: i64) -> Segment {
        Segment { start_min, end_min }
    }

    /// The live 5-6pm loss on 2026-07-16: a 16:20-17:01 segment spills one minute past
    /// 17:00, so hours_json contains T17 — but nothing was ever folded FOR hour 17.
    /// The guard must let hour 17 fold, while still recognising hour 16 as folded.
    #[test]
    fn a_segment_spilling_past_the_boundary_does_not_mark_the_next_hour_folded() {
        let prior = vec![row_with_segments(vec![seg(16 * 60 + 20, 17 * 60 + 1)])];
        assert!(!hour_already_folded(&prior, "2026-07-16T17"));
        assert!(hour_already_folded(&prior, "2026-07-16T16"));
    }

    /// The live 3-4pm loss (same day, same bug): T3's 14:44-15:02 segment from the
    /// hour-14 fold skipped the hour-15 fold entirely.
    #[test]
    fn the_hour_14_spill_regression_does_not_block_hour_15() {
        let prior = vec![row_with_segments(vec![seg(14 * 60 + 44, 15 * 60 + 2)])];
        assert!(!hour_already_folded(&prior, "2026-07-16T15"));
        assert!(hour_already_folded(&prior, "2026-07-16T14"));
    }

    /// A segment genuinely starting inside the hour IS proof the hour folded.
    #[test]
    fn a_segment_starting_in_the_hour_marks_it_folded() {
        let prior = vec![row_with_segments(vec![seg(17 * 60, 17 * 60 + 58)])];
        assert!(hour_already_folded(&prior, "2026-07-16T17"));
    }

    /// No prior tasks, or an unparseable hour label, must fail OPEN (fold runs) —
    /// the merge is retry-safe, a skipped fold is silent data loss.
    #[test]
    fn empty_state_and_garbage_labels_fail_open() {
        assert!(!hour_already_folded(&[], "2026-07-16T17"));
        let prior = vec![row_with_segments(vec![seg(0, 60)])];
        assert!(!hour_already_folded(&prior, "not-an-hour-label"));
    }

    /// The coalescing blind spot the marker signal exists to cover: hour 15's fold extended
    /// a task across the 15:00 boundary and `segment::normalize` merged it back into hour
    /// 14's segment (14:44-15:02 + 15:02-15:30 -> 14:44-15:30), keeping the earlier start.
    /// Nothing now STARTS in hour 15, so the geometry check alone reports "not folded" and
    /// would re-fold hour 15 on any retry. This pins that gap; `run`'s marker-row check
    /// (`task_db::hour_folded_marker_exists`, DB-backed) is what actually closes it.
    #[test]
    fn a_coalesced_segment_defeats_the_start_check_alone() {
        let prior = vec![row_with_segments(vec![seg(14 * 60 + 44, 15 * 60 + 30)])];
        assert!(
            !hour_already_folded(&prior, "2026-07-16T15"),
            "documents the geometry blind spot the marker row covers"
        );
    }
}
