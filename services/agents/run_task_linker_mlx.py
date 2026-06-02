#!/usr/bin/env python3
"""run_task_linker_mlx — direct MLX in-process inference for session classification.

Reads a JSON payload from stdin:
    {"session_ids": [int, ...], "meridian_db": str, "traceparent": str | null}

Loads the MLX model in-process via outlines + mlx_lm (no HTTP server, no Hermes agent
loop, no tool calls), and uses FSM-constrained decoding to guarantee the response is
always a valid SessionClassification JSON object.

OTel span hierarchy (when invoked as a script via main()):
    run_task_linker_mlx          ← root span, parented to Rust traceparent
        classify_session         ← one per session_id
            db_fetch
            build_prompt
            llm_inference
            parse_response

When imported by server.py the same child spans are emitted under whatever
parent span the server establishes (propagated via contextvars).

Requires: outlines[mlxlm]>=1.3, mlx-lm>=0.22 (installed in .venv).
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

from opentelemetry.trace import StatusCode
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

# ── Whole-prompt context budget ────────────────────────────────────────────
# The full prompt (system + session evidence + candidate tickets + recent
# context) must fit the SELECTED model's context window minus the output
# reserve. _user_message_char_budget() derives a char budget for the user
# message from the loaded model's context length; build_user_message() then
# truncates session_text and the candidate list to fit. Conservative by design:
# a low chars-per-token estimate truncates slightly early rather than risking
# the hard ExceededContextWindow error (asymmetric failure — overflow is fatal,
# under-fill is a mild quality cost).
_CTX_WINDOW_FLOOR = 4096
_CTX_WINDOW_CEILING = 131072
_CTX_WINDOW_FALLBACK = 8192          # used only when config.json can't be read
_PROMPT_SAFETY_MARGIN_TOKENS = 512   # chat-template markers, role tokens, fudge
_CHARS_PER_TOKEN = 3.5               # dense code/OCR/JSON skews low — bias safe

# The eval-tuned default classifier model. It lives in the llm_selector catalog
# (_MODELS) as "qwen3.5-9b-optiq"; llm_selector keeps it on machines where it
# fits and degrades only when Metal headroom can't accommodate it. The catalog
# is the single source of truth for its working-set footprint — the constant
# below is only the fallback used if the lookup ever fails (~5-6 GB resident for
# the 9B-4bit weights, plus KV cache at our 1024-token generation budget).
_DEFAULT_MLX_MODEL_ID = "mlx-community/Qwen3.5-9B-OptiQ-4bit"
_DEFAULT_MLX_MODEL_MIN_RAM_GB = 6.5

# Explicit pin — set MLX_MODEL_ID to bypass dynamic selection entirely (eval
# experiments, reproducible benchmarks). When unset, the model is chosen at
# runtime by llm_selector.select_mlx_model_id() based on available compute.
_MLX_MODEL_ID_PIN = os.environ.get("MLX_MODEL_ID")

# Resolved lazily and cached for the process lifetime by _resolve_model_id().
# Kept as a module attribute (not just a function return) so /info, /v1/models,
# and the llm_inference span all report the same, truthful id.
_MLX_MODEL_ID: str | None = None


def _resolve_model_id() -> str:
    """Resolve the MLX model id for this process — once, then cached.

    Order: explicit MLX_MODEL_ID pin → dynamic selection via llm_selector →
    the hardcoded eval-tuned default on any failure. Never returns None so the
    in-process load always has a concrete id to hand mlx_lm.load.
    """
    global _MLX_MODEL_ID
    if _MLX_MODEL_ID is not None:
        return _MLX_MODEL_ID

    if _MLX_MODEL_ID_PIN:
        _MLX_MODEL_ID = _MLX_MODEL_ID_PIN
        log.info("run_task_linker_mlx: model pinned via MLX_MODEL_ID=%s", _MLX_MODEL_ID)
        return _MLX_MODEL_ID

    try:
        from agents.llm_selector import resolve_model, select_mlx_model_id
        entry = resolve_model(_DEFAULT_MLX_MODEL_ID)
        preferred_min_ram = (
            entry["min_ram_gb"] if entry else _DEFAULT_MLX_MODEL_MIN_RAM_GB
        )
        _MLX_MODEL_ID = select_mlx_model_id(
            preferred_hf_id=_DEFAULT_MLX_MODEL_ID,
            preferred_min_ram_gb=preferred_min_ram,
        ) or _DEFAULT_MLX_MODEL_ID
    except Exception as exc:  # noqa: BLE001
        log.warning(
            "run_task_linker_mlx: dynamic model selection failed (%s) — "
            "using default %s", exc, _DEFAULT_MLX_MODEL_ID,
        )
        _MLX_MODEL_ID = _DEFAULT_MLX_MODEL_ID

    log.info("run_task_linker_mlx: resolved MLX model=%s", _MLX_MODEL_ID)
    return _MLX_MODEL_ID


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
    category: Literal[
        "coding", "code_review", "meeting", "communication", "design",
        "documentation", "planning", "deployment_devops", "research",
        "idle_personal",
    ] = Field(
        ...,
        description=(
            "The single best activity category for this session. Derive it from "
            "the evidence (app, window titles, screen content); no category is "
            "supplied in the input. Declared early in the schema so FSM decoding "
            "always emits it before the long session_summary field."
        ),
    )
    category_confidence: float = Field(
        ..., ge=0.0, le=1.0,
        description="How certain you are about `category` (0.0-1.0).",
    )
    category_explanation: str = Field(
        ..., min_length=1, max_length=300,
        description=(
            "One concise sentence justifying the `category` choice, citing the "
            "app, window titles, or OCR evidence (e.g. 'VS Code editing "
            "run_watcher.py with a cargo build in the terminal'). Shown in the "
            "dashboard next to the category. Kept short so FSM decoding emits it "
            "before the long session_summary."
        ),
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
        ...,
        description=(
            "Concise justification citing window titles, OCR text, or context "
            "clues. No hard length cap — outlines must not truncate this field; "
            "the only ceiling is the server-side _MAX_TOKENS generation budget. "
            "Store whatever the model produces verbatim."
        ),
    )
    dimensions: dict[str, list[str]] = Field(
        default_factory=dict,
        description=(
            "Inferred activity tags. Keys: activity, intent, engagement, "
            "collaboration, tool, topic, practice. "
            "Values: lowercase snake_case lists. Omit keys with no evidence."
        ),
    )
    session_summary: str = Field(
        ..., min_length=100, max_length=1000,
        description=(
            "A factual prose summary of EVERYTHING the user did in this "
            "session, written for downstream project-management updates. "
            "Length is adaptive: aim for ~10 sentences for short trivial "
            "sessions and up to ~40-80 sentences for content-rich sessions; "
            "match depth to the evidence. Past tense, third person. "
            "PRESERVE every SDLC-relevant signal: "
            "(1) specific files, paths, modules touched; "
            "(2) commands, scripts, queries run + their outcome; "
            "(3) errors hit, stack traces, failing tests; "
            "(4) technical decisions made and the alternative considered; "
            "(5) tests written or run + pass/fail; "
            "(6) commits, branches, PRs opened/merged; "
            "(7) blockers and unanswered questions; "
            "(8) external research / docs / Stack Overflow / Claude advice consulted; "
            "(9) validations, manual QA, screenshots reviewed; "
            "(10) design choices, schema changes, migrations. "
            "DO NOT write marketing language, vague claims, or speculation "
            "about future work. Cite the evidence you saw in the session_text — "
            "this is the single source of truth the PM updater will compose from."
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
# Raw mlx_lm tokenizer per model — kept so _count_tokens() can measure the
# system prompt against the real vocabulary (the dominant, constant term in the
# context budget) instead of a char estimate.
_tokenizer_cache: dict[str, Any] = {}


def _get_model() -> Any:
    """Return an outlines-wrapped model, loading from disk on the first call."""
    model_id = _resolve_model_id()
    if model_id in _model_cache:
        return _model_cache[model_id]

    try:
        import mlx_lm
        import outlines
    except ImportError as exc:
        raise ImportError(
            f"Required package not installed: {exc}. "
            "Install with: pip install 'mlx-lm>=0.22' 'outlines[mlxlm]>=1.3'"
        ) from exc

    log.info(
        "run_task_linker_mlx: loading %s (first call this process)", model_id
    )
    t0 = time.time()
    mlx_model, tokenizer = mlx_lm.load(
        model_id,
        tokenizer_config={"trust_remote_code": True},
    )
    outlines_model = outlines.from_mlxlm(mlx_model, tokenizer)
    log.info("run_task_linker_mlx: model loaded in %.1fs", time.time() - t0)

    _model_cache[model_id] = outlines_model
    _tokenizer_cache[model_id] = tokenizer
    return outlines_model


# ---------------------------------------------------------------------------
# Context budget — derive a per-model char budget for the user message so the
# whole prompt fits the selected model's context window (see build_user_message
# in _prompts.py). Resolved lazily and cached for the process lifetime.
# ---------------------------------------------------------------------------

_ctx_window_tokens: int | None = None
_user_msg_char_budget: int | None = None
_user_msg_budget_resolved = False


def _hf_snapshot_dir(hf_id: str) -> "Path | None":
    """Locate the on-disk HF snapshot dir holding a model's config.json."""
    hf_home = os.environ.get("HF_HOME")
    cache = (
        os.environ.get("HF_HUB_CACHE")
        or (os.path.join(hf_home, "hub") if hf_home else None)
        or str(Path.home() / ".cache" / "huggingface" / "hub")
    )
    snaps = Path(cache) / ("models--" + hf_id.replace("/", "--")) / "snapshots"
    if not snaps.is_dir():
        return None
    for rev in sorted(snaps.iterdir()):
        if rev.is_dir() and (rev / "config.json").exists():
            return rev
    return None


def _context_window_tokens() -> int:
    """Read the selected model's context length from its config.json.

    Reads max_position_embeddings (the authoritative context length) from the
    model's config; falls back to a conservative default and clamps to sane
    bounds. Read from disk rather than a hand-maintained map so it can't drift.
    """
    global _ctx_window_tokens
    if _ctx_window_tokens is not None:
        return _ctx_window_tokens
    ctx: Any = None
    try:
        model_id = _resolve_model_id()
        snap = _hf_snapshot_dir(model_id) if "/" in model_id else None
        cfg_path = (snap / "config.json") if snap else (Path(model_id) / "config.json")
        if cfg_path.exists():
            cfg = json.loads(cfg_path.read_text(encoding="utf-8"))
            ctx = cfg.get("max_position_embeddings")
            if ctx is None and isinstance(cfg.get("text_config"), dict):
                ctx = cfg["text_config"].get("max_position_embeddings")
    except Exception as exc:  # noqa: BLE001
        log.debug("run_task_linker_mlx: context-window probe failed: %s", exc)
    if not isinstance(ctx, int) or ctx <= 0:
        log.info(
            "run_task_linker_mlx: context window unknown — using fallback %d",
            _CTX_WINDOW_FALLBACK,
        )
        ctx = _CTX_WINDOW_FALLBACK
    ctx = max(_CTX_WINDOW_FLOOR, min(_CTX_WINDOW_CEILING, ctx))
    _ctx_window_tokens = ctx
    log.info("run_task_linker_mlx: model context window = %d tokens", ctx)
    return ctx


def _count_tokens(text: str) -> int:
    """Token count via the loaded tokenizer; conservative char estimate if absent.

    The tokenizer is only cached after _get_model() runs. On the server it is
    preloaded at startup, so the system-prompt measurement is exact; on the CLI
    first call it falls back to chars/_CHARS_PER_TOKEN (an over-estimate, which
    is the safe direction for a budget).
    """
    tok = _tokenizer_cache.get(_resolve_model_id())
    if tok is not None:
        try:
            return len(tok.encode(text))
        except Exception:  # noqa: BLE001
            pass
    return int(len(text) / _CHARS_PER_TOKEN) + 1


def _user_message_char_budget() -> "int | None":
    """Char budget for the classifier user message, or None to use the static cap.

    Precedence (mirrors _resolve_model_id): an explicit SESSION_TEXT_CAP env
    (including 0) pins the legacy static behaviour and disables this dynamic
    budget — eval reproducibility and caller-owns-fit. Otherwise derive it from
    the model's context window minus the system prompt and output reserve.
    """
    global _user_msg_char_budget, _user_msg_budget_resolved
    if _user_msg_budget_resolved:
        return _user_msg_char_budget
    _user_msg_budget_resolved = True

    if os.environ.get("SESSION_TEXT_CAP") is not None:
        log.info("run_task_linker_mlx: SESSION_TEXT_CAP pinned — dynamic prompt budget off")
        _user_msg_char_budget = None
        return None

    try:
        ctx = _context_window_tokens()
        system_tokens = _count_tokens(_SYSTEM_PROMPT)
        user_tokens = ctx - _MAX_TOKENS - system_tokens - _PROMPT_SAFETY_MARGIN_TOKENS
        _user_msg_char_budget = max(0, int(user_tokens * _CHARS_PER_TOKEN))
        log.info(
            "run_task_linker_mlx: user-message char budget=%d "
            "(ctx=%d system_tok=%d output=%d margin=%d)",
            _user_msg_char_budget, ctx, system_tokens, _MAX_TOKENS,
            _PROMPT_SAFETY_MARGIN_TOKENS,
        )
    except Exception as exc:  # noqa: BLE001
        log.warning(
            "run_task_linker_mlx: char-budget calc failed (%s) — static cap", exc
        )
        _user_msg_char_budget = None
    return _user_msg_char_budget


# ---------------------------------------------------------------------------
# DB helpers
# ---------------------------------------------------------------------------

def _fetch_session(
    con: _sqlite3.Connection, session_id: int
) -> dict[str, Any] | None:
    row = con.execute(
        "SELECT id, app_name, started_at, ended_at, duration_s, session_text,"
        "       session_text_source, window_titles, category, confidence,"
        "       session_summary, claude_session_uuid"
        " FROM app_sessions WHERE id = ?",
        (session_id,),
    ).fetchone()
    return dict(row) if row else None


def _fetch_recent_sessions(
    con: _sqlite3.Connection, before_id: int
) -> list[dict[str, Any]]:
    # Recent context is a task-continuity signal only: app, time, duration and
    # which ticket each recent session mapped to. We deliberately do NOT select
    # session_text/excerpt or category — recent OCR is noise here and a category
    # tag would feed a prior back into classification. (session_text is still
    # referenced in WHERE only to skip empty-capture rows.)
    rows = con.execute(
        "SELECT app_name, started_at, duration_s, task_key, task_routing"
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
        "session_id":           session_id,
        "task_key":             None,
        "confidence":           0.0,
        "category":             "idle_personal",
        "category_confidence":  0.0,
        "category_explanation": "",
        "session_type":         "overhead",
        "reasoning":            reason,
        "method":              method,
        "dimensions":          {},
        "session_summary":     "",
        "category_explanation": "",
        "elapsed_s":           elapsed,
    }


def _classify_one(
    session_id: int,
    con: _sqlite3.Connection,
) -> dict[str, Any]:
    # ── db_fetch ──────────────────────────────────────────────────────────────
    with tracer.start_as_current_span("db_fetch") as db_span:
        session_raw = _fetch_session(con, session_id)
        if session_raw is None:
            db_span.set_attribute("pm_tasks_count", 0)
            db_span.set_attribute("recent_sessions_count", 0)
            db_span.add_event("session_not_found", {"session_id": session_id})
            db_span.set_status(StatusCode.ERROR, f"session {session_id} not found in DB")
            return _error_result(
                session_id, f"session {session_id} not found in DB", 0.0, "mlx_error"
            )

        pm_tasks = _fetch_pm_tasks(con)
        recent   = _fetch_recent_sessions(con, session_id)

        db_span.set_attribute("pm_tasks_count", len(pm_tasks))
        db_span.set_attribute("recent_sessions_count", len(recent))

        session_text = session_raw.get("session_text") or ""
        # Coding-agent rows (Claude Code / Codex) carry the full transcript in
        # session_text and a concise, high-quality prose summary in
        # session_summary. Classify on the summary, not the multi-MB transcript:
        # cheaper, faster, and it's already the distilled "what was done".
        if session_raw.get("claude_session_uuid") and (session_raw.get("session_summary") or "").strip():
            session_text = session_raw["session_summary"]
        db_span.add_event("session_loaded", {
            "app_name":           str(session_raw.get("app_name") or ""),
            "duration_s":         float(session_raw.get("duration_s") or 0.0),
            "category":           str(session_raw.get("category") or ""),
            "session_text_chars": len(session_text),
            "text_source":        str(session_raw.get("session_text_source") or ""),
        })
        recent_task_keys = [r.get("task_key") for r in recent if r.get("task_key")]
        db_span.add_event("context_loaded", {
            "pm_tasks_count":        len(pm_tasks),
            "recent_sessions_count": len(recent),
            "recent_task_keys":      ", ".join(recent_task_keys) if recent_task_keys else "-",
        })

    session = {
        "id":                  session_id,
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
    with tracer.start_as_current_span("build_prompt") as bp_span:
        bp_span.set_attribute("pm_tasks_count", len(pm_tasks))
        bp_span.set_attribute("recent_sessions_count", len(recent))
        char_budget = _user_message_char_budget()
        bp_span.set_attribute("char_budget", char_budget if char_budget is not None else -1)
        user_message = build_user_message(
            session, pm_tasks, recent_sessions=recent, char_budget=char_budget,
        )
        bp_span.set_attribute("prompt_chars", len(user_message))
        recent_with_task = sum(1 for r in recent if r.get("task_key"))
        bp_span.add_event("prompt_assembled", {
            "pm_tasks_included":        len(pm_tasks),
            "recent_sessions_included": len(recent),
            "recent_with_task_key":     recent_with_task,
            "session_text_chars":       len(session_text),
            "prompt_chars":             len(user_message),
            "prompt_text":              user_message[:3000],
        })

    messages = [
        {"role": "system", "content": _SYSTEM_PROMPT},
        {"role": "user",   "content": user_message},
    ]

    # ── llm_inference ─────────────────────────────────────────────────────────
    t0 = time.time()
    with tracer.start_as_current_span("llm_inference") as llm_span:
        llm_span.set_attribute("model", _resolve_model_id())
        llm_span.set_attribute("max_tokens", _MAX_TOKENS)
        llm_span.set_attribute("temperature", _TEMPERATURE)
        llm_span.add_event("inference_started", {"session_id": session_id})
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
            llm_span.set_attribute("outcome", "mlx_error")
            llm_span.set_attribute("elapsed_s", elapsed)
            llm_span.set_status(StatusCode.ERROR, str(exc))
            llm_span.add_event("inference_error", {
                "error_type":    type(exc).__name__,
                "error_message": str(exc)[:500],
                "elapsed_s":     elapsed,
            })
            log.warning(
                "run_task_linker_mlx: inference failed for session %d: %s",
                session_id, exc,
            )
            return _error_result(
                session_id, f"mlx inference error: {exc}", elapsed, "mlx_error"
            )

        elapsed = time.time() - t0
        llm_span.set_attribute("outcome", "mlx_direct")
        llm_span.set_attribute("elapsed_s", elapsed)
        llm_span.set_attribute("response_chars", len(raw))
        llm_span.add_event("inference_complete", {
            "elapsed_s":      elapsed,
            "response_chars": len(raw),
        })

    log.debug(
        "run_task_linker_mlx: session %d raw (%.1fs): %.200s",
        session_id, elapsed, raw,
    )

    # ── parse_response ────────────────────────────────────────────────────────
    with tracer.start_as_current_span("parse_response") as pr_span:
        pr_span.set_attribute("raw_chars", len(raw))
        pr_span.add_event("raw_mlx_output", {
            "chars":   len(raw),
            "preview": raw[:500],
        })

        # outlines guarantees schema validity; model_validate_json rarely fails.
        try:
            result = SessionClassification.model_validate_json(raw)
        except Exception as exc:
            pr_span.set_attribute("outcome", "schema_error")
            pr_span.set_status(StatusCode.ERROR, str(exc))
            pr_span.add_event("parse_failure", {
                "error":       str(exc)[:300],
                "raw_preview": raw[:300],
            })
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
            pr_span.set_attribute("outcome", "invalid_task_key")
            pr_span.set_status(StatusCode.ERROR, f"unknown task_key {task_key!r}")
            pr_span.add_event("parse_failure", {
                "error":       f"model returned unknown task_key {task_key!r}",
                "raw_preview": raw[:300],
            })
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
        pr_span.set_attribute("outcome", "ok")
        pr_span.set_attribute("task_key", task_key if task_key is not None else "-")
        pr_span.set_attribute("confidence", confidence)
        pr_span.add_event("parse_success", {
            "task_key":         task_key if task_key is not None else "-",
            "confidence":       confidence,
            "session_type":     result.session_type,
            "dimensions_count": len(result.dimensions),
            "dimension_keys":   ", ".join(sorted(result.dimensions.keys())),
        })

    return {
        "session_id":           session_id,
        "task_key":             task_key,
        "confidence":           confidence,
        "category":             result.category,
        "category_confidence":  max(0.0, min(1.0, result.category_confidence)),
        "category_explanation": result.category_explanation,
        "session_type":         result.session_type,
        "reasoning":            result.reasoning,
        "method":              "mlx_direct",
        "dimensions":          result.dimensions,
        "session_summary":     result.session_summary,
        "elapsed_s":           elapsed,
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
            char_budget=_user_message_char_budget(),
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
    traceparent: str | None = payload.get("traceparent")

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

    ctx = observability.extract_parent_context(traceparent)
    with tracer.start_as_current_span("run_task_linker_mlx", context=ctx) as root_span:
        root_span.set_attribute("session_count", len(session_ids))
        root_span.set_attribute("db_path", Path(db_path).name)

        run_log_path, run_log_file = _open_run_log(db_path)
        log.info("run_task_linker_mlx: writing run log to %s", run_log_path)

        con = _sqlite3.connect(db_path)
        con.row_factory = _sqlite3.Row
        try:
            results: list[dict[str, Any]] = []
            for sid in session_ids:
                with tracer.start_as_current_span("classify_session") as cls_span:
                    cls_span.set_attribute("session_id", sid)
                    log.info("run_task_linker_mlx: classifying session %d", sid)
                    result = _classify_one_logged(sid, con, run_log_file)
                    results.append(result)
                    cls_span.set_attribute("task_key", result["task_key"] or "-")
                    cls_span.set_attribute("session_type", result["session_type"])
                    cls_span.set_attribute("method", result["method"])
                    cls_span.set_attribute("elapsed_s", result["elapsed_s"])
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

        root_span.set_attribute("results_count", len(results))
        sys.stdout.write(json.dumps({"results": results}) + "\n")
        sys.stdout.flush()

    observability.shutdown()


if __name__ == "__main__":
    main()
