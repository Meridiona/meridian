"""Agno Workflow wrapping the worklog pipeline stages.

The pipeline runs as a real agno ``Workflow`` of sequential ``Step``s, each a thin
wrapper over a plain-Python stage that shares one per-run ``HourContext``:

    Step distill → Step report → Step candidates → Step classify
      → Step propose   (only if <2 matched; may abstain)
      → Step generate  (one worklog per matched + proposed ticket → persist)

The branching that used to live in agno Condition/Loop/Router now lives inside the
stages (classify_hour does tier-1/tier-2 internally; stage_propose self-guards and
may abstain; stage_generate drafts every ticket and records "nothing worth logging"
for an empty hour) — simpler control flow, same trace coverage via the manual spans.

``Parallel`` is deliberately NOT used: the machine holds one model at a time
(embedder / reranker / 2B share a single slot), so concurrent model steps would
violate that and thrash. Persisted to a SqliteDb for run history. Draft→approve
stays the EXISTING pm_worklogs UI flow (steps write `state='drafted'` rows), so the
workflow runs to completion rather than pausing.
"""
from __future__ import annotations

import json
import logging
from pathlib import Path

from agents.worklog_pipeline.pipeline import (
    HourContext, HourResult, _DEFAULT_SERVER,
    stage_candidates, stage_classify, stage_distill, stage_generate,
    stage_propose, stage_report,
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
    from agno.workflow import Step, Workflow
    from agno.workflow.types import StepInput, StepOutput
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

    def s_classify(_si: StepInput) -> StepOutput:
        if _stopped():
            return StepOutput(content="skipped", stop=True)
        stage_classify(ctx)
        return StepOutput(content=f"matched={len(ctx._outcome.bindings)} tier={ctx._outcome.tier_used}")

    def s_propose(_si: StepInput) -> StepOutput:
        # Only when fewer than two tickets matched (stage_propose self-guards);
        # may abstain, leaving ctx._proposed None.
        if _stopped():
            return StepOutput(content="skipped", stop=True)
        stage_propose(ctx)
        return StepOutput(content=f"proposed={'yes' if ctx._proposed else 'no'}")

    def s_generate(_si: StepInput) -> StepOutput:
        # One /generate_worklog call per matched ticket AND the proposed ticket,
        # then persist. An empty outcome records "nothing worth logging".
        if _stopped():
            return StepOutput(content="skipped", stop=True)
        stage_generate(ctx)
        return StepOutput(content=json.dumps(ctx.result.as_dict()))

    # Every step must complete for the hour to be meaningful: skip_on_failure=False
    # → agno re-raises after retries → /worklog_hour 500 → the Rust driver retries
    # the WHOLE hour on its next pass (a second, coarser retry tier). Generation
    # faults are caught inside generation.py (return None, no draft) rather than
    # raised, so a model hiccup degrades to fewer drafts instead of failing the hour.
    def _step(name, executor):
        return Step(name=name, executor=executor, max_retries=_RETRIES)

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
            # Single classify step: classify_hour runs tier-1 then tier-2 batches
            # internally until >= 2 matches or backlog exhausted.
            _step("classify", s_classify),
            # Propose then generate — plain-Python stages (no Router/Loop). propose
            # decides a new Task/Bug or abstains; generate drafts every ticket
            # (matched + proposed) and persists. Both no-op cleanly on a stopped hour.
            _step("propose", s_propose),
            _step("generate", s_generate),
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
