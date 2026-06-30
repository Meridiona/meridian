"""Tiered task classifier — abstention-first, FSM JSON over HTTP.

Tier 1: match the hour's activity report against the confirmed daily plan
        (2-5 tasks), with reranker scores as a hint.
Tier 2: only if Tier 1 found < 2 matches — scan the rest of the backlog in
        reranker-ranked order, 5 tasks per batch, stop at the first batch that
        yields a match (off-plan work is rare; this bounds LLM calls).
Tier 3: still nothing → caller proposes a new ticket.

Calls POST /classify_tasks on the MLX server directly. The endpoint decodes with
outlines FSM (grammar-constrained JSON) server-side; callers receive a
ClassificationResult-shaped JSON object.

Failure posture (matches ``generation.py``): a TRANSPORT/HTTP failure is RAISED so
the workflow Step retries and, if it still fails, the hour fails and the Rust driver
re-runs it next pass — a transient model blip must NEVER be mistaken for "no match"
and silently drop the hour's real tickets. A well-formed but empty result returns [].
"""
from __future__ import annotations

import json
import logging
import urllib.request
from dataclasses import dataclass, field

from opentelemetry import trace

from agents import observability
from agents.worklog_pipeline.models import ClassificationResult, classification_keys

log = logging.getLogger("meridian.worklog.classifier")
tracer = trace.get_tracer("meridian.worklog.classifier")

_BATCH = 5             # tier-2 backlog tasks per LLM call
_MIN_CONFIDENCE = 0.5  # drop matches the model itself isn't confident in
_DEFAULT_SERVER = "http://127.0.0.1:7823"


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
class ClassificationOutcome:
    bindings:    list[TaskBinding] = field(default_factory=list)
    tier_used:   int = 0          # 1, 2, or 0 when nothing matched


def _post_classify(
    server_url: str,
    report: str,
    candidates: list[Candidate],
    tier: int,
    tier_note: str,
) -> list[TaskBinding]:
    """POST /classify_tasks and return validated TaskBindings.

    Sends the active span's ``traceparent`` so the server-side ``classify_tasks``
    span nests under the caller's tier/batch span (one connected worklog trace).
    """
    keys = [c.task_key for c in candidates]
    body = {
        "report":     report,
        "candidates": [
            {"task_key": c.task_key, "title": c.title, "doc": c.doc, "rerank_score": c.rerank_score}
            for c in candidates
        ],
        "tier":      tier,
        "tier_note": tier_note,
        "traceparent": observability.current_traceparent(),
    }
    req = urllib.request.Request(
        f"{server_url}/classify_tasks",
        data=json.dumps(body).encode(),
        headers={"Content-Type": "application/json"},
    )
    try:
        with urllib.request.urlopen(req, timeout=120) as r:
            resp = json.loads(r.read())
    except Exception as exc:  # noqa: BLE001
        # Transport/HTTP failure (timeout, conn reset, the route's 500 on an inference
        # error). RAISE — do not return [] — so the retry tier fires. Swallowing this
        # would make a transient blip indistinguishable from a genuine "no match" and
        # silently drop the hour's real tickets (then wrongly propose a new one).
        log.warning("classify: tier-%d /classify_tasks request failed: %s", tier, exc)
        raise

    log.debug("classify: tier-%d raw response reasoning=%r matches=%r",
              tier, resp.get("reasoning", "")[:100], resp.get("matches"))

    try:
        result = ClassificationResult(
            reasoning=resp.get("reasoning", ""),
            matches=resp.get("matches", []),
        )
    except (TypeError, ValueError) as exc:
        log.warning("classify: tier-%d response parse failed (%s)", tier, exc)
        return []

    best: dict[str, TaskBinding] = {}
    for key, conf, why in classification_keys(result):
        if key not in keys:
            log.debug("classify: dropping hallucinated key %s", key)
            continue
        if conf < _MIN_CONFIDENCE:
            log.debug("classify: dropping low-confidence %s @ %.2f", key, conf)
            continue
        prev = best.get(key)
        if prev is None or conf > prev.confidence:
            best[key] = TaskBinding(task_key=key, confidence=conf, why=why, tier=tier)
    return list(best.values())


def classify_tier1(
    server_url: str,
    report: str,
    daily: list[Candidate],
) -> list[TaskBinding]:
    """One classification call over the confirmed daily plan. Empty list = no match."""
    if not daily:
        return []
    t1 = _post_classify(server_url, report, daily, 1, "today's confirmed plan")
    if t1:
        log.info("classify: tier-1 matched %s", [b.task_key for b in t1])
    return t1


def classify_tier2_batch(
    server_url: str,
    report: str,
    batch: list[Candidate],
    batch_idx: int,
) -> list[TaskBinding]:
    """One classification call over a single backlog batch (≤_BATCH tasks). Empty = no match."""
    if not batch:
        return []
    t2 = _post_classify(server_url, report, batch, 2, "wider backlog batch")
    if t2:
        log.info("classify: tier-2 matched %s (batch %d)", [b.task_key for b in t2], batch_idx)
    return t2


# Tier-2 batch size, re-exported for the tier-1 debug harness (scripts/run-classify-tier1.sh)
# which paginates the backlog the same way classify_hour does.
BATCH = _BATCH


def classify_hour(
    server_url: str,
    report: str,
    daily: list[Candidate],
    backlog: list[Candidate],
) -> ClassificationOutcome:
    """Tiered classification: tier-1 over the daily plan, then tier-2 backlog
    batches until two tickets total are matched or the backlog is exhausted.

    This is the single entry point the pipeline's ``stage_classify`` calls (the
    production agno Workflow wraps it as one ``classify`` Step); ``classify_tier1``
    / ``classify_tier2_batch`` are its building blocks (also driven directly by the
    tier-1 debug harness, scripts/run-classify-tier1.sh).
    """
    with tracer.start_as_current_span("worklog.classify.tier1") as sp:
        t1 = classify_tier1(server_url, report, daily)
        sp.set_attribute("candidates", len(daily))
        sp.set_attribute("matched", len(t1))
        sp.set_attribute("matched_keys", ",".join(b.task_key for b in t1))
    if len(t1) >= 2:
        return ClassificationOutcome(bindings=t1, tier_used=1)

    all_t2: list[TaskBinding] = []
    if backlog:
        with tracer.start_as_current_span("worklog.classify.tier2") as t2_sp:
            for i in range(0, len(backlog), _BATCH):
                bi = i // _BATCH
                batch = backlog[i : i + _BATCH]
                with tracer.start_as_current_span(f"worklog.classify.tier2.batch{bi}") as bsp:
                    batch_result = classify_tier2_batch(server_url, report, batch, bi)
                    bsp.set_attribute("batch_index", bi)
                    bsp.set_attribute("candidates", len(batch))
                    bsp.set_attribute("matched", len(batch_result))
                    bsp.set_attribute("matched_keys", ",".join(b.task_key for b in batch_result))
                all_t2.extend(batch_result)
                if len(t1) + len(all_t2) >= 2:
                    break
            t2_sp.set_attribute("batches_run", (i // _BATCH) + 1)
            t2_sp.set_attribute("matched", len(all_t2))

    if t1 or all_t2:
        tier = 2 if all_t2 else 1
        return ClassificationOutcome(bindings=t1 + all_t2, tier_used=tier)
    log.info("classify: no task matched — proposing new ticket")
    return ClassificationOutcome()
