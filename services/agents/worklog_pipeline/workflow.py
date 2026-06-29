"""Agno Workflow wrapping the worklog pipeline stages.

The pipeline runs as a real agno ``Workflow`` whose CONTROL FLOW is expressed in
agno primitives (not plain-Python branching), so the tier path is first-class in
the trace:

    Step distill → Step report → Step candidates
      → Condition(daily plan exists)        → Step match_tier1
      → Condition(tier-1 found nothing)     → Loop(backlog batches) → Step match_tier2_batch
      → Condition(hour produced a report)   → Router(bindings?)
                                                ├─ Loop(per binding) → Step draft_one
                                                └─ Step propose

``Parallel`` is deliberately NOT used: the machine holds one model at a time
(embedder / reranker / 2B share a single slot), so concurrent model steps would
violate that and thrash. Every step shares one per-run ``HourContext`` closure;
the Condition/Loop/Router predicates read that ctx. Persisted to a SqliteDb for
run history. Draft→approve stays the EXISTING pm_worklogs UI flow (steps write
`state='drafted'` rows), so the workflow runs to completion rather than pausing.
"""
from __future__ import annotations

import json
import logging
import math
from pathlib import Path

from agents.worklog_pipeline.match import BATCH
from agents.worklog_pipeline.pipeline import (
    HourContext, HourResult, _DEFAULT_SERVER,
    stage_candidates, stage_distill, stage_draft_one, stage_match_tier1,
    stage_match_tier2_batch, stage_propose, stage_report,
)

log = logging.getLogger("meridian.worklog.workflow")


def _wf_db_file(meridian_db_path: str) -> str:
    """Sidecar SqliteDb for agno run history, next to meridian.db."""
    return str(Path(meridian_db_path).expanduser().parent / "worklog_workflow.db")


def run_hour_workflow(
    hour: str,
    *,
    db_path: str,
    server_url: str = _DEFAULT_SERVER,
    cycle_index: int | None = None,
    dry_run: bool = False,
    max_backlog: int = 15,
    run_id: str | None = None,
    traceparent: str | None = None,
    wf_db_file: str | None = None,
) -> HourResult:
    """Run the worklog pipeline for one hour as an agno Workflow. Returns the
    HourResult (also the content of the final step)."""
    from agno.db.sqlite import SqliteDb
    from agno.workflow import Condition, Loop, Router, Step, Workflow
    from agno.workflow.types import OnError, StepInput, StepOutput
    from opentelemetry import context as _otel_ctx

    from agents import observability

    # This runs in a threadpool thread (run_in_threadpool), which does NOT inherit
    # the worklog.hour span context. Re-attach it from the propagated traceparent
    # so the agno Workflow/Agent spans AND the manual stage spans nest under the
    # root rather than starting fresh roots.
    _parent_ctx = observability.extract_parent_context(traceparent)
    _ctx_token = _otel_ctx.attach(_parent_ctx) if _parent_ctx is not None else None

    ctx = HourContext(
        hour=hour, db_path=db_path, server_url=server_url,
        cycle_index=cycle_index or 0, dry_run=dry_run, max_backlog=max_backlog,
        run_id=run_id, traceparent=traceparent,
    )

    def _stopped() -> bool:
        return bool(ctx.result.note)

    # Retry policy — AGNO-NATIVE, not raw-python. agno's Step.max_retries re-runs
    # a step ONLY when its executor RAISES (no backoff). So executors here do NOT
    # catch exceptions: a transient failure (model timeout, connection reset, bad
    # JSON from the OpenAI client) propagates and agno retries it _RETRIES times.
    # Deterministic non-errors (no sessions, an unparseable-but-returned draft) are
    # NOT exceptions — they return normally and are handled in-band, so we never
    # burn retries re-running something that will fail identically.
    _RETRIES = 2

    # ── Step executors (one unit of work each) ──────────────────────────────
    # `if _stopped()` short-circuits a step when an earlier one set a terminal note
    # (e.g. "no sessions"); it is a control signal, NOT exception-swallowing.
    def s_distill(_si: StepInput) -> StepOutput:
        # No sessions → stop cleanly (return, no raise → no retry). A real fault
        # (e.g. /distill_hour timeout) raises → agno retries.
        ok = stage_distill(ctx)
        return StepOutput(content=f"distilled {ctx.result.nsess} sessions", stop=not ok)

    def s_report(_si: StepInput) -> StepOutput:
        if _stopped():
            return StepOutput(content="skipped", stop=True)
        stage_report(ctx)
        return StepOutput(content=f"report {ctx.result.report_chars} chars")

    def s_candidates(_si: StepInput) -> StepOutput:
        if _stopped():
            return StepOutput(content="skipped", stop=True)
        stage_candidates(ctx)
        return StepOutput(content=f"daily={len(ctx.daily)} backlog={len(ctx.backlog)}")

    def s_match_tier1(_si: StepInput) -> StepOutput:
        stage_match_tier1(ctx)
        return StepOutput(content=f"tier1 matched={len(ctx._outcome.bindings)}")

    def s_match_tier2_batch(_si: StepInput) -> StepOutput:
        stage_match_tier2_batch(ctx)
        return StepOutput(content=f"tier2 batch@{ctx._batch_i} matched={len(ctx._outcome.bindings)}")

    def s_draft_one(_si: StepInput) -> StepOutput:
        stage_draft_one(ctx)
        return StepOutput(content=f"drafted {ctx._draft_i}/{len(ctx._outcome.bindings)}")

    def s_propose(_si: StepInput) -> StepOutput:
        stage_propose(ctx)
        return StepOutput(content=json.dumps(ctx.result.as_dict()))

    # Steps that must complete for the hour to be meaningful (distill/report/
    # candidates/match) fail the hour after retries (skip_on_failure=False → agno
    # re-raises → /worklog_hour 500 → the Rust driver retries the WHOLE hour on its
    # next pass — a second, coarser retry tier). The finalize steps (draft/propose)
    # use skip_on_failure=True so one bad draft is recorded and skipped rather than
    # blocking the hour forever (a deterministic truncation would otherwise loop).
    def _step(name, executor, *, skip_on_failure: bool = False):
        return Step(name=name, executor=executor,
                    max_retries=_RETRIES, skip_on_failure=skip_on_failure)

    # ── agno control-flow predicates (read the shared ctx closure) ──────────
    def _has_daily(_si: StepInput) -> bool:
        return bool(ctx.daily) and not _stopped()

    def _tier1_unmatched(_si: StepInput) -> bool:
        return (not ctx._outcome.bindings) and not _stopped()

    def _tier2_done(_outputs) -> bool:
        # Stop the backlog Loop once we have a hit or have scanned everything.
        return bool(ctx._outcome.bindings) or ctx._batch_i >= len(ctx.backlog)

    def _active(_si: StepInput) -> bool:
        return not _stopped()

    def _drafts_done(_outputs) -> bool:
        return ctx._draft_i >= len(ctx._outcome.bindings)

    # ── Branch constructs ───────────────────────────────────────────────────
    # Tier-2 backlog scan: repeat one batch step until hit / exhausted.
    tier2_loop = Loop(
        name="tier2_scan",
        steps=[_step("match_tier2_batch", s_match_tier2_batch)],
        end_condition=_tier2_done,
        max_iterations=max(1, math.ceil((max_backlog or 0) / BATCH) + 1),
    )
    # Draft branch: one worklog per matched binding. The cursor advances only on
    # success (see stage_draft_one), so a transient raise leaves the cursor put →
    # agno retries the SAME binding; a persistent raise propagates out of the Loop
    # (no infinite re-loop). A returned-but-unparsed draft is NOT a raise, so it is
    # recorded and the cursor moves on — no wasted retries on deterministic output.
    draft_loop = Loop(
        name="draft_each",
        steps=[_step("draft_one", s_draft_one)],
        end_condition=_drafts_done,
        max_iterations=8,  # an hour rarely binds >8 tasks; end_condition stops earlier
    )
    propose_step = _step("propose", s_propose)

    def _select_finalize(_si: StepInput):
        # bindings present → draft each; otherwise propose a new ticket.
        return [draft_loop] if ctx._outcome.bindings else [propose_step]

    workflow = Workflow(
        name="worklog_hour",
        db=SqliteDb(
            db_file=wf_db_file or _wf_db_file(db_path),
            session_table="worklog_workflow_sessions",
        ),
        steps=[
            _step("distill", s_distill),
            _step("report", s_report),
            _step("candidates", s_candidates),
            # Tier 1 — only when a confirmed daily plan exists. on_error=fail so a
            # match that still errors after retries fails the hour (driver retries)
            # rather than being silently skipped past.
            Condition(name="tier1_if_daily", evaluator=_has_daily,
                      steps=[_step("match_tier1", s_match_tier1)],
                      on_error=OnError.fail),
            # Tier 2 — only when tier 1 found nothing: scan backlog in batches.
            Condition(name="tier2_if_unmatched", evaluator=_tier1_unmatched,
                      steps=[tier2_loop], on_error=OnError.fail),
            # Finalize (draft-per-task OR propose) — only when the hour produced
            # a report (i.e. not stopped at distill/report). on_error=fail so a
            # draft/propose that still errors after retries fails the hour (driver
            # retries) rather than being silently skipped.
            Condition(name="finalize_if_active", evaluator=_active,
                      steps=[Router(name="draft_or_propose",
                                    selector=_select_finalize,
                                    choices=[draft_loop, propose_step])],
                      on_error=OnError.fail),
        ],
    )

    try:
        # session_id groups every hour-run of the same day into ONE AgentOS
        # session (the Traces/Sessions views group by session_id) — agno stamps
        # it onto the run's root span, so the trace surfaces under that session.
        workflow.run(input=hour, run_id=run_id, session_id=f"wl-{hour[:10]}")
    except Exception:  # noqa: BLE001
        log.exception("worklog workflow failed for hour=%s", hour)
        raise
    finally:
        if _ctx_token is not None:
            _otel_ctx.detach(_ctx_token)
    return ctx.result
