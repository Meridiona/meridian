"""run_task_linker — thin hermes bridge for batch session→task classification.

Reads a JSON batch from stdin, calls classify_session() in MODE_STANDALONE on
each session, writes a single JSON object with all results to stdout.

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

from agents._hermes_setup import ensure_hermes_importable
from agents import observability
from agents.task_classifier_agent import classify_session, ClassifierDecision, MODE_STANDALONE

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
    pm_task_lookup: dict[str, dict[str, Any]],
    all_pm_tasks: list[dict[str, Any]],
) -> dict[str, Any]:
    """Run classify_session for one session; return a result dict."""
    sid = int(session_raw["id"])
    session = _build_session(session_raw)

    t0 = time.time()
    try:
        # Redirect stdout → stderr for the duration of the hermes call so that
        # any print() statements inside AIAgent.__init__ or the LLM call do not
        # contaminate the JSON output that Rust reads from our stdout.
        with contextlib.redirect_stdout(sys.stderr):
            result: ClassifierDecision = classify_session(
                session,
                dims_grouped={},        # no Stage 1/2 dims available
                top_candidates=[],      # not used in MODE_STANDALONE
                pm_task_lookup=pm_task_lookup,
                mode=MODE_STANDALONE,
                all_pm_tasks=all_pm_tasks,
            )
    except Exception as exc:  # noqa: BLE001
        elapsed = time.time() - t0
        log.warning("run_task_linker: exception for session %d: %s", sid, exc)
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

    return {
        "session_id": sid,
        "task_key":   result.chosen_task_key,
        "confidence": result.confidence,
        "routing":    result.routing,
        "reasoning":  result.reasoning,
        "method":     method,
        "dimensions": result.dimensions,
        "elapsed_s":  result.elapsed_s,
    }


def main() -> None:
    ensure_hermes_importable()

    try:
        raw_input = sys.stdin.read()
        payload = json.loads(raw_input)
    except (json.JSONDecodeError, ValueError) as exc:
        log.error("run_task_linker: malformed stdin JSON: %s", exc)
        sys.exit(1)

    sessions: list[dict[str, Any]] = payload.get("sessions", [])
    pm_tasks: list[dict[str, Any]] = payload.get("pm_tasks", [])
    pm_task_lookup: dict[str, dict[str, Any]] = {t["task_key"]: t for t in pm_tasks}

    log.info("run_task_linker: received %d sessions, %d pm_tasks", len(sessions), len(pm_tasks))

    results: list[dict[str, Any]] = []
    for session_raw in sessions:
        log.info(
            "run_task_linker: classifying session %d (app=%s dur=%ss text_len=%d)",
            int(session_raw.get("id", 0)),
            session_raw.get("app_name"),
            session_raw.get("duration_s"),
            len(session_raw.get("session_text", "")),
        )
        result = _classify_one(session_raw, pm_task_lookup, pm_tasks)
        results.append(result)
        log.info(
            "run_task_linker: session_id=%d task_key=%s routing=%s elapsed_s=%.2f",
            result["session_id"],
            result["task_key"],
            result["routing"],
            result["elapsed_s"],
        )

    sys.stdout.write(json.dumps({"results": results}))
    sys.stdout.write("\n")
    sys.stdout.flush()


if __name__ == "__main__":
    main()
