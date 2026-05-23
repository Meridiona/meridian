#!/Users/adityaharish/Documents/Meridiona/meridian/services/.venv313/bin/python3.13
"""run_task_linker_mlx — direct MLX in-process inference for session classification.

Reads a JSON payload from stdin: {"session_ids": [int, ...], "meridian_db": str}
Loads the MLX model in-process via outlines + mlx_lm (no HTTP server, no Hermes agent
loop, no tool calls), and uses FSM-constrained decoding to guarantee the response is
always a valid SessionClassification JSON object.

Requires: outlines[mlxlm]>=1.2, mlx-lm>=0.22 (installed in .venv313).
Method tag in results: "mlx_direct".
"""
from __future__ import annotations

import json
import logging
import os
import sqlite3 as _sqlite3
import sys
import time
from pathlib import Path
from typing import Any, Literal, Optional

from pydantic import BaseModel, Field

_SERVICES_DIR = Path(__file__).parent.parent
os.environ.setdefault("HERMES_HOME", str(_SERVICES_DIR / ".hermes"))

from agents import observability
from agents._prompts import build_user_message
from agents._system_context import SYSTEM_CONTEXT

log = logging.getLogger("agents.run_task_linker_mlx")
tracer = observability.setup("meridian-task-linker-mlx")

_CONTEXT_WINDOW = 5
_MAX_TOKENS = 1024
_TEMPERATURE = 0.0  # greedy decoding — deterministic classification

_MLX_MODEL_ID = os.environ.get(
    "MLX_MODEL_ID", "mlx-community/Qwen3.5-9B-OptiQ-4bit"
)
_SKILL_PATH = (
    _SERVICES_DIR / "skills" / "activity" / "task-classifier" / "SKILL.md"
)

# If the persistent MLX server is running, use it for inference (avoids cold-load).
_SERVER_PORT = int(os.environ.get("MLX_SERVER_PORT", "7823"))
_SERVER_URL = f"http://127.0.0.1:{_SERVER_PORT}/classify"


# ---------------------------------------------------------------------------
# Pydantic schema — outlines uses this for FSM-constrained decoding, which
# guarantees the model output is always a valid instance of this class.
# ---------------------------------------------------------------------------

class SessionClassification(BaseModel):
    task_key: Optional[str] = Field(
        None,
        description="One of the supplied candidate task keys, or null if none fit.",
    )
    confidence: float = Field(
        ..., ge=0.0, le=1.0,
        description="How certain you are. See scoring heuristics for ranges.",
    )
    session_type: Literal["task", "overhead", "untracked"] = Field(
        ...,
        description=(
            "'task' = matched to a ticket; "
            "'overhead' = idle/personal/unrelated — discarded; "
            "'untracked' = real work with no matching ticket — retained."
        ),
    )
    reasoning: str = Field(
        ..., max_length=500,
        description="1–4 sentences citing window titles, OCR text, or context clues.",
    )
    dimensions: dict[str, list[str]] = Field(
        default_factory=dict,
        description=(
            "Inferred activity tags. Keys: activity, intent, engagement, "
            "collaboration, tool, topic, practice. "
            "Values: lowercase snake_case lists. Omit keys with no evidence."
        ),
    )


# ---------------------------------------------------------------------------
# Skill content — read once at module load, injected into every system prompt.
# ---------------------------------------------------------------------------

def _load_skill() -> str:
    try:
        return _SKILL_PATH.read_text(encoding="utf-8")
    except OSError:
        log.warning("run_task_linker_mlx: SKILL.md not found at %s", _SKILL_PATH)
        return ""


_SKILL_CONTENT = _load_skill()

_SYSTEM_PROMPT = (
    SYSTEM_CONTEXT
    + ("\n\n---\n\n" + _SKILL_CONTENT if _SKILL_CONTENT else "")
)


# ---------------------------------------------------------------------------
# Model loading — cached for the process lifetime.
# outlines.from_mlxlm wraps the already-loaded mlx model; subsequent calls
# skip the expensive disk load.
# ---------------------------------------------------------------------------

_model_cache: dict[str, Any] = {}


def _get_model() -> Any:
    """Return an outlines-wrapped model, loading from disk on the first call."""
    if _MLX_MODEL_ID in _model_cache:
        return _model_cache[_MLX_MODEL_ID]

    try:
        import mlx_lm
        import outlines
    except ImportError as exc:
        raise ImportError(
            f"Required package not installed: {exc}. "
            "Install with: pip install 'mlx-lm>=0.22' 'outlines[mlxlm]>=1.2'"
        ) from exc

    log.info(
        "run_task_linker_mlx: loading %s (first call this process)", _MLX_MODEL_ID
    )
    t0 = time.time()
    mlx_model, tokenizer = mlx_lm.load(
        _MLX_MODEL_ID,
        tokenizer_config={"trust_remote_code": True},
    )
    outlines_model = outlines.from_mlxlm(mlx_model, tokenizer)
    log.info("run_task_linker_mlx: model loaded in %.1fs", time.time() - t0)

    _model_cache[_MLX_MODEL_ID] = outlines_model
    return outlines_model


# ---------------------------------------------------------------------------
# DB helpers
# ---------------------------------------------------------------------------

def _fetch_session(
    con: _sqlite3.Connection, session_id: int
) -> dict[str, Any] | None:
    row = con.execute(
        "SELECT id, app_name, started_at, ended_at, duration_s, session_text,"
        "       session_text_source, window_titles, category, confidence"
        " FROM app_sessions WHERE id = ?",
        (session_id,),
    ).fetchone()
    return dict(row) if row else None


def _fetch_recent_sessions(
    con: _sqlite3.Connection, before_id: int
) -> list[dict[str, Any]]:
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
        " FROM pm_tasks",
    ).fetchall()
    return [dict(r) for r in rows]


# ---------------------------------------------------------------------------
# Core classification
# ---------------------------------------------------------------------------

def _error_result(
    session_id: int, reason: str, elapsed: float, method: str
) -> dict[str, Any]:
    return {
        "session_id":   session_id,
        "task_key":     None,
        "confidence":   0.0,
        "session_type": "overhead",
        "reasoning":    reason,
        "method":       method,
        "dimensions":   {},
        "elapsed_s":    elapsed,
    }


def _classify_one(
    session_id: int,
    con: _sqlite3.Connection,
) -> dict[str, Any]:
    session_raw = _fetch_session(con, session_id)
    if session_raw is None:
        return _error_result(
            session_id, f"session {session_id} not found in DB", 0.0, "mlx_error"
        )

    pm_tasks = _fetch_pm_tasks(con)
    recent   = _fetch_recent_sessions(con, session_id)

    session = {
        "id":                  session_id,
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

    user_message = build_user_message(session, pm_tasks, recent_sessions=recent)
    messages = [
        {"role": "system", "content": _SYSTEM_PROMPT},
        {"role": "user",   "content": user_message},
    ]

    t0 = time.time()
    try:
        from mlx_lm.sample_utils import make_sampler
        from outlines.inputs import Chat

        model = _get_model()
        raw = model(
            Chat(messages),
            output_type=SessionClassification,
            max_tokens=_MAX_TOKENS,
            sampler=make_sampler(temp=_TEMPERATURE),
            verbose=False,
        )
    except Exception as exc:
        elapsed = time.time() - t0
        log.warning(
            "run_task_linker_mlx: inference failed for session %d: %s",
            session_id, exc,
        )
        return _error_result(
            session_id, f"mlx inference error: {exc}", elapsed, "mlx_error"
        )

    elapsed = time.time() - t0
    log.debug(
        "run_task_linker_mlx: session %d raw (%.1fs): %.200s",
        session_id, elapsed, raw,
    )

    # outlines guarantees schema validity; validate_json won't fail.
    # We still do semantic checks: task_key must be in the candidate list.
    try:
        result = SessionClassification.model_validate_json(raw)
    except Exception as exc:
        log.warning(
            "run_task_linker_mlx: schema validation failed for session %d: %s",
            session_id, exc,
        )
        return _error_result(
            session_id, f"schema validation error: {exc}", elapsed, "mlx_parse_error"
        )

    # Semantic guard: task_key must be one of the supplied candidates.
    task_key = result.task_key
    if task_key is not None and task_key not in valid_keys:
        log.warning(
            "run_task_linker_mlx: model returned unknown task_key %r for session %d",
            task_key, session_id,
        )
        return _error_result(
            session_id,
            f"model returned unknown task_key {task_key!r}",
            elapsed,
            "mlx_parse_error",
        )

    # Clamp confidence to [0, 1] in case the model sneaks past schema bounds.
    confidence = max(0.0, min(1.0, result.confidence))

    return {
        "session_id":   session_id,
        "task_key":     task_key,
        "confidence":   confidence,
        "session_type": result.session_type,
        "reasoning":    result.reasoning,
        "method":       "mlx_direct",
        "dimensions":   result.dimensions,
        "elapsed_s":    elapsed,
    }


# ---------------------------------------------------------------------------
# Run log — one JSONL file per invocation written to ~/.meridian/logs/mlx/
# Each line is a full record: session inputs + raw model output + final result.
# ---------------------------------------------------------------------------

def _open_run_log(db_path: str) -> "tuple[Path, Any]":
    """Create the run log file and return (log_path, file_handle)."""
    import datetime

    ts = datetime.datetime.now().strftime("%Y%m%dT%H%M%S")
    log_dir = _SERVICES_DIR / "logs" / "mlx"
    log_dir.mkdir(parents=True, exist_ok=True)
    log_path = log_dir / f"run_{ts}.jsonl"
    return log_path, log_path.open("w", encoding="utf-8")


def _classify_one_logged(
    session_id: int,
    con: _sqlite3.Connection,
    run_log: Any,
) -> dict[str, Any]:
    """Classify one session and append a full record to the run log."""
    # Gather inputs before classification so we can log them even on error.
    session_raw = _fetch_session(con, session_id)
    pm_tasks = _fetch_pm_tasks(con) if session_raw else []
    recent = _fetch_recent_sessions(con, session_id) if session_raw else []

    if session_raw:
        user_message = build_user_message(
            {
                "id":                  session_id,
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
            },
            pm_tasks,
            recent_sessions=recent,
        )
    else:
        user_message = ""

    result = _classify_one(session_id, con)

    record = {
        "session_id":    session_id,
        "session_raw":   dict(session_raw) if session_raw else None,
        "pm_task_count": len(pm_tasks),
        "pm_tasks":      pm_tasks,
        "recent_count":  len(recent),
        "recent":        recent,
        "user_message":  user_message,
        "system_prompt": _SYSTEM_PROMPT,
        "result":        result,
    }
    run_log.write(json.dumps(record, default=str) + "\n")
    run_log.flush()
    return result


# ---------------------------------------------------------------------------
# Entry point
# ---------------------------------------------------------------------------

def main() -> None:
    try:
        payload = json.loads(sys.stdin.read())
    except (json.JSONDecodeError, ValueError) as exc:
        log.error("run_task_linker_mlx: malformed stdin JSON: %s", exc)
        sys.exit(1)

    session_ids: list[int] = payload.get("session_ids", [])
    db_path: str = payload.get("meridian_db", "")

    if not db_path:
        log.error("run_task_linker_mlx: meridian_db path is empty")
        sys.exit(1)

    if not Path(db_path).exists():
        log.error("run_task_linker_mlx: db file does not exist: %s", db_path)
        sys.exit(1)

    if not session_ids:
        log.info("run_task_linker_mlx: no session_ids provided, nothing to do")
        sys.stdout.write(json.dumps({"results": []}) + "\n")
        sys.stdout.flush()
        return

    log.info("run_task_linker_mlx: %d sessions, db=%s", len(session_ids), db_path)

    run_log_path, run_log_file = _open_run_log(db_path)
    log.info("run_task_linker_mlx: writing run log to %s", run_log_path)

    con = _sqlite3.connect(db_path)
    con.row_factory = _sqlite3.Row
    try:
        results: list[dict[str, Any]] = []
        for sid in session_ids:
            log.info("run_task_linker_mlx: classifying session %d", sid)
            result = _classify_one_logged(sid, con, run_log_file)
            results.append(result)
            log.info(
                "run_task_linker_mlx: session_id=%d task_key=%s "
                "session_type=%s elapsed_s=%.2f",
                result["session_id"],
                result["task_key"],
                result["session_type"],
                result["elapsed_s"],
            )
    finally:
        con.close()
        run_log_file.close()

    sys.stdout.write(json.dumps({"results": results}) + "\n")
    sys.stdout.flush()


if __name__ == "__main__":
    main()
