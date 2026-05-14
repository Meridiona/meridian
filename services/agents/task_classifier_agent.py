"""Task Classifier Agent — resolves ambiguous session→task matches via hermes AIAgent.

Three execution modes (controlled by the `mode` parameter):
  MODE_TIEBREAK   — Stage 2 ran; break the tie among top-K ranked candidates.
  MODE_NO_DIMS    — Stage 2 ran; Stage 1 disabled — no rule-derived dims.
  MODE_STANDALONE — Stage 1+2 both disabled; keyword-prefilter from all tasks
                    and ask the agent to also infer dimension tags.

We deliberately use AIAgent in its simplest mode:
* `enabled_toolsets=[]`  — no tools to call
* `max_iterations=1`     — one model round, no agent loop
* system prompt is the base SKILL.md with a mode-specific addendum appended
  so prompt iteration doesn't require code changes.
"""
from __future__ import annotations

import logging
import os
import time
from dataclasses import dataclass, field

from agents import observability
from agents.config import MODEL, BASE_URL, API_KEY, load_skill

from ._prompts import build_system_prompt, build_user_message, _prefilter_tasks
from ._parser import parse_response, routing_for

log = logging.getLogger("agents.task_classifier_agent")
tracer = observability.setup("meridian-task-classifier")


# ── Config / thresholds ────────────────────────────────────────────────────────
AUTO_FLOOR  = float(os.environ.get("AGENT_AUTO_FLOOR",  "0.65"))
QUEUE_FLOOR = float(os.environ.get("AGENT_QUEUE_FLOOR", "0.40"))
MAX_TOKENS           = int(os.environ.get("AGENT_MAX_TOKENS", "4000"))
SKILL_NAME           = os.environ.get("AGENT_SKILL_NAME", "stage3-agent")
MAX_STANDALONE_TASKS = int(os.environ.get("AGENT_STANDALONE_MAX_TASKS", "20"))

# Execution mode constants.
MODE_TIEBREAK   = "tiebreak"
MODE_NO_DIMS    = "no_dims"
MODE_STANDALONE = "standalone"


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
    dims_grouped: dict[str, set[str]],
    top_candidates: list,
    pm_task_lookup: dict[str, dict],
    *,
    mode: str = MODE_TIEBREAK,
    all_pm_tasks: list[dict] | None = None,
) -> ClassifierDecision:
    """Ask the configured hermes AIAgent to classify a session into a task.

    `mode` controls prompt adaptation and candidate format:
      MODE_TIEBREAK   — Stage 2 ran; break the tie among top_candidates (default)
      MODE_NO_DIMS    — Stage 2 ran; Stage 1 disabled — no rule-derived dims
      MODE_STANDALONE — Stage 1+2 disabled; pick from all_pm_tasks + infer dims
    """
    sid = int(session["id"])
    with tracer.start_as_current_span("task_classifier_agent.decide") as span:
        span.set_attribute("session_id", sid)
        span.set_attribute("model", MODEL or "")
        span.set_attribute("mode", mode)
        span.set_attribute("candidates_count",
                           len(all_pm_tasks) if mode == MODE_STANDALONE else len(top_candidates))
        result = _classify_session_inner(
            session, dims_grouped, top_candidates, pm_task_lookup, sid,
            mode=mode, all_pm_tasks=all_pm_tasks,
        )
        span.set_attribute("method", result.method)
        span.set_attribute("routing", result.routing)
        span.set_attribute("confidence", float(result.confidence))
        if result.chosen_task_key:
            span.set_attribute("chosen_task_key", result.chosen_task_key)
        span.set_attribute("agent_latency_ms", int(result.elapsed_s * 1000))
        return result


def _classify_session_inner(
    session: dict,
    dims_grouped: dict[str, set[str]],
    top_candidates: list,
    pm_task_lookup: dict[str, dict],
    sid: int,
    *,
    mode: str = MODE_TIEBREAK,
    all_pm_tasks: list[dict] | None = None,
) -> ClassifierDecision:
    standalone_tasks: list[dict] | None = None
    if mode == MODE_STANDALONE:
        if not all_pm_tasks:
            return ClassifierDecision(
                session_id=sid, chosen_task_key=None, confidence=0.0,
                reasoning="no pm_tasks for standalone mode", routing="skip",
                method="agent_unavailable",
            )
        standalone_tasks = _prefilter_tasks(session, all_pm_tasks, MAX_STANDALONE_TASKS)
        valid_keys = {t["task_key"] for t in standalone_tasks}
    else:
        valid_keys = {c.task_key for c in top_candidates}
        if not valid_keys:
            return ClassifierDecision(
                session_id=sid, chosen_task_key=None, confidence=0.0,
                reasoning="no candidates", routing="skip", method="agent_unavailable",
            )

    user_message = build_user_message(
        session, dims_grouped, top_candidates, pm_task_lookup,
        mode=mode,
        mode_tiebreak=MODE_TIEBREAK,
        mode_no_dims=MODE_NO_DIMS,
        mode_standalone=MODE_STANDALONE,
        standalone_tasks=standalone_tasks,
    )
    log.debug("task_classifier_agent user message:\n%s", user_message)

    try:
        base_prompt = load_skill(SKILL_NAME)
    except FileNotFoundError as exc:
        return ClassifierDecision(
            session_id=sid, chosen_task_key=None, confidence=0.0,
            reasoning=str(exc), routing="skip", method="agent_unavailable",
        )

    system_prompt = build_system_prompt(SKILL_NAME, mode, base_prompt)

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

    log.info("task_classifier_agent: model=%s base_url=%s skill=%s mode=%s",
             MODEL, BASE_URL, SKILL_NAME, mode)

    t0 = time.time()
    try:
        agent = AIAgent(
            model=MODEL,
            base_url=BASE_URL,
            api_key=API_KEY or "none",
            ephemeral_system_prompt=system_prompt,
            enabled_toolsets=[],
            quiet_mode=True,
            skip_context_files=True,
            load_soul_identity=False,
            skip_memory=True,
            max_iterations=1,
            max_tokens=MAX_TOKENS,
        )
        result = agent.run_conversation(user_message)
    except Exception as exc:
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
            debug={"error": err, "model": MODEL, "base_url": BASE_URL},
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
            "model":         MODEL,
            "base_url":      BASE_URL,
            "n_candidates":  len(valid_keys),
            "auto_floor":    AUTO_FLOOR,
            "queue_floor":   QUEUE_FLOOR,
            "skill":         SKILL_NAME,
            "mode":          mode,
        },
    )


__all__ = [
    "ClassifierDecision", "classify_session", "AUTO_FLOOR", "QUEUE_FLOOR",
    "MODE_TIEBREAK", "MODE_NO_DIMS", "MODE_STANDALONE",
]
