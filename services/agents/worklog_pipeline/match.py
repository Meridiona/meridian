"""Tiered task matcher — abstention-first, schema-enforced (no regex).

Tier 1: match the hour's activity report against the confirmed daily plan
        (2-5 tasks), with reranker scores as a hint.
Tier 2: only if Tier 1 found nothing — scan the rest of the backlog in
        reranker-ranked order, 5 tasks per batch, stop at the first batch that
        yields a match (off-plan work is rare; this bounds LLM calls).
Tier 3: still nothing → caller proposes a new ticket.

Each tier runs an agno Agent whose ``output_schema`` is a MatchResult with
``task_key`` constrained to that tier/batch's candidate set, so the model can
only return real candidate keys. An empty match list is the expected "no task"
answer and is never coerced into a match.
"""
from __future__ import annotations

import logging
from dataclasses import dataclass, field

from agents.worklog_pipeline.models import build_match_result, match_keys
from agents.worklog_pipeline.prompts.match_tasks import SYSTEM as MATCH_SYSTEM

log = logging.getLogger("meridian.worklog.match")

_BATCH = 5             # tier-2 backlog tasks per LLM call
_MIN_CONFIDENCE = 0.5  # drop matches the model itself isn't confident in


@dataclass
class Candidate:
    task_key:    str
    title:       str
    doc:         str            # rendered ticket text (for reranker + prompt)
    rerank_score: float = 0.0


@dataclass
class TaskBinding:
    task_key:   str
    confidence: float
    why:        str
    tier:       int


@dataclass
class MatchOutcome:
    bindings:    list[TaskBinding] = field(default_factory=list)
    tier_used:   int = 0          # 1, 2, or 0 when nothing matched
    propose_new: bool = False     # True → no task matched, draft a new one


def _render_candidates(cands: list[Candidate]) -> str:
    lines = []
    for c in cands:
        hint = f"  [reranker hint: {c.rerank_score:.2f}]" if c.rerank_score else ""
        lines.append(f"- {c.task_key}: {c.title}{hint}")
        if c.doc and c.doc.strip() != c.title.strip():
            lines.append(f"    {c.doc[:240]}")
    return "\n".join(lines)


def _run_tier(
    agent_factory,
    report: str,
    cands: list[Candidate],
    tier: int,
    tier_note: str,
) -> list[TaskBinding]:
    """One schema-enforced match call over a fixed candidate set."""
    keys = [c.task_key for c in cands]
    schema = build_match_result(keys)
    agent = agent_factory(schema)
    user = (
        f"ACTIVITY SUMMARY (last hour):\n{report}\n\n"
        f"CANDIDATE TASKS ({tier_note}):\n{_render_candidates(cands)}"
    )
    response = agent.run(input=user)
    result = response.content
    if not hasattr(result, "matches"):
        # Schema parse failed (e.g. truncated generation). Treat as no match —
        # never coerce a malformed response into a binding.
        log.warning("match: tier-%d output did not parse to MatchResult (type=%s) — no match",
                    tier, type(result).__name__)
        return []
    # Dedupe by task_key — the model occasionally lists the same task twice with
    # different `why`. Keep the highest-confidence occurrence.
    best: dict[str, TaskBinding] = {}
    for key, conf, why in match_keys(result):
        if key not in keys:  # schema already guarantees this; belt-and-braces
            continue
        if conf < _MIN_CONFIDENCE:
            # The model sometimes lists non-matches with confidence ~0 and a
            # "did not advance" justification. Honour its own confidence: drop them.
            log.debug("match: dropping low-confidence %s @ %.2f", key, conf)
            continue
        prev = best.get(key)
        if prev is None or conf > prev.confidence:
            best[key] = TaskBinding(task_key=key, confidence=conf, why=why, tier=tier)
    return list(best.values())


def run_tier1(agent_factory, report: str, daily: list[Candidate]) -> list[TaskBinding]:
    """One match call over the confirmed daily plan. Empty list = no match."""
    if not daily:
        return []
    t1 = _run_tier(agent_factory, report, daily, 1, "today's confirmed plan")
    if t1:
        log.info("match: tier-1 matched %s", [b.task_key for b in t1])
    return t1


def run_tier2_batch(agent_factory, report: str, batch: list[Candidate], batch_idx: int) -> list[TaskBinding]:
    """One match call over a single backlog batch (≤_BATCH tasks). Empty = no match."""
    if not batch:
        return []
    t2 = _run_tier(agent_factory, report, batch, 2, "wider backlog batch")
    if t2:
        log.info("match: tier-2 matched %s (batch %d)", [b.task_key for b in t2], batch_idx)
    return t2


# Tier-2 batch size, re-exported so the workflow can size its Loop iterations.
BATCH = _BATCH


def match_hour(
    agent_factory,
    report: str,
    daily: list[Candidate],
    backlog: list[Candidate],
) -> MatchOutcome:
    """Tiered match in one call (plain-Python path, kept for tests/eval harness).

    The PRODUCTION orchestration lives in the agno Workflow (workflow.py) as
    Condition → Loop → Router over ``run_tier1`` / ``run_tier2_batch``; this
    function mirrors that flow for non-workflow callers.
    """
    t1 = run_tier1(agent_factory, report, daily)
    if t1:
        return MatchOutcome(bindings=t1, tier_used=1)
    for i in range(0, len(backlog), _BATCH):
        t2 = run_tier2_batch(agent_factory, report, backlog[i : i + _BATCH], i // _BATCH)
        if t2:
            return MatchOutcome(bindings=t2, tier_used=2)
    log.info("match: no task matched — proposing new ticket")
    return MatchOutcome(propose_new=True)
