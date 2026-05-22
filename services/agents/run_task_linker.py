"""run_task_linker — task classification via hermes AIAgent (Python library).

Reads a JSON payload from stdin: {"session_ids": [int, ...], "meridian_db": str}
Fetches all required data (session, recent context, open tickets) from the DB,
calls hermes AIAgent for each session, writes results to stdout.

Hermes memory is enabled so the agent learns developer patterns over time.
Memory is stored in HERMES_HOME/memories/ (services/.hermes/memories/).

LLM selection: if LLM_PREFER_LOCAL=1 (default), select_model_for_hermes() is
called once at startup to find the best available local endpoint (Ollama, LM
Studio, llama.cpp, mlx_lm, or Apple Intelligence). The selected endpoint is
used for all sessions in the batch. Falls back to cloud config on failure.
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

from opentelemetry import trace
from opentelemetry.trace import StatusCode

# HERMES_HOME must be set before any hermes package is imported.
# tools/skills_tool.SKILLS_DIR is a module-level constant computed at first
# import — if HERMES_HOME is set after that import, it reads ~/.hermes instead.
_SERVICES_DIR = Path(__file__).parent.parent
os.environ.setdefault("HERMES_HOME", str(_SERVICES_DIR / ".hermes"))

from agents import observability
from agents._hermes_setup import ensure_hermes_importable
from agents._prompts import build_user_message, _format_session, _format_candidates
from agents._parser import parse_response
from agents._system_context import SYSTEM_CONTEXT
from agents.config import MODEL, BASE_URL, API_KEY, AGENT_MAX_TOKENS, LLM_PREFER_LOCAL
from agents.llm_selector import select_model_for_hermes

ensure_hermes_importable()
from run_agent import AIAgent  # noqa: E402

log = logging.getLogger("agents.run_task_linker")
tracer = observability.setup("meridian-task-linker")

_CONTEXT_WINDOW = 5

# Resolve LLM endpoint once per process — local first, cloud fallback.
def _resolve_llm() -> tuple[str, str, str]:
    """Return (model, base_url, api_key) for this batch.

    Tries select_model_for_hermes() when LLM_PREFER_LOCAL is enabled; falls
    back to the static cloud config on any error or when no local model fits.
    """
    if LLM_PREFER_LOCAL:
        try:
            ep = select_model_for_hermes()
            if ep is not None:
                log.info(
                    "run_task_linker: using local model=%s runtime=%s base_url=%s",
                    ep.model, ep.runtime, ep.base_url,
                )
                return ep.model, ep.base_url, ep.api_key
        except Exception as exc:  # noqa: BLE001
            log.warning("run_task_linker: llm_selector failed, using cloud fallback: %s", exc)
    log.info("run_task_linker: using cloud model=%s base_url=%s", MODEL, BASE_URL)
    return MODEL, BASE_URL, API_KEY or "none"


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
        "       COALESCE(status_category,'') AS status_category,"
        "       COALESCE(issue_type,'') AS issue_type,"
        "       COALESCE(parent_key,'') AS parent_key,"
        "       COALESCE(epic_title,'') AS epic_title,"
        "       COALESCE(sprint_name,'') AS sprint_name"
        " FROM pm_tasks WHERE LOWER(status_category) != 'done' AND parent_key IS NULL",
    ).fetchall()
    return [dict(r) for r in rows]


def _classify_one(
    session_id: int,
    db_path: str,
    con: _sqlite3.Connection,
    *,
    agent: AIAgent,
    llm_model: str,
    llm_base_url: str,
) -> dict[str, Any]:
    _tracer = trace.get_tracer("agents.run_task_linker")
    sid = session_id

    # ── db_fetch ──────────────────────────────────────────────────────────────
    with _tracer.start_as_current_span("db_fetch") as db_span:
        session_raw = _fetch_session(con, sid)
        if session_raw is None:
            db_span.set_attribute("pm_tasks_count", 0)
            db_span.set_attribute("recent_sessions_count", 0)
            db_span.add_event("session_not_found", {"session_id": sid})
            db_span.set_status(StatusCode.ERROR, f"session {sid} not found in DB")
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
        db_span.set_attribute("pm_tasks_count", len(pm_tasks))
        db_span.set_attribute("recent_sessions_count", len(recent_sessions))

        session_text = session_raw.get("session_text") or ""
        _session_for_fmt = {
            **session_raw,
            "window_titles": json.loads(session_raw.get("window_titles") or "[]"),
        }
        db_span.add_event("session_loaded", {
            "app_name":           str(session_raw.get("app_name") or ""),
            "duration_s":         float(session_raw.get("duration_s") or 0.0),
            "category":           str(session_raw.get("category") or ""),
            "session_text_chars": len(session_text),
            "text_source":        str(session_raw.get("session_text_source") or ""),
            "session_formatted":  _format_session(_session_for_fmt)[:2000],
        })
        recent_task_keys = [r.get("task_key") or "-" for r in recent_sessions if r.get("task_key")]
        db_span.add_event("context_loaded", {
            "pm_tasks_count":        len(pm_tasks),
            "recent_sessions_count": len(recent_sessions),
            "recent_task_keys":      ", ".join(recent_task_keys) if recent_task_keys else "-",
            "pm_tasks_formatted":    _format_candidates(pm_tasks)[:3000],
        })

    session = {
        "id":                  sid,
        "app_name":            session_raw.get("app_name"),
        "started_at":          session_raw.get("started_at", ""),
        "ended_at":            session_raw.get("ended_at", ""),
        "duration_s":          session_raw.get("duration_s"),
        "session_text":        session_text,
        "session_text_source": session_raw.get("session_text_source", "unknown"),
        "window_titles":       json.loads(session_raw.get("window_titles") or "[]"),
        "category":            session_raw.get("category"),
        "confidence":          session_raw.get("confidence", 0.0),
        "audio_snippets":      [],
    }
    valid_keys = {t["task_key"] for t in pm_tasks}

    # ── build_prompt ──────────────────────────────────────────────────────────
    with _tracer.start_as_current_span("build_prompt") as bp_span:
        bp_span.set_attribute("pm_tasks_count", len(pm_tasks))
        bp_span.set_attribute("recent_sessions_count", len(recent_sessions))
        user_message = build_user_message(session, pm_tasks, recent_sessions=recent_sessions)
        bp_span.set_attribute("prompt_chars", len(user_message))
        recent_with_task = sum(1 for r in recent_sessions if r.get("task_key"))
        bp_span.add_event("prompt_assembled", {
            "pm_tasks_included":        len(pm_tasks),
            "recent_sessions_included": len(recent_sessions),
            "recent_with_task_key":     recent_with_task,
            "session_text_chars":       len(session_text),
            "prompt_chars":             len(user_message),
            "prompt_text":              user_message[:3000],
        })

    # ── llm_inference ─────────────────────────────────────────────────────────
    t0 = time.time()
    with _tracer.start_as_current_span("llm_inference") as llm_span:
        llm_span.set_attribute("model", llm_model)
        llm_span.set_attribute("base_url", llm_base_url)
        llm_span.set_attribute("prompt_chars", len(user_message))
        try:
            llm_span.add_event("agent_run", {
                "model":           llm_model,
                "base_url":        llm_base_url,
                "max_iterations":  10,
                "memory_enabled":  False,
                "toolsets":        "none",
            })

            with contextlib.redirect_stdout(sys.stderr):
                result = agent.run_conversation(user_message)

            elapsed = time.time() - t0
            iterations = 0
            response_chars = 0
            if isinstance(result, dict):
                iterations = result.get("iterations") or result.get("turns") or 0
                raw_preview = str(result.get("final_response") or result.get("response") or "")
                response_chars = len(raw_preview)

            llm_span.set_attribute("outcome", "hermes_aiagent")
            llm_span.set_attribute("elapsed_s", elapsed)
            llm_span.set_attribute("iterations", iterations)
            llm_span.add_event("conversation_complete", {
                "elapsed_s":      elapsed,
                "iterations":     iterations,
                "response_chars": response_chars,
                "response":       raw_preview[:3000],
            })

        except Exception as exc:  # noqa: BLE001
            elapsed = time.time() - t0
            llm_span.set_attribute("outcome", "llm_error")
            llm_span.set_attribute("elapsed_s", elapsed)
            llm_span.set_status(StatusCode.ERROR, str(exc))
            llm_span.add_event("agent_error", {
                "error_type":    type(exc).__name__,
                "error_message": str(exc)[:500],
                "elapsed_s":     elapsed,
            })
            log.warning("run_task_linker: AIAgent failed for session %d: %s", sid, exc)
            return {
                "session_id":   sid,
                "task_key":     None,
                "confidence":   0.0,
                "session_type": "overhead",
                "reasoning":    f"agent error: {exc}",
                "method":       "llm_error",
                "dimensions":   {},
                "elapsed_s":    elapsed,
            }

    raw = ""
    if isinstance(result, dict):
        raw = str(result.get("final_response") or result.get("response") or "").strip()

    log.debug("run_task_linker: session %d raw (%.1fs): %.200s", sid, elapsed, raw)

    # ── parse_response ────────────────────────────────────────────────────────
    with _tracer.start_as_current_span("parse_response") as pr_span:
        pr_span.set_attribute("raw_chars", len(raw))
        # Always emit the raw LLM output — this is the single most useful event
        # for debugging parse failures: you can see exactly what the model returned.
        pr_span.add_event("raw_llm_response", {
            "chars":   len(raw),
            "preview": raw[:500],
        })

        task_key, confidence, reasoning, dimensions, session_type, err = parse_response(raw, valid_keys)
        if err:
            pr_span.set_attribute("outcome", "llm_parse_error")
            pr_span.set_attribute("task_key", "-")
            pr_span.set_attribute("confidence", 0.0)
            pr_span.set_status(StatusCode.ERROR, err)
            pr_span.add_event("parse_failure", {
                "error":       err,
                "raw_preview": raw[:300],
            })
            log.warning("run_task_linker: parse error for session %d: %s", sid, err)
            return {
                "session_id":   sid,
                "task_key":     None,
                "confidence":   0.0,
                "session_type": "overhead",
                "reasoning":    err,
                "method":       "llm_parse_error",
                "dimensions":   {},
                "elapsed_s":    elapsed,
            }

        pr_span.set_attribute("outcome", "ok")
        pr_span.set_attribute("task_key", task_key if task_key is not None else "-")
        pr_span.set_attribute("confidence", confidence)
        pr_span.add_event("parse_success", {
            "task_key":        task_key if task_key is not None else "-",
            "confidence":      confidence,
            "session_type":    session_type,
            "dimensions_count": len(dimensions),
            "dimension_keys":  ", ".join(sorted(dimensions.keys())),
        })

    return {
        "session_id":   sid,
        "task_key":     task_key,
        "confidence":   confidence,
        "session_type": session_type,
        "reasoning":    reasoning,
        "method":       "hermes_aiagent",
        "dimensions":   dimensions,
        "elapsed_s":    elapsed,
    }


def main() -> None:
    try:
        payload = json.loads(sys.stdin.read())
    except (json.JSONDecodeError, ValueError) as exc:
        log.error("run_task_linker: malformed stdin JSON: %s", exc)
        sys.exit(1)

    session_ids: list[int] = payload.get("session_ids", [])
    db_path: str = payload.get("meridian_db", "")
    traceparent: str | None = payload.get("traceparent")

    # Input validation
    if not db_path:
        log.error("run_task_linker: meridian_db path is empty")
        sys.exit(1)

    if not Path(db_path).exists():
        log.error("run_task_linker: db file does not exist: %s", db_path)
        sys.exit(1)

    if not session_ids:
        log.info("run_task_linker: no session_ids provided, nothing to do")
        sys.stdout.write(json.dumps({"results": []}))
        sys.stdout.write("\n")
        sys.stdout.flush()
        return

    log.info("run_task_linker: %d sessions, db=%s", len(session_ids), db_path)

    ctx = observability.extract_parent_context(traceparent)

    with tracer.start_as_current_span("run_task_linker", context=ctx) as root_span:
        root_span.set_attribute("session_count", len(session_ids))
        root_span.set_attribute("db_path", Path(db_path).name)

        # ── llm_selection ─────────────────────────────────────────────────────
        with tracer.start_as_current_span("llm_selection") as sel_span:
            llm_model, llm_base_url, llm_api_key = _resolve_llm()
            is_local = llm_base_url != BASE_URL
            runtime = "local" if is_local else "cloud"
            sel_span.set_attribute("model", llm_model)
            sel_span.set_attribute("runtime", runtime)
            sel_span.set_attribute("is_local", is_local)

        with contextlib.redirect_stdout(sys.stderr):
            agent = AIAgent(
                model=llm_model,
                base_url=llm_base_url,
                api_key=llm_api_key,
                enabled_toolsets=[],
                max_iterations=10,
                quiet_mode=True,
                skip_context_files=True,
                load_soul_identity=False,
                skip_memory=True,
                tool_delay=0.0,
                max_tokens=AGENT_MAX_TOKENS,
                ephemeral_system_prompt=SYSTEM_CONTEXT,
            )

        con = _sqlite3.connect(db_path)
        con.row_factory = _sqlite3.Row
        try:
            results: list[dict[str, Any]] = []
            for session_id in session_ids:
                log.info("run_task_linker: classifying session %d", session_id)

                # ── classify_session ──────────────────────────────────────────
                with tracer.start_as_current_span("classify_session") as cls_span:
                    cls_span.set_attribute("session_id", session_id)
                    result = _classify_one(
                        session_id, db_path, con,
                        agent=agent,
                        llm_model=llm_model,
                        llm_base_url=llm_base_url,
                    )
                    _row = con.execute(
                        "SELECT app_name, duration_s FROM app_sessions WHERE id = ?",
                        (session_id,),
                    ).fetchone()
                    if _row:
                        cls_span.set_attribute("app_name", _row[0] or "unknown")
                        cls_span.set_attribute("duration_s", float(_row[1] or 0.0))

                results.append(result)
                log.info(
                    "run_task_linker: session_id=%d task_key=%s session_type=%s elapsed_s=%.2f",
                    result["session_id"],
                    result["task_key"],
                    result["session_type"],
                    result["elapsed_s"],
                )
        finally:
            con.close()

    sys.stdout.write(json.dumps({"results": results}))
    sys.stdout.write("\n")
    sys.stdout.flush()


if __name__ == "__main__":
    main()
