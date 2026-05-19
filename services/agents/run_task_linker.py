"""run_task_linker — thin hermes bridge for batch session→task classification.

Reads a JSON batch from stdin, calls classify_session() for each session,
writes a single JSON object with all results to stdout.

No DB access, no cursor management, no orchestration — pure function: JSON in,
JSON out. Intended to be spawned by the Rust daemon as a subprocess.
"""
from __future__ import annotations

import contextlib
import json
import logging
import sys
import time
from typing import Any

from opentelemetry import trace as trace_api

from agents._hermes_setup import ensure_hermes_importable
from agents import observability
from agents.task_classifier_agent import classify_session, ClassifierDecision

log = logging.getLogger("agents.run_task_linker")
tracer = observability.setup("meridian-task-linker")


def _build_session(raw: dict[str, Any]) -> dict[str, Any]:
    """Normalise an input session dict to the shape _format_session() expects."""
    return {
        "id":             raw["id"],
        "app_name":       raw.get("app_name"),
        "duration_s":     raw.get("duration_s"),
        "session_text":   raw.get("session_text", ""),
        "window_titles":  raw.get("window_titles", []),
        "category":       raw.get("category"),
        "confidence":     raw.get("confidence", 0.0),
        "audio_snippets": raw.get("audio_snippets", []),
    }


def _classify_one(
    session_raw: dict[str, Any],
    pm_tasks: list[dict[str, Any]],
) -> dict[str, Any]:
    """Run classify_session for one session; return a result dict."""
    sid = int(session_raw["id"])
    session = _build_session(session_raw)

    with tracer.start_as_current_span(
        "run_task_linker.classify_one",
        attributes={
            "session.id":        sid,
            "session.app_name":  session.get("app_name", ""),
            "session.duration_s": session.get("duration_s", 0),
            "session.text_len":  len(session.get("session_text", "")),
        },
    ) as span:
        t0 = time.time()
        try:
            # Redirect stdout → stderr for the duration of the hermes call so that
            # any print() statements inside AIAgent.__init__ or the LLM call do not
            # contaminate the JSON output that Rust reads from our stdout.
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
            "session_id":  sid,
            "task_key":    result.chosen_task_key,
            "confidence":  result.confidence,
            "routing":     result.routing,
            "reasoning":   result.reasoning,
            "method":      method,
            "dimensions":  result.dimensions,
            "elapsed_s":   result.elapsed_s,
            "llm_model":   result.debug.get("model"),
            "llm_runtime": result.debug.get("llm_runtime", "cloud"),
            "llm_is_local": result.debug.get("llm_is_local", False),
        }


def main() -> None:
    ensure_hermes_importable()

    try:
        raw_input = sys.stdin.read()
        payload = json.loads(raw_input)
    except (json.JSONDecodeError, ValueError) as exc:
        log.error("run_task_linker: malformed stdin JSON: %s", exc)
        sys.exit(1)

    traceparent = payload.get("traceparent")
    parent_ctx = observability.extract_parent_context(traceparent)

    sessions: list[dict[str, Any]] = payload.get("sessions", [])
    pm_tasks: list[dict[str, Any]] = payload.get("pm_tasks", [])

    log.info("run_task_linker: received %d sessions, %d pm_tasks", len(sessions), len(pm_tasks))

    with tracer.start_as_current_span(
        "run_task_linker.batch",
        context=parent_ctx,
        attributes={
            "sessions.count":  len(sessions),
            "pm_tasks.count":  len(pm_tasks),
        },
    ) as batch_span:
        results: list[dict[str, Any]] = []
        for session_raw in sessions:
            log.info(
                "run_task_linker: classifying session %d (app=%s dur=%ss text_len=%d)",
                int(session_raw.get("id", 0)),
                session_raw.get("app_name"),
                session_raw.get("duration_s"),
                len(session_raw.get("session_text", "")),
            )
            result = _classify_one(session_raw, pm_tasks)
            results.append(result)
            log.info(
                "run_task_linker: session_id=%d task_key=%s routing=%s model=%s runtime=%s elapsed_s=%.2f",
                result["session_id"],
                result["task_key"],
                result["routing"],
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
