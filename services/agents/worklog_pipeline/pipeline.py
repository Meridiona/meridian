"""End-to-end worklog pipeline for one hour, as composable stages.

    distil_hour → activity report → reranker hint → tiered match →
    worklog draft per matched task  (or  propose new task)  → persist

The work is split into stages that share a single ``HourContext``. ``run_hour``
calls them in sequence (the plain path); ``workflow.py`` wraps the same stage
functions as agno ``Step``s for run persistence + the agno-native structure.
Every model call goes through the MLX server's HTTP endpoints, so only one model
is ever resident.
"""
from __future__ import annotations

import json
import logging
import urllib.request
from dataclasses import dataclass, field

from opentelemetry import trace

from agents import observability
from agents.worklog_pipeline import db as wdb
from agents.worklog_pipeline.agent_io import make_match_agent_factory, make_schema_agent
from agents.worklog_pipeline.match import (
    BATCH, Candidate, MatchOutcome, run_tier1, run_tier2_batch,
)
from agents.worklog_pipeline.models import ProposedTicket, WorklogDraft
from agents.worklog_pipeline.prompts.match_tasks import SYSTEM as MATCH_SYSTEM
from agents.worklog_pipeline.prompts.propose_ticket import SYSTEM as PROPOSE_SYSTEM
from agents.worklog_pipeline.worklog import WORKLOG_SYSTEM, generate_worklog

log = logging.getLogger("meridian.worklog.pipeline")
tracer = trace.get_tracer("meridian.worklog.pipeline")

_DEFAULT_SERVER = "http://127.0.0.1:7823"


def _post(server: str, path: str, body: dict, timeout: float = 300) -> dict:
    req = urllib.request.Request(
        f"{server}{path}", data=json.dumps(body).encode(),
        headers={"Content-Type": "application/json"},
    )
    with urllib.request.urlopen(req, timeout=timeout) as r:
        return json.loads(r.read())


@dataclass
class HourResult:
    hour:         str
    day_utc:      str
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
            "hour": self.hour, "day_utc": self.day_utc, "nsess": self.nsess,
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
        self.day_utc = self.hour[:10]
        if not self.cycle_index:
            self.cycle_index = int(self.hour[11:13])
        if self.result is None:
            self.result = HourResult(hour=self.hour, day_utc=self.day_utc)
        # Internal cross-stage state. Initialised here (not attached dynamically)
        # so a stage that runs without its predecessor never raises AttributeError
        # — it just sees the empty default and produces nothing.
        self._open_tasks: dict = {}
        self._outcome: MatchOutcome = MatchOutcome()
        self._batch_i: int = 0   # tier-2 backlog cursor (advanced per Loop iteration)
        self._draft_i: int = 0   # draft cursor (advanced per draft Loop iteration)


# ── Stages ──────────────────────────────────────────────────────────────────────

def _format_coding_block(coding: list[dict]) -> str:
    """Render coding-agent summaries as a verbatim activity-summary section.

    Each session becomes one labeled block carrying its own agent-written summary
    unchanged (no re-compression, no rewrite) — so file names, ticket keys, and
    per-task detail survive into the matcher and worklog draft.
    """
    if not coding:
        return ""
    parts = ["## Coding sessions (agent summaries — verbatim)"]
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
    d = _post(ctx.server_url, "/distill_hour",
              {"hour": ctx.hour, "db_path": ctx.db_path, "traceparent": ctx.traceparent})
    ctx.body = d["body"]
    ocr_nsess = d["nsess"]

    conn = wdb.open_db(ctx.db_path)
    try:
        coding = wdb.fetch_coding_summaries(conn, ctx.hour)
    finally:
        conn.close()
    ctx.coding_block = _format_coding_block(coding)
    ctx.result.nsess = ocr_nsess + len(coding)

    span = trace.get_current_span()
    span.set_attribute("ocr_nsess", ocr_nsess)
    span.set_attribute("coding_nsess", len(coding))
    if coding:
        span.set_attribute("coding_preview", observability.preview(ctx.coding_block))

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
    """Build the activity summary the matcher/worklog see.

    The activity report (think-mode prose) covers ONLY the distilled OCR/app body.
    Coding-agent work is appended VERBATIM (the bypass-the-rewrite decision): the
    agent summaries are already clean prose and must not be genericised by the
    team-facing report prompt — file names and ticket keys would be stripped.
    """
    if ctx.body.strip():
        rep = _post(ctx.server_url, "/activity_report",
                    {"body": ctx.body, "label": ctx.hour, "max_tokens": 8192,
                     "traceparent": ctx.traceparent})
        ctx.report = rep["report"]
    else:
        ctx.report = ""
    if ctx.coding_block:
        ctx.report = f"{ctx.report}\n\n{ctx.coding_block}".strip()
    ctx.result.report_chars = len(ctx.report)
    log.info("worklog: hour=%s report_chars=%d coding_folded=%s",
             ctx.hour, ctx.result.report_chars, bool(ctx.coding_block),
             extra={"hour": ctx.hour, "report_chars": ctx.result.report_chars,
                    "coding_folded": bool(ctx.coding_block)})


def stage_candidates(ctx: HourContext) -> None:
    """Read daily plan + backlog candidates and attach reranker hints."""
    with tracer.start_as_current_span("worklog.candidates") as span:
        conn = wdb.open_db(ctx.db_path)
        try:
            plan_keys = wdb.fetch_confirmed_plan(conn, ctx.day_utc)
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
            })["ranked"]
            score = {r["task_key"]: r["score"] for r in ranked}
            for c in cands:
                c.rerank_score = score.get(c.task_key, 0.0)

        rerank(ctx.daily)
        if backlog_keys:
            bcands = [cand(k) for k in backlog_keys]
            rerank(bcands)
            bcands.sort(key=lambda c: -c.rerank_score)
            ctx.backlog = bcands[:ctx.max_backlog]
        # stash open_tasks for the persist stage
        ctx._open_tasks = open_tasks  # type: ignore[attr-defined]

        ranked_all = sorted(ctx.daily + ctx.backlog, key=lambda c: -c.rerank_score)
        top = ranked_all[0] if ranked_all else None
        span.set_attribute("n_plan", len(plan_keys))
        span.set_attribute("n_open_tasks", len(open_tasks))
        span.set_attribute("n_daily", len(ctx.daily))
        span.set_attribute("n_backlog", len(ctx.backlog))
        span.set_attribute("rerank_top_key", top.task_key if top else "")
        span.set_attribute("rerank_top_score", round(top.rerank_score, 4) if top else 0.0)
        span.set_attribute(
            "rerank_scores_preview",
            observability.preview(
                ", ".join(f"{c.task_key}:{c.rerank_score:.3f}" for c in ranked_all)
            ),
        )


def _match_factory(ctx: HourContext):
    return make_match_agent_factory(MATCH_SYSTEM, server_url=ctx.server_url)


def stage_match_tier1(ctx: HourContext) -> None:
    """Tier 1 — match the confirmed daily plan. Runs only when a daily plan
    exists (the agno Condition guards this). Sets ctx._outcome on a hit.

    The agno Agent.run() inside nests as an OpenInference span (prompt/output/
    tokens); we annotate the current (agno step) span with the decision.
    """
    span = trace.get_current_span()
    bindings = run_tier1(_match_factory(ctx), ctx.report, ctx.daily)
    if bindings:
        ctx._outcome = MatchOutcome(bindings=bindings, tier_used=1)
        ctx.result.tier_used = 1
    span.set_attribute("tier", 1)
    span.set_attribute("n_daily", len(ctx.daily))
    span.set_attribute("matched_keys", ",".join(b.task_key for b in bindings))


def stage_match_tier2_batch(ctx: HourContext) -> None:
    """Tier 2 — one backlog batch (the agno Loop repeats this, advancing the
    cursor, until a hit or the backlog is exhausted)."""
    span = trace.get_current_span()
    i = ctx._batch_i
    batch = ctx.backlog[i : i + BATCH]
    # run FIRST, advance the cursor only on success — so an agno retry (which
    # re-runs this step on a raised transient error) re-processes THIS batch
    # rather than skipping ahead to the next one.
    bindings = run_tier2_batch(_match_factory(ctx), ctx.report, batch, i // BATCH)
    ctx._batch_i = i + BATCH
    if bindings:
        ctx._outcome = MatchOutcome(bindings=bindings, tier_used=2)
        ctx.result.tier_used = 2
    span.set_attribute("tier", 2)
    span.set_attribute("batch_index", i // BATCH)
    span.set_attribute("batch_keys", ",".join(c.task_key for c in batch))
    span.set_attribute("matched_keys", ",".join(b.task_key for b in bindings))


def stage_draft_one(ctx: HourContext) -> None:
    """Draft + persist ONE worklog for the next matched binding (the agno draft
    Loop repeats this, advancing _draft_i, once per matched task)."""
    res = ctx.result
    bindings = ctx._outcome.bindings
    i = ctx._draft_i
    if i >= len(bindings):
        return
    # Mutual exclusion (idempotent re-run): on the FIRST draft, retract any stale
    # artifacts from a prior run of this hour — the now-superseded proposed ticket,
    # and drafted worklogs for tasks NOT matched this run. The matched set is fully
    # known before drafting (ctx._outcome.bindings). Re-running the loop (agno
    # retry) just re-skips already-skipped rows — harmless.
    if i == 0 and not ctx.dry_run:
        matched_keys = [bb.task_key for bb in bindings]
        conn = wdb.open_db(ctx.db_path)
        try:
            wdb.retract_proposed_task(conn, day_utc=ctx.day_utc, source_hour=ctx.hour)
            wdb.retract_drafted_worklogs(
                conn, day_utc=ctx.day_utc, cycle_index=ctx.cycle_index,
                keep_task_keys=matched_keys)
        finally:
            conn.close()
    b = bindings[i]
    with tracer.start_as_current_span("worklog.draft") as dspan:
        dspan.set_attribute("task_key", b.task_key)
        dspan.set_attribute("confidence", round(b.confidence, 3))
        dspan.set_attribute("tier", b.tier)
        t = ctx._open_tasks.get(b.task_key, {"title": b.task_key, "description_text": ""})
        wl_agent = make_schema_agent(WORKLOG_SYSTEM, WorklogDraft,
                                     server_url=ctx.server_url, max_tokens=2400)
        draft = generate_worklog(
            wl_agent, ctx.report, ctx.body, b.task_key,
            t.get("title", b.task_key), t.get("description_text") or "", b.why)
        res.matched.append({"task_key": b.task_key, "confidence": b.confidence,
                            "why": b.why, "tier": b.tier,
                            "summary": draft.summary if draft else None})
        dspan.set_attribute("parsed", draft is not None)
        if draft:
            dspan.set_attribute("summary_preview", observability.preview(draft.summary))
        if draft and not ctx.dry_run:
            conn = wdb.open_db(ctx.db_path)
            try:
                payload = wdb.build_payload(
                    b.task_key, f"{ctx.hour}:00:00+00:00", f"{ctx.hour}:59:59+00:00",
                    ctx.cycle_index, 3600, draft)
                wid = wdb.upsert_worklog(
                    conn, task_key=b.task_key, day_utc=ctx.day_utc,
                    cycle_index=ctx.cycle_index,
                    window_start=f"{ctx.hour}:00:00+00:00",
                    window_end=f"{ctx.hour}:59:59+00:00",
                    confidence=max(0.0, min(1.0, float(draft.confidence))),
                    time_spent_seconds=3600,
                    payload=payload, workflow_run_id=ctx.run_id or f"wl-{ctx.hour}")
                if wid:
                    res.worklog_ids.append(wid)
                    dspan.set_attribute("worklog_id", wid)
            finally:
                conn.close()
            log.info("worklog: drafted %s (tier=%d conf=%.2f worklog_id=%s) for hour=%s",
                     b.task_key, b.tier, b.confidence, res.worklog_ids[-1] if res.worklog_ids else None,
                     ctx.hour, extra={"task_key": b.task_key, "tier": b.tier,
                                      "confidence": b.confidence, "hour": ctx.hour})
        elif draft is None:
            log.warning("worklog: draft did not parse for %s (hour=%s) — skipped",
                        b.task_key, ctx.hour, extra={"task_key": b.task_key, "hour": ctx.hour})
    # Advance only after this binding is fully handled. A raise above (transient
    # model error) leaves the cursor put, so the agno retry re-drafts THIS binding;
    # a returned-but-unparsed draft (summary=None) is "handled" and we move on.
    ctx._draft_i = i + 1


def stage_propose(ctx: HourContext) -> None:
    """Tier 3 — no task fit; draft a proposed new ticket (the agno Router selects
    this branch when there are no bindings)."""
    res = ctx.result
    with tracer.start_as_current_span("worklog.propose") as pr_span:
        pr_agent = make_schema_agent(PROPOSE_SYSTEM, ProposedTicket,
                                     server_url=ctx.server_url, max_tokens=900)
        user = (f"ACTIVITY SUMMARY (last hour):\n{ctx.report}\n\n"
                f"DISTILLED CAPTURE DETAIL:\n{ctx.body[:6000]}")
        pt = pr_agent.run(input=user).content
        parsed = isinstance(pt, ProposedTicket)
        pr_span.set_attribute("parsed", parsed)
        if parsed:
            res.proposed = {"title": pt.title, "description": pt.description}
            pr_span.set_attribute("proposed_title", pt.title)
            pr_span.set_attribute("proposed_desc_preview", observability.preview(pt.description))
            if not ctx.dry_run:
                conn = wdb.open_db(ctx.db_path)
                try:
                    res.proposed_id = wdb.upsert_proposed_task(
                        conn, day_utc=ctx.day_utc, source_hour=ctx.hour,
                        title=pt.title, description=pt.description, reasoning="",
                        workflow_run_id=ctx.run_id or f"wl-{ctx.hour}")
                    if res.proposed_id is not None:
                        pr_span.set_attribute("proposed_id", res.proposed_id)
                    # Mutual exclusion: this hour now resolves to a NEW-ticket
                    # proposal, so retract any drafted worklogs left by a prior
                    # run that had matched a task (idempotent re-run).
                    wdb.retract_drafted_worklogs(
                        conn, day_utc=ctx.day_utc, cycle_index=ctx.cycle_index,
                        keep_task_keys=[])
                finally:
                    conn.close()
            log.info("worklog: proposed new ticket '%s' (id=%s) for hour=%s",
                     pt.title, res.proposed_id, ctx.hour,
                     extra={"proposed_id": res.proposed_id, "hour": ctx.hour})
        else:
            log.warning("worklog: propose output did not parse for hour=%s", ctx.hour,
                        extra={"hour": ctx.hour})


# ── Plain orchestration ──────────────────────────────────────────────────────────

def run_hour(
    hour: str,
    *,
    db_path: str,
    server_url: str = _DEFAULT_SERVER,
    cycle_index: int | None = None,
    dry_run: bool = False,
    max_backlog: int = 15,
    traceparent: str | None = None,
) -> HourResult:
    """Run the full pipeline for one hour label 'YYYY-MM-DDTHH'.

    Delegates to the agno Workflow so there is ONE orchestration path: the
    Condition → Loop → Router graph in workflow.py. (CLI / eval callers land
    here; the daemon enters via /worklog_hour → run_hour_workflow directly.)
    """
    from agents.worklog_pipeline.workflow import run_hour_workflow

    return run_hour_workflow(
        hour, db_path=db_path, server_url=server_url, cycle_index=cycle_index,
        dry_run=dry_run, max_backlog=max_backlog, traceparent=traceparent,
    )
