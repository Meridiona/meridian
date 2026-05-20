"""run_task_linker — JSON bridge between the Rust daemon and the hermes task classifier.

Reads a JSON payload from stdin:
  {"sessions": [session_dict, ...], "pm_tasks": [task_dict, ...]}

Sessions and pm_tasks are pre-fetched by the Rust daemon. Calls classify_session()
from task_classifier_agent for each session and writes results to stdout:
  {"results": [result_dict, ...]}
"""
from __future__ import annotations

import contextlib
import json
import logging
import sys
import time
from typing import Any

from opentelemetry import trace as trace_api

from agents import observability
from agents.task_classifier_agent import classify_session, ClassifierDecision

log = logging.getLogger("agents.run_task_linker")
tracer = observability.setup("meridian-task-linker")


def _build_session(session_raw: dict[str, Any]) -> dict[str, Any]:
    """Normalise a raw session dict from the Rust payload into the shape classify_session expects."""
    wt = session_raw.get("window_titles") or []
    if isinstance(wt, str):
        try:
            wt = json.loads(wt)
        except (json.JSONDecodeError, ValueError):
            wt = []
    return {
        "id":                  session_raw.get("id"),
        "app_name":            session_raw.get("app_name"),
        "started_at":          session_raw.get("started_at", ""),
        "ended_at":            session_raw.get("ended_at", ""),
        "duration_s":          session_raw.get("duration_s"),
        "session_text":        session_raw.get("session_text") or "",
        "session_text_source": session_raw.get("session_text_source", "unknown"),
        "window_titles":       wt,
        "category":            session_raw.get("category"),
        "confidence":          session_raw.get("confidence") or 0.0,
        "audio_snippets":      session_raw.get("audio_snippets") or [],
    }


def _classify_one(
    session_raw: dict[str, Any],
    pm_tasks: Any,
    recent_sessions: list[dict[str, Any]],
) -> dict[str, Any]:
    """Classify a single session. Returns a result dict — never raises."""
    session = _build_session(session_raw)
    sid = int(session.get("id") or 0)

    with tracer.start_as_current_span(
        "run_task_linker.classify_one",
        attributes={
            "session.id":         sid,
            "session.app_name":   session.get("app_name") or "",
            "session.duration_s": session.get("duration_s") or 0,
            "session.text_len":   len(session.get("session_text", "")),
        },
    ) as span:
        t0 = time.time()
        try:
            with contextlib.redirect_stdout(sys.stderr):
                result: ClassifierDecision = classify_session(session, pm_tasks)
        except Exception as exc:  # noqa: BLE001
            elapsed = time.time() - t0
            log.warning("run_task_linker: exception for session %d: %s", sid, exc)
            span.record_exception(exc)
            span.set_status(trace_api.StatusCode.ERROR, str(exc))
            return {
                "session_id": sid,
                "task_key":   None,
                "confidence": 0.0,
                "routing":    "skip",
                "reasoning":  f"exception: {exc}",
                "method":     "llm_error",
                "dimensions": {},
                "elapsed_s":  elapsed,
            }

        method = "llm_standalone" if result.method == "task_classifier" else result.method

        span.set_attribute("result.routing",    result.routing)
        span.set_attribute("result.task_key",   result.chosen_task_key or "")
        span.set_attribute("result.confidence", result.confidence)
        span.set_attribute("result.method",     method)

        return {
            "session_id":   sid,
            "task_key":     result.chosen_task_key,
            "confidence":   result.confidence,
            "routing":      result.routing,
            "reasoning":    result.reasoning,
            "method":       method,
            "dimensions":   result.dimensions,
            "elapsed_s":    result.elapsed_s,
            "llm_model":    result.debug.get("model"),
            "llm_runtime":  result.debug.get("llm_runtime", "cloud"),
            "llm_is_local": result.debug.get("llm_is_local", False),
        }


def main() -> None:
    try:
        payload = json.loads(sys.stdin.read())
    except (json.JSONDecodeError, ValueError) as exc:
        log.error("run_task_linker: malformed stdin JSON: %s", exc)
        sys.exit(1)

    traceparent = payload.get("traceparent")
    parent_ctx = observability.extract_parent_context(traceparent)

    sessions: list[dict[str, Any]] = payload.get("sessions", [])
    pm_tasks: list[dict[str, Any]] = payload.get("pm_tasks", [])

    if not isinstance(sessions, list) or not isinstance(pm_tasks, list):
        log.error("run_task_linker: 'sessions' and 'pm_tasks' must be lists")
        sys.exit(1)

    if not sessions:
        sys.stdout.write(json.dumps({"results": []}))
        sys.stdout.write("\n")
        sys.stdout.flush()
        return

    with tracer.start_as_current_span(
        "run_task_linker.batch",
        context=parent_ctx,
        attributes={
            "sessions.count": len(sessions),
            "pm_tasks.count": len(pm_tasks),
        },
    ):
        results: list[dict[str, Any]] = []
        for session_raw in sessions:
            log.info(
                "run_task_linker: classifying session %d (app=%s dur=%ss text_len=%d)",
                int(session_raw.get("id", 0)),
                session_raw.get("app_name"),
                session_raw.get("duration_s"),
                len(session_raw.get("session_text", "")),
            )
            result = _classify_one(session_raw, pm_tasks, [])
            results.append(result)
            log.info(
                "run_task_linker: session_id=%d task_key=%s routing=%s model=%s runtime=%s elapsed_s=%.2f",
                result["session_id"],
                result["task_key"],
                result.get("routing", "?"),
                result.get("llm_model", "?"),
                result.get("llm_runtime", "cloud"),
                result["elapsed_s"],
                extra={
                    "llm_model":    result.get("llm_model"),
                    "llm_runtime":  result.get("llm_runtime"),
                    "llm_is_local": result.get("llm_is_local"),
                },
            )

    sys.stdout.write(json.dumps({"results": results}))
    sys.stdout.write("\n")
    sys.stdout.flush()


if __name__ == "__main__":
    main()
