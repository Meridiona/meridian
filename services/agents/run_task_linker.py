"""run_task_linker — task classification via hermes AIAgent (Python library).

Reads a JSON payload from stdin: {"session_ids": [int, ...], "meridian_db": str}
Fetches all required data (session, recent context, open tickets) from the DB,
calls hermes AIAgent for each session, writes results to stdout.

Hermes memory is enabled so the agent learns developer patterns over time.
Memory is stored in HERMES_HOME/memories/ (services/.hermes/memories/).
"""
from __future__ import annotations

import contextlib
import json
import logging
import os
import sqlite3 as _sqlite3
import sys
import time
from pathlib import Path
from typing import Any

from opentelemetry import trace as trace_api

from agents._hermes_setup import ensure_hermes_importable
from agents import observability
from agents._prompts import build_user_message
from agents._parser import parse_response
from agents.config import MODEL, BASE_URL, API_KEY, AGENT_MAX_TOKENS

log = logging.getLogger("agents.run_task_linker")
tracer = observability.setup("meridian-task-linker")

_CONTEXT_WINDOW = 5


def _fetch_session(con: _sqlite3.Connection, session_id: int) -> dict[str, Any] | None:
    row = con.execute(
        "SELECT id, app_name, started_at, ended_at, duration_s, session_text,"
        "       session_text_source, window_titles, category, confidence"
        " FROM app_sessions WHERE id = ?",
        (session_id,),
    ).fetchone()
    if row is None:
        return None
    return dict(row)


def _fetch_recent_sessions(con: _sqlite3.Connection, before_id: int) -> list[dict[str, Any]]:
    rows = con.execute(
        "SELECT app_name, started_at, duration_s, task_key, task_routing, category,"
        "       COALESCE(SUBSTR(session_text, 1, 200), '') AS text_excerpt"
        " FROM app_sessions"
        " WHERE id < ? AND duration_s > 1 AND COALESCE(session_text,'') != ''"
        " ORDER BY id DESC LIMIT ?",
        (before_id, _CONTEXT_WINDOW),
    ).fetchall()
    result = [dict(r) for r in rows]
    result.reverse()
    return result


def _fetch_pm_tasks(con: _sqlite3.Connection) -> list[dict[str, Any]]:
    rows = con.execute(
        "SELECT task_key, title,"
        "       COALESCE(description_text,'') AS description_text,"
        "       COALESCE(status,'') AS status,"
        "       COALESCE(status_category,'') AS status_category,"
        "       COALESCE(issue_type,'') AS issue_type,"
        "       COALESCE(epic_title,'') AS epic_title,"
        "       COALESCE(sprint_name,'') AS sprint_name"
        " FROM pm_tasks WHERE LOWER(status_category) != 'done'",
    ).fetchall()
    return [dict(r) for r in rows]


def _classify_one(
    session_id: int,
    db_path: str,
    con: _sqlite3.Connection,
) -> dict[str, Any]:
    sid = session_id
    session_raw = _fetch_session(con, sid)
    if session_raw is None:
        return {
            "session_id":   sid,
            "task_key":     None,
            "confidence":   0.0,
            "session_type": "overhead",
            "reasoning":    f"session {sid} not found in DB",
            "method":       "llm_error",
            "dimensions":   {},
            "elapsed_s":    0.0,
        }

    pm_tasks = _fetch_pm_tasks(con)
    recent_sessions = _fetch_recent_sessions(con, sid)

    session = {
        "id":                  sid,
        "app_name":            session_raw.get("app_name"),
        "started_at":          session_raw.get("started_at", ""),
        "ended_at":            session_raw.get("ended_at", ""),
        "duration_s":          session_raw.get("duration_s"),
        "session_text":        session_raw.get("session_text", ""),
        "session_text_source": session_raw.get("session_text_source", "unknown"),
        "window_titles":       json.loads(session_raw.get("window_titles") or "[]"),
        "category":            session_raw.get("category"),
        "confidence":          session_raw.get("confidence", 0.0),
        "audio_snippets":      [],
    }
    valid_keys = {t["task_key"] for t in pm_tasks}

    user_message = build_user_message(session, pm_tasks, recent_sessions=recent_sessions)

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
    try:
        payload = json.loads(sys.stdin.read())
    except (json.JSONDecodeError, ValueError) as exc:
        log.error("run_task_linker: malformed stdin JSON: %s", exc)
        sys.exit(1)

    traceparent = payload.get("traceparent")
    parent_ctx = observability.extract_parent_context(traceparent)

    sessions: list[dict[str, Any]] = payload.get("sessions", [])
    pm_tasks: list[dict[str, Any]] = payload.get("pm_tasks", [])

    # Input validation
    if not db_path:
        log.error("run_task_linker: meridian_db path is empty")
        sys.exit(1)

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
