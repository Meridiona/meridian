"""Session Task Classifier — resolves session→task matches via hermes AIAgent.

Receives a session and the full list of open pm_tasks. Keyword-ranks candidates
down to MAX_CANDIDATES before sending to the LLM so the prompt stays focused.

Uses AIAgent in its simplest mode:
* `enabled_toolsets=[]`  — no tools to call
* `max_iterations=1`     — one model round, no agent loop
* system prompt is the SKILL.md for 'task-classifier'
"""
from __future__ import annotations

import logging
import os
import time
from dataclasses import dataclass, field

from agents import observability
from agents.config import MODEL, BASE_URL, API_KEY, load_skill, LLM_PREFER_LOCAL, LLM_BUDGET_PCT

from ._prompts import build_user_message
from ._parser import parse_response, routing_for

log = logging.getLogger("agents.task_classifier_agent")
tracer = observability.setup("meridian-task-classifier")


# ── Config / thresholds ────────────────────────────────────────────────────────
AUTO_FLOOR     = float(os.environ.get("AGENT_AUTO_FLOOR",  "0.65"))
QUEUE_FLOOR    = float(os.environ.get("AGENT_QUEUE_FLOOR", "0.40"))
MAX_TOKENS = int(os.environ.get("AGENT_MAX_TOKENS", "4000"))
SKILL_NAME = os.environ.get("AGENT_SKILL_NAME", "task-classifier")


# ── Result type ────────────────────────────────────────────────────────────────
@dataclass
class ClassifierDecision:
    session_id: int
    chosen_task_key: str | None
    confidence: float
    reasoning: str
    routing: str                      # 'auto' | 'queue' | 'skip'
    method: str                       # 'task_classifier' | 'agent_unavailable' | 'agent_invalid_response'
    raw_response: str = ""
    elapsed_s: float = 0.0
    debug: dict = field(default_factory=dict)
    dimensions: dict[str, list[str]] = field(default_factory=dict)


# ── Public entry ───────────────────────────────────────────────────────────────
def classify_session(
    session: dict,
    pm_tasks: list[dict],
    pm_task_lookup: dict[str, dict] | None = None,
) -> ClassifierDecision:
    """Match a session to the best open Jira ticket via the hermes AIAgent.

    pm_tasks is the full list of open tasks; they are keyword-ranked down to
    MAX_CANDIDATES before being sent to the LLM.
    pm_task_lookup is unused but accepted for call-site compatibility.
    """
    sid = int(session["id"])
    with tracer.start_as_current_span("task_classifier_agent.decide") as span:
        span.set_attribute("session_id", sid)
        span.set_attribute("model", MODEL or "")
        span.set_attribute("pm_tasks", len(pm_tasks))
        result = _classify_session_inner(session, pm_tasks, sid)
        span.set_attribute("method", result.method)
        span.set_attribute("routing", result.routing)
        span.set_attribute("confidence", float(result.confidence))
        if result.chosen_task_key:
            span.set_attribute("chosen_task_key", result.chosen_task_key)
        span.set_attribute("agent_latency_ms", int(result.elapsed_s * 1000))
        return result


def _classify_session_inner(
    session: dict,
    pm_tasks: list[dict],
    sid: int,
) -> ClassifierDecision:
    if not pm_tasks:
        return ClassifierDecision(
            session_id=sid, chosen_task_key=None, confidence=0.0,
            reasoning="no pm_tasks available", routing="skip",
            method="agent_unavailable",
        )

    valid_keys = {t["task_key"] for t in pm_tasks}

    user_message = build_user_message(session, pm_tasks)
    log.debug("task_classifier_agent user message:\n%s", user_message)

    try:
        base_prompt = load_skill(SKILL_NAME)
    except FileNotFoundError as exc:
        return ClassifierDecision(
            session_id=sid, chosen_task_key=None, confidence=0.0,
            reasoning=str(exc), routing="skip", method="agent_unavailable",
        )

    # ── Dynamic LLM endpoint selection ───────────────────────────────────────
    # Default to static config; override with a local model if available.
    _model, _base_url, _api_key = MODEL, BASE_URL, (API_KEY or "none")
    if LLM_PREFER_LOCAL:
        from agents.llm_selector import select_model_for_hermes
        local_ep = select_model_for_hermes(budget_pct=LLM_BUDGET_PCT)
        if local_ep:
            _model, _base_url, _api_key = local_ep.model, local_ep.base_url, local_ep.api_key
            log.info("task_classifier_agent: local model=%s runtime=%s",
                     _model, local_ep.runtime)
        else:
            log.info("task_classifier_agent: no local model available, using cloud model=%s",
                     _model)

    from agents._hermes_setup import ensure_hermes_importable
    ensure_hermes_importable()
    try:
        from run_agent import AIAgent
    except ImportError as exc:
        return ClassifierDecision(
            session_id=sid, chosen_task_key=None, confidence=0.0,
            reasoning=f"hermes AIAgent import failed: {exc}",
            routing="skip", method="agent_unavailable",
        )

    log.info("task_classifier_agent: model=%s base_url=%s skill=%s pm_tasks=%d",
             _model, _base_url, SKILL_NAME, len(pm_tasks))

    t0 = time.time()
    try:
        agent = AIAgent(
            model=_model,
            base_url=_base_url,
            api_key=_api_key,
            ephemeral_system_prompt=base_prompt,
            enabled_toolsets=[],
            quiet_mode=True,
            skip_context_files=True,
            load_soul_identity=False,
            skip_memory=True,
            max_iterations=1,
            max_tokens=MAX_TOKENS,
        )
        result = agent.run_conversation(user_message)
    except Exception as exc:  # noqa: BLE001
        elapsed = time.time() - t0
        log.warning("task_classifier_agent AIAgent failed: %s", exc)
        return ClassifierDecision(
            session_id=sid, chosen_task_key=None, confidence=0.0,
            reasoning=f"AIAgent run failed: {exc}", routing="skip",
            method="agent_unavailable", elapsed_s=elapsed,
        )

    elapsed = time.time() - t0

    raw = ""
    if isinstance(result, dict):
        raw = str(
            result.get("final_response")
            or result.get("response")
            or ""
        ).strip()

    log.debug("task_classifier_agent raw response (%.1fs): %s", elapsed, raw[:1000])

    task_key, confidence, reasoning, dimensions, err = parse_response(raw, valid_keys)
    if err:
        log.warning("task_classifier_agent invalid response: %s", err)
        return ClassifierDecision(
            session_id=sid, chosen_task_key=None, confidence=0.0,
            reasoning=err, routing="skip", method="agent_invalid_response",
            raw_response=raw[:1000], elapsed_s=elapsed,
            debug={"error": err, "model": _model, "base_url": _base_url},
        )

    routing = routing_for(confidence, task_key, AUTO_FLOOR, QUEUE_FLOOR)
    return ClassifierDecision(
        session_id=sid,
        chosen_task_key=task_key,
        confidence=confidence,
        reasoning=reasoning,
        routing=routing,
        method="task_classifier",
        raw_response=raw[:1000],
        elapsed_s=elapsed,
        dimensions=dimensions,
        debug={
            "model":       _model,
            "base_url":    _base_url,
            "n_tasks":     len(pm_tasks),
            "auto_floor":  AUTO_FLOOR,
            "queue_floor": QUEUE_FLOOR,
            "skill":       SKILL_NAME,
        },
    )


__all__ = [
    "ClassifierDecision", "classify_session", "AUTO_FLOOR", "QUEUE_FLOOR",
]
