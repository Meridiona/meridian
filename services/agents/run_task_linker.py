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

# HERMES_HOME must be set before any hermes package is imported.
# tools/skills_tool.SKILLS_DIR is a module-level constant computed at first
# import — if HERMES_HOME is set after that import, it reads ~/.hermes instead.
_SERVICES_DIR = Path(__file__).parent.parent
os.environ.setdefault("HERMES_HOME", str(_SERVICES_DIR / ".hermes"))

from agents import observability
from agents._prompts import build_user_message
from agents._parser import parse_response
from agents._system_context import SYSTEM_CONTEXT
from agents.config import MODEL, BASE_URL, API_KEY, AGENT_MAX_TOKENS, LLM_PREFER_LOCAL
from agents.llm_selector import select_model_for_hermes

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
    *,
    llm_model: str,
    llm_base_url: str,
    llm_api_key: str,
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

    t0 = time.time()
    try:
        from run_agent import AIAgent

        agent = AIAgent(
            # ── Connection ────────────────────────────────────────────
            model=llm_model,
            base_url=llm_base_url,
            api_key=llm_api_key,
            # provider=None,                # omit — hermes infers from base_url
            # api_mode=None,                # omit — default "chat_completions"
            # fallback_model=None,          # omit — no fallback needed

            # ── Toolsets ──────────────────────────────────────────────
            # Valid names come from toolsets.get_all_toolsets() — not documented on AIAgent.
            # "memory"  → enables tool: memory        (writes learned patterns to HERMES_HOME/memories/)
            # "skills"  → enables tools: skills_list, skill_view, skill_manage
            #             (background skill-review thread can patch task-classifier/SKILL.md)
            enabled_toolsets=["memory", "skills"],
            # disabled_toolsets=None,       # omit — using enabled_toolsets allowlist instead

            # ── Limits ────────────────────────────────────────────────
            max_iterations=10,               # 1 classification + up to 2 memory/skill writes
            max_tokens=AGENT_MAX_TOKENS,    # cap response size
            # tool_delay=1.0,               # omit — default 1s is fine
            # reasoning_config=None,        # omit — let hermes pick effort level

            # ── Output / logging ──────────────────────────────────────
            quiet_mode=True,                # suppress hermes progress lines (we redirect stdout anyway)
            # verbose_logging=False,        # omit — we use our own logging module
            # save_trajectories=False,      # omit — not persisting JSONL traces
            # log_prefix="",               # omit — single-process, no prefix needed

            # ── Context injection ─────────────────────────────────────
            skip_context_files=True,        # don't inject SOUL.md / AGENTS.md / .cursorrules
            load_soul_identity=False,       # don't load SOUL.md even partially
            skip_memory=False,              # persistent memory ON — self-learning enabled
            ephemeral_system_prompt=SYSTEM_CONTEXT,  # shared with server.py for consistency

            # ── Session / platform (not applicable in daemon context) ─
            # session_id=None,              # omit — auto-generated per call
            # platform=None,               # omit — not cli/telegram/discord
            # user_id=None,                # omit — single-user daemon
            # prefill_messages=None,        # omit — no few-shot priming needed
            # checkpoints_enabled=False,    # omit — not needed for short classification calls
        )

        with contextlib.redirect_stdout(sys.stderr):
            result = agent.run_conversation(user_message)

    except Exception as exc:  # noqa: BLE001
        elapsed = time.time() - t0
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

    elapsed = time.time() - t0

    raw = ""
    if isinstance(result, dict):
        raw = str(result.get("final_response") or result.get("response") or "").strip()

    log.debug("run_task_linker: session %d raw (%.1fs): %.200s", sid, elapsed, raw)

    task_key, confidence, reasoning, dimensions, session_type, err = parse_response(raw, valid_keys)
    if err:
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

    llm_model, llm_base_url, llm_api_key = _resolve_llm()

    con = _sqlite3.connect(db_path)
    con.row_factory = _sqlite3.Row
    try:
        results: list[dict[str, Any]] = []
        for session_id in session_ids:
            log.info("run_task_linker: classifying session %d", session_id)
            result = _classify_one(
                session_id, db_path, con,
                llm_model=llm_model,
                llm_base_url=llm_base_url,
                llm_api_key=llm_api_key,
            )
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
