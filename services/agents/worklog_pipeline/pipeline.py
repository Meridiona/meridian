"""End-to-end worklog pipeline for one hour, as composable stages.

    distil_hour → activity report → reranker hint → tiered match →
    propose (if <2 matched, may abstain) → worklog draft per ticket → persist

The work is split into stages that share a single ``HourContext``; ``workflow.py``
wraps the stage functions as agno ``Step``s (the single orchestration path) for run
persistence + the agno-native structure, entered via ``run_hour_workflow``.
Every model call goes through the MLX server's HTTP endpoints (classify /
propose_ticket / generate_worklog), so only one model is ever resident and there
are no agno generation-agents — the stages POST to the server like the classifier.
"""
from __future__ import annotations

import logging
import time
from dataclasses import dataclass, field

from opentelemetry import trace

from agents import observability
from agents.time_utils import local_hour_utc_bounds
from agents.worklog_pipeline import db as wdb
from agents.worklog_pipeline import generation
from agents.worklog_pipeline.classifier import (
    Candidate, ClassificationOutcome, classify_hour,
)
from agents.worklog_pipeline.generation import _post  # canonical JSON transport
from agents.worklog_pipeline.models import ProposedTicket
log = logging.getLogger("meridian.worklog.pipeline")
tracer = trace.get_tracer("meridian.worklog.pipeline")

_DEFAULT_SERVER = "http://127.0.0.1:7823"

# Per-endpoint transport timeouts. The fast, non-generative calls (embedding
# distill, rerank) cap at 120s; the generative /activity_report gets the full
# generation budget (matches generation._post's 300s default) so a slow model
# doesn't time it out while the downstream /generate_worklog would not.
_FAST_TIMEOUT = 120.0


@dataclass
class HourResult:
    hour:         str
    day_local:    str
    nsess:        int = 0
    report_chars: int = 0
    tier_used:    int = 0
    matched:      list[dict] = field(default_factory=list)   # [{task_key, confidence, why, summary}]
    proposed:     dict | None = None
    worklog_ids:  list[int] = field(default_factory=list)
    proposed_id:  int | None = None
    note:         str = ""

    def as_dict(self) -> dict:
        return {
            "hour": self.hour, "day_local": self.day_local, "nsess": self.nsess,
            "report_chars": self.report_chars, "tier_used": self.tier_used,
            "matched": self.matched, "proposed": self.proposed,
            "worklog_ids": self.worklog_ids, "proposed_id": self.proposed_id,
            "note": self.note,
        }


@dataclass
class HourContext:
    hour:        str
    db_path:     str
    server_url:  str = _DEFAULT_SERVER
    cycle_index: int = 0
    dry_run:     bool = False
    max_backlog: int = 15
    run_id:      str | None = None
    traceparent: str | None = None
    # filled by stages
    body:         str = ""   # distilled OCR/app body (coding rows excluded)
    coding_block: str = ""   # verbatim agent summaries for coding sessions this hour
    report:       str = ""   # activity report + coding_block (what match/worklog see)
    daily:   list[Candidate] = field(default_factory=list)
    backlog: list[Candidate] = field(default_factory=list)
    result:  HourResult | None = None

    def __post_init__(self) -> None:
        self.day_local = self.hour[:10]  # local date from local-hour label
        if not self.cycle_index:
            self.cycle_index = int(self.hour[11:13])
        if self.result is None:
            self.result = HourResult(hour=self.hour, day_local=self.day_local)
        # Internal cross-stage state. Initialised here (not attached dynamically)
        # so a stage that runs without its predecessor never raises AttributeError
        # — it just sees the empty default and produces nothing.
        self._open_tasks: dict = {}
        self._outcome: ClassificationOutcome = ClassificationOutcome()
        self._proposed: ProposedTicket | None = None  # set by stage_propose when a new ticket is drafted
        self._draft_i: int = 0   # draft cursor (advanced per draft Loop iteration)


# ── Stages ──────────────────────────────────────────────────────────────────────

def _parent(ctx: HourContext):
    """Parent context for a stage's TOP span = the ``worklog.hour`` span.

    The agno Workflow runs each Step in an execution context where the
    workflow-level ``otel_ctx.attach`` (in workflow.py) does NOT reach, so without
    this each stage span would start as its own root. Anchoring explicitly to the
    hour's traceparent (carried on ``ctx.traceparent``) makes every stage a direct
    child of ``worklog.hour`` — one connected trace. Returns None on the eval/test
    path (no traceparent) → a fresh root, which is correct there.
    """
    return observability.extract_parent_context(ctx.traceparent)

def _format_coding_block(coding: list[dict]) -> str:
    """Render coding-agent summaries as a LABELED input section for the report LLM.

    Each session becomes one labeled block carrying its own agent-written summary.
    This is fed INTO the activity_report input (not appended after), with a header
    that tells the model these are the developer's own coding-agent sessions to be
    consolidated into the single activity story — the report prompt is responsible
    for preserving the ticket keys / file names the matcher needs.
    """
    if not coding:
        return ""
    parts = ["## CODING-AGENT SESSIONS THIS HOUR "
             "(the developer drove these AI coding agents — treat as their own work "
             "and weave into the activity summary; keep ticket keys and file names)"]
    for c in coding:
        hhmm = (c.get("started_at") or "")[11:16]
        app = c.get("app_name") or "Coding agent"
        tk = f" · {c['task_key']}" if c.get("task_key") else ""
        summary = (c.get("session_summary") or "").strip()
        parts.append(f"\n[{hhmm} · {app}{tk}]\n{summary}")
    return "\n".join(parts)


def stage_distill(ctx: HourContext) -> bool:
    """Distil the hour's OCR/app sessions and gather coding-agent summaries.

    Returns False (stop) only if the hour has NO activity at all — neither
    distillable OCR sessions nor any summarised coding session.
    """
    with tracer.start_as_current_span("worklog.sessions", context=_parent(ctx)) as sess_span:
        _t0 = time.monotonic()
        # Pass this span's traceparent so distill_hour loopback nests under it.
        tp = observability.current_traceparent()
        d = _post(ctx.server_url, "/distill_hour",
                  {"hour": ctx.hour, "db_path": ctx.db_path, "traceparent": tp},
                  timeout=_FAST_TIMEOUT)
        ctx.body = d["body"]
        ocr_nsess = d["nsess"]

        conn = wdb.open_db(ctx.db_path)
        try:
            coding = wdb.fetch_coding_summaries(conn, ctx.hour)
            ocr_sessions = wdb.fetch_sessions_for_hour(conn, ctx.hour)
        finally:
            conn.close()
        ctx.coding_block = _format_coding_block(coding)
        ctx.result.nsess = ocr_nsess + len(coding)

        # Roll up the named sessions that fed this hour so the parent span alone
        # tells you WHAT was distilled (the per-session children carry the detail).
        ocr_apps = [s.get("app_name") or "?" for s in ocr_sessions]
        coding_apps = [c.get("app_name") or "?" for c in coding]
        sess_span.set_attribute("ocr_nsess", ocr_nsess)
        sess_span.set_attribute("coding_nsess", len(coding))
        sess_span.set_attribute("total_nsess", ctx.result.nsess)
        sess_span.set_attribute("ocr_session_apps", ", ".join(ocr_apps))
        sess_span.set_attribute("coding_session_apps", ", ".join(coding_apps))
        sess_span.set_attribute("body_chars", len(ctx.body))
        sess_span.set_attribute("distil_input_tokens_est", int(d.get("raw_chars", 0)) // 4)
        sess_span.set_attribute("distil_output_tokens_est", int(d.get("out_chars", 0)) // 4)
        sess_span.set_attribute("distil_elapsed_s", d.get("elapsed_s", 0.0))
        sess_span.set_attribute("elapsed_s", round(time.monotonic() - _t0, 2))

        # Child span per OCR session (metadata only — no LLM text)
        for s in ocr_sessions:
            with tracer.start_as_current_span("worklog.session.ocr") as sp:
                sp.set_attribute("app_name", s.get("app_name") or "")
                sp.set_attribute("started_at", s.get("started_at") or "")
                sp.set_attribute("duration_s", s.get("duration_s") or 0)

        # Child span per coding session — summary visible as Output panel in OO
        for c in coding:
            with tracer.start_as_current_span("worklog.session.coding") as sp:
                sp.set_attribute("app_name", c.get("app_name") or "")
                sp.set_attribute("task_key", c.get("task_key") or "")
                sp.set_attribute("started_at", c.get("started_at") or "")
                sp.set_attribute("llm_output",
                    observability.preview(c.get("session_summary") or "", max_chars=4000))

        if not ctx.body.strip() and not ctx.coding_block:
            ctx.result.note = "no sessions"
            log.info("worklog: hour=%s has no sessions — skipping", ctx.hour,
                     extra={"hour": ctx.hour})
            return False
        log.info("worklog: hour=%s distilled ocr_nsess=%d coding_nsess=%d body_chars=%d",
                 ctx.hour, ocr_nsess, len(coding), len(ctx.body),
                 extra={"hour": ctx.hour, "ocr_nsess": ocr_nsess,
                        "coding_nsess": len(coding), "body_chars": len(ctx.body)})
        return True


def stage_report(ctx: HourContext) -> None:
    """Build the ONE consolidated activity summary the matcher/worklog see.

    Both signals for the hour go into the SAME activity_report LLM call: the
    distilled OCR/app body AND the coding-agent session summaries (labeled in the
    input). The report prompt weaves them into a single story and is told to
    preserve concrete identifiers (ticket keys, file paths) so the matcher keeps
    its recall — coding work is consolidated, never a separate tacked-on section.
    """
    with tracer.start_as_current_span("worklog.report", context=_parent(ctx)) as rep_span:
        _t0 = time.monotonic()
        # Pass this span's traceparent so activity_report child spans nest here.
        tp = observability.current_traceparent()
        # ONE consolidated input = distilled OCR/app body + the labeled coding-agent
        # session summaries, so the report LLM weaves coding work into a single story
        # (the prompt preserves ticket keys / file paths for the matcher).
        report_input = ctx.body
        if ctx.coding_block:
            report_input = f"{ctx.body}\n\n{ctx.coding_block}".strip()
        if report_input.strip():
            rep = _post(ctx.server_url, "/activity_report",
                        {"body": report_input, "label": ctx.hour, "traceparent": tp})
            ctx.report = rep["report"]
            # Roll up the activity_report LLM metrics so the stage span alone shows
            # tokens + time (the activity_report child carries the full detail).
            rep_span.set_attribute("input_tokens", rep.get("input_tokens", 0))
            rep_span.set_attribute("output_tokens", rep.get("output_tokens", 0))
            rep_span.set_attribute("think_tokens", rep.get("think_tokens", 0))
            rep_span.set_attribute("report_elapsed_s", rep.get("elapsed_s", 0.0))
        else:
            ctx.report = ""

        rep_span.set_attribute("input_chars", len(report_input))
        ctx.result.report_chars = len(ctx.report)
        rep_span.set_attribute("report_chars", ctx.result.report_chars)
        rep_span.set_attribute("has_coding", bool(ctx.coding_block))
        rep_span.set_attribute("elapsed_s", round(time.monotonic() - _t0, 2))
        log.info("worklog: hour=%s report_chars=%d coding_folded=%s",
                 ctx.hour, ctx.result.report_chars, bool(ctx.coding_block),
                 extra={"hour": ctx.hour, "report_chars": ctx.result.report_chars,
                        "coding_folded": bool(ctx.coding_block)})


def stage_candidates(ctx: HourContext) -> None:
    """Read daily plan + backlog candidates and attach reranker hints."""
    conn = wdb.open_db(ctx.db_path)
    try:
        plan_keys = wdb.fetch_confirmed_plan(conn, ctx.day_local)
        open_tasks = {t["task_key"]: t for t in wdb.fetch_open_tasks(conn)}
    finally:
        conn.close()

    def cand(key: str) -> Candidate:
        t = open_tasks.get(key, {"task_key": key, "title": key})
        return Candidate(task_key=key, title=t.get("title", key), doc=wdb.render_doc(t))

    ctx.daily = [cand(k) for k in plan_keys if k in open_tasks]
    backlog_keys = [k for k in open_tasks if k not in set(plan_keys)]

    def rerank(cands: list[Candidate]) -> None:
        if not cands:
            return
        ranked = _post(ctx.server_url, "/rerank", {
            "query": ctx.report[:1800],
            "candidates": [{"task_key": c.task_key, "doc": c.doc} for c in cands],
            "traceparent": observability.current_traceparent(),
        }, timeout=_FAST_TIMEOUT)["ranked"]
        score = {r["task_key"]: r["score"] for r in ranked}
        for c in cands:
            c.rerank_score = score.get(c.task_key, 0.0)

    with tracer.start_as_current_span("worklog.candidates", context=_parent(ctx)) as span:
        _t0 = time.monotonic()
        rerank(ctx.daily)
        if backlog_keys:
            bcands = [cand(k) for k in backlog_keys]
            rerank(bcands)
            bcands.sort(key=lambda c: -c.rerank_score)
            ctx.backlog = bcands[:ctx.max_backlog]
        span.set_attribute("n_daily", len(ctx.daily))
        span.set_attribute("n_backlog", len(ctx.backlog))
        span.set_attribute("elapsed_s", round(time.monotonic() - _t0, 2))
    ctx._open_tasks = open_tasks  # type: ignore[attr-defined]


def stage_classify(ctx: HourContext) -> None:
    """Tiered task classification — tier-1 daily plan then tier-2 backlog.

    Calls classify_hour which handles the tier logic internally: tier-1 first,
    then tier-2 batches until total matches >= 2 or backlog exhausted. The
    ``worklog.classify`` span is the parent the tier-1/tier-2/batch + server-side
    ``classify_tasks`` spans nest under. Sets ctx._outcome and ctx.result.tier_used.
    """
    with tracer.start_as_current_span("worklog.classify", context=_parent(ctx)) as span:
        _t0 = time.monotonic()
        span.set_attribute("daily_candidates", len(ctx.daily))
        span.set_attribute("backlog_candidates", len(ctx.backlog))
        outcome = classify_hour(ctx.server_url, ctx.report, ctx.daily, ctx.backlog)
        ctx._outcome = outcome
        ctx.result.tier_used = outcome.tier_used
        span.set_attribute("tier_used", outcome.tier_used)
        span.set_attribute("n_matched", len(outcome.bindings))
        span.set_attribute("matched_keys", ",".join(b.task_key for b in outcome.bindings))
        span.set_attribute("elapsed_s", round(time.monotonic() - _t0, 2))


def stage_propose(ctx: HourContext) -> None:
    """Decide whether the hour needs a NEW ticket, and draft it if so.

    Runs only when fewer than two existing tickets matched (an hour with two
    matches is already well-accounted-for). The proposer is told which tickets
    matched so it won't duplicate covered work, and MAY abstain — leaving
    ``ctx._proposed = None`` when the residual work isn't worth a PM ticket.
    """
    with tracer.start_as_current_span("worklog.propose", context=_parent(ctx)) as span:
        _t0 = time.monotonic()
        if len(ctx._outcome.bindings) >= 2:
            span.set_attribute("skipped", True)
            span.set_attribute("skip_reason", "two tickets already matched")
            span.set_attribute("elapsed_s", round(time.monotonic() - _t0, 2))
            return
        span.set_attribute("skipped", False)
        titles = {k: (t.get("title") or "") for k, t in ctx._open_tasks.items()}
        ctx._proposed = generation.propose_ticket(
            ctx.server_url, report=ctx.report, body=ctx.body,
            matched=ctx._outcome.bindings, titles=titles,
            traceparent=observability.current_traceparent(),
        )
        span.set_attribute("should_propose", ctx._proposed is not None)
        if ctx._proposed:
            span.set_attribute("issue_type", ctx._proposed.issue_type)
            span.set_attribute("title", ctx._proposed.title)
            span.set_attribute("reasoning", observability.preview(ctx._proposed.reasoning, 500))
            log.info("worklog: hour=%s proposing new %s ticket %r",
                     ctx.hour, ctx._proposed.issue_type, ctx._proposed.title)
        span.set_attribute("elapsed_s", round(time.monotonic() - _t0, 2))


def stage_generate(ctx: HourContext) -> None:
    """Draft + persist one worklog per ticket — matched bindings AND a proposed
    new ticket — then reconcile stale artifacts from a prior run of this hour.

    An hour can legitimately resolve to NOTHING: no match and an abstained
    proposal. That records a note and persists nothing (after retracting any
    stale prior-run rows), which is the correct "not worth logging" outcome.
    """
    res = ctx.result
    bindings = ctx._outcome.bindings
    proposed = ctx._proposed
    n_items = len(bindings) + (1 if proposed else 0)

    # Reset accumulators so an agno retry of this whole-stage step rebuilds the
    # result cleanly instead of appending duplicates (the per-item Loop cursor
    # that used to make this incremental is gone).
    res.matched = []
    res.worklog_ids = []
    res.proposed = None
    res.proposed_id = None

    _utc_s, _utc_e = local_hour_utc_bounds(ctx.hour)
    win_start, win_end = f"{_utc_s}+00:00", f"{_utc_e}+00:00"
    run_id = ctx.run_id or f"wl-{ctx.hour}"
    matched_keys = [b.task_key for b in bindings]

    with tracer.start_as_current_span("worklog.generate", context=_parent(ctx)) as gspan:
        _t0 = time.monotonic()
        gspan.set_attribute("n_items", n_items)
        gspan.set_attribute("n_matched", len(bindings))
        gspan.set_attribute("n_proposed", 1 if proposed else 0)
        gspan.set_attribute("matched_keys", ",".join(matched_keys))
        gspan.set_attribute("proposed_title", proposed.title if proposed else "")

        # Idempotent reconciliation (also covers the empty outcome): retract this
        # hour's stale DRAFTED worklogs not in the matched set, and drop a stale
        # proposal when this run produced none. A run that re-resolves identically
        # just re-skips already-skipped rows.
        if not ctx.dry_run:
            conn = wdb.open_db(ctx.db_path)
            try:
                wdb.retract_drafted_worklogs(
                    conn, day_utc=ctx.day_local, cycle_index=ctx.cycle_index,
                    keep_task_keys=matched_keys)
                if proposed is None:
                    wdb.retract_proposed_task(conn, day_utc=ctx.day_local, source_hour=ctx.hour)
            finally:
                conn.close()

        if n_items == 0:
            res.note = "nothing worth logging"
            gspan.set_attribute("note", res.note)
            gspan.set_attribute("elapsed_s", round(time.monotonic() - _t0, 2))
            log.info("worklog: hour=%s produced no match and no proposal — nothing logged",
                     ctx.hour, extra={"hour": ctx.hour})
            return

        # Divide the hour evenly across all drafted items so N don't log N×3600s.
        time_per_item = 3600 // max(1, n_items)

        # ── Matched tickets → pm_worklogs drafts ────────────────────────────
        for b in bindings:
            t = ctx._open_tasks.get(b.task_key, {"title": b.task_key, "description_text": ""})
            with tracer.start_as_current_span("worklog.generate.ticket") as tspan:
                tspan.set_attribute("task_key", b.task_key)
                tspan.set_attribute("is_new", False)
                tspan.set_attribute("confidence", b.confidence)
                draft = generation.generate_worklog(
                    ctx.server_url, report=ctx.report, body=ctx.body, task_key=b.task_key,
                    title=t.get("title", b.task_key), description=t.get("description_text") or "",
                    why=b.why, is_new=False, traceparent=observability.current_traceparent())
                tspan.set_attribute("drafted", draft is not None)
            res.matched.append({"task_key": b.task_key, "confidence": b.confidence,
                                "why": b.why, "tier": b.tier,
                                "summary": draft.summary if draft else None})
            if draft and not ctx.dry_run:
                conn = wdb.open_db(ctx.db_path)
                try:
                    payload = wdb.build_payload(
                        b.task_key, win_start, win_end, ctx.cycle_index, time_per_item,
                        draft, reasoning=b.why)
                    wid = wdb.upsert_worklog(
                        conn, task_key=b.task_key, day_utc=ctx.day_local,
                        cycle_index=ctx.cycle_index, window_start=win_start, window_end=win_end,
                        confidence=max(0.0, min(1.0, float(draft.confidence))),
                        time_spent_seconds=time_per_item, payload=payload, workflow_run_id=run_id)
                    if wid:
                        res.worklog_ids.append(wid)
                finally:
                    conn.close()

        # ── Proposed new ticket → pm_proposed_tasks (+ its drafted worklog) ──
        if proposed:
            why = proposed.reasoning or proposed.description
            with tracer.start_as_current_span("worklog.generate.ticket") as tspan:
                tspan.set_attribute("task_key", "(proposed)")
                tspan.set_attribute("is_new", True)
                tspan.set_attribute("issue_type", proposed.issue_type)
                draft = generation.generate_worklog(
                    ctx.server_url, report=ctx.report, body=ctx.body, task_key="(proposed)",
                    title=proposed.title, description=proposed.description, why=why, is_new=True,
                    traceparent=observability.current_traceparent())
                tspan.set_attribute("drafted", draft is not None)
            res.proposed = {"title": proposed.title, "description": proposed.description,
                            "issue_type": proposed.issue_type}
            wl_payload = (wdb.build_payload("", win_start, win_end, ctx.cycle_index,
                                            time_per_item, draft, reasoning=why)
                          if draft else None)
            wl_conf = max(0.0, min(1.0, float(draft.confidence))) if draft else 0.0
            if not ctx.dry_run:
                conn = wdb.open_db(ctx.db_path)
                try:
                    res.proposed_id = wdb.upsert_proposed_task(
                        conn, day_utc=ctx.day_local, source_hour=ctx.hour,
                        title=proposed.title, description=proposed.description,
                        reasoning=why, issue_type=proposed.issue_type,
                        workflow_run_id=run_id, worklog_payload=wl_payload,
                        time_spent_seconds=time_per_item, confidence=wl_conf,
                        window_start=win_start, window_end=win_end)
                finally:
                    conn.close()

        gspan.set_attribute("elapsed_s", round(time.monotonic() - _t0, 2))
