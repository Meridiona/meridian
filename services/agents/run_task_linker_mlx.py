#!/usr/bin/env python3
"""run_task_linker_mlx — direct MLX in-process inference for session classification.

Reads a JSON payload from stdin:
    {"session_ids": [int, ...], "meridian_db": str, "traceparent": str | null}

Loads the MLX model in-process via outlines + mlx_lm , and uses FSM-constrained decoding to guarantee the response is
always a valid SessionClassification JSON object.

OTel span hierarchy (when invoked as a script via main()):
    run_task_linker_mlx          ← root span, parented to Rust traceparent
        classify_session         ← one per session_id
            db_fetch
            classifier_input     ← the COMPLETE model input (system + user)
                system_prompt        — classifier skill + context
                recent_context       — past-5 sessions
                session_block        — the input session being classified
                candidate_tickets    — ranked candidate tickets (★ = today)
            llm_inference
            classifier_output    ← the COMPLETE raw model output
                reasoning            — why this task match
                category             — category justification (+ category/confidence)
                dimensions           — inferred activity tags
                session_summary      — PM-facing prose deliverable

When imported by server.py the same child spans are emitted under whatever
parent span the server establishes (propagated via contextvars).

Requires: outlines[mlxlm]>=1.3, mlx-lm>=0.22 (installed in .venv).
Method tag in results: "mlx_direct".
"""
from __future__ import annotations

import datetime as _dt
import gc
import json
import logging
import os
import sqlite3 as _sqlite3
import sys
import threading
import time
from contextlib import contextmanager
from pathlib import Path
from typing import Any, Literal, Optional, Iterator

from opentelemetry import trace
from opentelemetry.trace import StatusCode
from pydantic import BaseModel, Field

_SERVICES_DIR = Path(__file__).parent.parent

from agents import observability
from agents._prompts import (
    build_user_message,
    _format_candidates,
    _format_recent_sessions,
    _format_session,
)
from agents._system_context import SYSTEM_CONTEXT

log = logging.getLogger("agents.run_task_linker_mlx")
tracer = observability.setup("meridian-task-linker-mlx")

_CONTEXT_WINDOW = 5
_MAX_TOKENS = 1024
_TEMPERATURE = 0.0  # greedy decoding — deterministic classification

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
        from agents.llm_selector import (
            APPLE_INTELLIGENCE_ID, resolve_model, select_mlx_model_id,
        )
        entry = resolve_model(_DEFAULT_MLX_MODEL_ID)
        preferred_min_ram = (
            entry["min_ram_gb"] if entry else _DEFAULT_MLX_MODEL_MIN_RAM_GB
        )
        selected = select_mlx_model_id(
            preferred_hf_id=_DEFAULT_MLX_MODEL_ID,
            preferred_min_ram_gb=preferred_min_ram,
        )
        # Propagate the Apple Intelligence sentinel as-is; fall back to the
        # default MLX model only when nothing at all was selected (None).
        _MLX_MODEL_ID = selected if selected is not None else _DEFAULT_MLX_MODEL_ID
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
        ..., min_length=100,
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
# Model loading — loaded lazily on first use, evicted when idle.
#
# The MLX model holds ~7 GB of Metal unified memory while resident (measured;
# note `ps`/Activity Monitor RSS does NOT show it). Classification is bursty,
# so we keep the model only while it's being used: load on first inference,
# and evict after MLX_IDLE_EVICT_S of inactivity (server.py runs the evictor).
# `del + gc.collect() + mx.clear_cache()` reclaims the full 7 GB; cold reload
# is ~3 s. `_model_lock` + `_in_flight` guarantee the evictor never frees the
# model out from under an in-flight inference.
# ---------------------------------------------------------------------------

_model_cache: dict[str, Any] = {}
# Tokenizer kept alongside the model so we can render the EXACT chat-template
# prompt (the wire format outlines feeds the model) for the classifier_input span.
_tokenizer_cache: dict[str, Any] = {}
_model_lock = threading.Lock()       # guards _model_cache mutation, _in_flight, _last_used, eviction
_in_flight = 0                       # inferences currently using the model
_last_used = time.monotonic()        # monotonic ts of the last finished inference

# Aggressive default (2 min): the model is present only during active bursts.
# Tune via env without a code change; 0 disables idle eviction entirely.
_IDLE_EVICT_S = float(os.environ.get("MLX_IDLE_EVICT_S", "120"))


def _get_model() -> Any:
    """Return an outlines-wrapped model, loading from disk on the first call.

    Cache-miss load is done under _model_lock (double-checked) so concurrent
    callers can't double-load and the idle evictor can't race the load.
    """
    model_id = _resolve_model_id()
    cached = _model_cache.get(model_id)
    if cached is not None:
        return cached

    with _model_lock:
        cached = _model_cache.get(model_id)   # re-check under lock
        if cached is not None:
            return cached
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


@contextmanager
def model_session() -> Iterator[Any]:
    """Yield the loaded model, marking it in-flight so the idle evictor never
    frees it mid-inference. Wrap every direct ``model(...)`` call in this.

    Lock is held only briefly (to bump/clear the in-flight counter), never for
    the duration of inference. NOTE: production serialises all MLX calls upstream
    via the Rust llm_gate (1-permit semaphore), so inferences don't actually
    overlap — this lock scope just avoids adding a second, redundant serialisation
    point, NOT a claim that concurrent generation on the shared model is safe.
    """
    global _in_flight, _last_used
    with _model_lock:
        _in_flight += 1
    try:
        yield _get_model()
    finally:
        with _model_lock:
            _in_flight -= 1
            _last_used = time.monotonic()


def maybe_evict_idle(idle_s: float | None = None) -> float | None:
    """Evict the model if it's resident, nothing is in flight, and it's been
    idle longer than ``idle_s`` (default MLX_IDLE_EVICT_S). Returns the GB freed,
    or None if no eviction happened. Safe to call from a threadpool worker.

    Uses a non-blocking lock acquire: if an inference/load is mutating state we
    simply skip this tick and try again on the next one.
    """
    ttl = _IDLE_EVICT_S if idle_s is None else idle_s
    if ttl <= 0:
        return None
    if not _model_lock.acquire(blocking=False):
        return None
    try:
        if _in_flight > 0 or not _model_cache:
            return None
        if (time.monotonic() - _last_used) < ttl:
            return None
        try:
            import mlx.core as mx
            before = mx.get_active_memory()
        except Exception:               # noqa: BLE001 — mx should always import here
            mx, before = None, 0
        _model_cache.clear()
        gc.collect()
        freed = 0.0
        if mx is not None:
            mx.clear_cache()
            freed = max(0.0, (before - mx.get_active_memory()) / 1e9)
        log.info(
            "run_task_linker_mlx: evicted idle model (idle ≥ %.0fs), freed ~%.1f GB",
            ttl, freed,
        )
        return freed
    finally:
        _model_lock.release()


def model_resident() -> bool:
    """True if the MLX model is currently loaded in memory."""
    return bool(_model_cache)


def model_active_memory_gb() -> float | None:
    """Live Metal active-memory footprint in GB, or None if MLX is unavailable.

    Process-wide Metal active memory (≈ the model when resident — the model
    dominates, though a transient load allocation can briefly inflate it), and
    the only honest measure: `ps`/Activity Monitor can't see Metal unified
    memory (they undercount by ~6.5 GB).
    """
    try:
        import mlx.core as mx
        return round(mx.get_active_memory() / 1e9, 2)
    except Exception:  # noqa: BLE001 — mx absent on non-MLX machines
        return None


# Apple Foundation Models has a 4096-token combined context window (input + output).
# The full _SYSTEM_PROMPT is ~19k chars / ~4800 tokens — it does NOT fit. Use a
# compact prompt instead: ~500 tokens for instructions, ~2000 for user, ~500 for output.
_APPLE_FM_USER_CHARS = 8_000   # ~2000 tokens — user message cap

# Compact classifier prompt sized for Apple FM's 4096-token window.
# Covers the essential decision logic; the full SKILL.md is used for larger models.
# Schema matches SessionClassification exactly — wrong types cause Pydantic rejection.
_APPLE_FM_SYSTEM_PROMPT = """\
You are Meridian's session classifier. Return ONLY a JSON object — no markdown, no extra text.

Required schema (all fields mandatory):
{"task_key": <string or null>, "confidence": <float 0.0-1.0>, "category": <see below>, "category_confidence": <float 0.0-1.0>, "category_explanation": "<one sentence max 300 chars>", "session_type": <see below>, "reasoning": "<concise justification>", "dimensions": {"activity": ["<tag>"], "tool": ["<tag>"]}, "session_summary": "<100-500 char factual past-tense prose>"}

category must be exactly one of: coding, code_review, meeting, communication, design, documentation, planning, deployment_devops, research, idle_personal

session_type must be exactly one of: task, overhead, untracked

Rules:
- task_key: ONLY copy a key from the supplied candidate list verbatim. null if no list or no clear match. NEVER invent a key.
- session_type "task": session matches a candidate ticket. session_type "overhead": idle/personal/music/idle_personal → confidence ≥ 0.9. session_type "untracked": real work, no ticket match → confidence 0.65-0.75.
- confidence: 0.95=certain, 0.80=probable, 0.65=likely, 0.50=uncertain
- dimensions values must be lists of lowercase snake_case strings
- session_summary must be factual past tense, cite specific files/tools/actions, minimum 2 sentences"""


_VALID_CATEGORIES = frozenset({
    "coding", "code_review", "meeting", "communication", "design",
    "documentation", "planning", "deployment_devops", "research", "idle_personal",
})


def _coerce_apple_fm_result(data: dict) -> dict:
    """Fill missing or malformed fields so Pydantic can validate Apple FM output.

    Apple FM doesn't guarantee all required fields. This function synthesizes
    missing ones from what was returned rather than failing.
    """
    # session_type coercion
    st = str(data.get("session_type", "untracked"))
    if st not in ("task", "overhead", "untracked"):
        st = "overhead" if st in ("idle", "personal") else "untracked"
    data["session_type"] = st

    # category coercion
    cat = str(data.get("category", ""))
    if cat not in _VALID_CATEGORIES:
        cat = "idle_personal" if st == "overhead" else "coding"
    data["category"] = cat

    # confidence: clamp to [0, 1]
    try:
        data["confidence"] = max(0.0, min(1.0, float(data.get("confidence", 0.7))))
    except (TypeError, ValueError):
        data["confidence"] = 0.7

    # category_confidence: derive from confidence if missing
    if "category_confidence" not in data or not isinstance(data["category_confidence"], (int, float)):
        data["category_confidence"] = round(data["confidence"] * 0.9, 2)
    else:
        data["category_confidence"] = max(0.0, min(1.0, float(data["category_confidence"])))

    # category_explanation: fall back to first sentence of reasoning
    if not data.get("category_explanation"):
        reasoning = str(data.get("reasoning", "No details recorded."))
        data["category_explanation"] = reasoning[:300]

    # reasoning: ensure it's a non-empty string
    if not data.get("reasoning"):
        data["reasoning"] = "Classified via Apple Foundation Models."

    # session_summary: must be at least 100 chars
    summary = str(data.get("session_summary", ""))
    if len(summary) < 100:
        # Pad from reasoning
        reasoning = str(data.get("reasoning", ""))
        summary = (summary + " " + reasoning).strip()
    if len(summary) < 100:
        summary = summary + " The session was processed by Apple Foundation Models."
    data["session_summary"] = summary

    # dimensions: must be dict[str, list[str]]
    dims = data.get("dimensions", {})
    if not isinstance(dims, dict):
        dims = {}
    data["dimensions"] = {
        k: ([str(i) for i in v] if isinstance(v, list) else [str(v)])
        for k, v in dims.items()
    }

    # task_key: null if session_type is not "task"
    if st != "task":
        data["task_key"] = None
    elif data.get("task_key") is not None:
        data["task_key"] = str(data["task_key"])

    return data


def _classify_apple_fm(messages: list[dict[str, str]]) -> "SessionClassification":
    """Classify via Apple Foundation Models (non-FSM, JSON parsing with coercion).

    Uses a compact system prompt sized for Apple FM's 4096-token context window.
    The full _SYSTEM_PROMPT (~4800 tokens) does not fit; _APPLE_FM_SYSTEM_PROMPT
    covers the essential decision logic in ~500 tokens.

    Apple FM may omit fields. _coerce_apple_fm_result fills missing required
    fields with sensible defaults before Pydantic validation.
    """
    import asyncio

    from apple_fm_sdk import LanguageModelSession  # type: ignore[import]

    # Always use the compact prompt — ignore whatever system message the caller sent.
    system = _APPLE_FM_SYSTEM_PROMPT
    user   = next((m["content"] for m in messages if m["role"] == "user"),   "")

    # Truncate to stay within the 4096-token context window.
    if len(user) > _APPLE_FM_USER_CHARS:
        log.debug(
            "run_task_linker_mlx: truncating Apple FM user message %d → %d chars",
            len(user), _APPLE_FM_USER_CHARS,
        )
        user = user[:_APPLE_FM_USER_CHARS]

    user_with_hint = (
        user
        + "\n\nRespond with a JSON object matching the schema above. "
        "Output only valid JSON — no markdown fences, no extra text."
    )

    async def _run(prompt: str) -> str:
        session = LanguageModelSession(instructions=system)
        r = await session.respond(prompt)
        return getattr(r, "content", r)

    def _parse(text: str) -> "SessionClassification":
        text = text.strip()
        if text.startswith("```"):
            text = text.split("\n", 1)[1].rsplit("```", 1)[0].strip()
        data = json.loads(text)
        return SessionClassification.model_validate(_coerce_apple_fm_result(data))

    def _call_apple_fm(prompt: str) -> str:
        # anyio (used by FastAPI's run_in_threadpool) sets up its own event loop
        # machinery in its worker threads. asyncio.run() raises
        # "cannot be called from a running event loop" even inside a threadpool
        # thread. Spawning a genuinely fresh OS thread with its own event loop
        # avoids anyio's loop entirely.
        import concurrent.futures
        def _in_fresh_thread() -> str:
            loop = asyncio.new_event_loop()
            asyncio.set_event_loop(loop)
            try:
                return loop.run_until_complete(_run(prompt))
            finally:
                loop.close()
                asyncio.set_event_loop(None)
        with concurrent.futures.ThreadPoolExecutor(max_workers=1) as ex:
            return ex.submit(_in_fresh_thread).result(timeout=60)

    raw = _call_apple_fm(user_with_hint)
    try:
        return _parse(raw)
    except Exception:
        # One retry: ask the model to complete any missing fields.
        fix_prompt = (
            "Your previous JSON was incomplete — it was missing required fields "
            "(category, category_confidence, category_explanation, session_summary). "
            "Return a complete JSON with ALL fields from the schema:\n" + raw
        )
        raw2 = _call_apple_fm(fix_prompt)
        return _parse(raw2)


# ---------------------------------------------------------------------------
# DB helpers
# ---------------------------------------------------------------------------

def _fetch_session(
    con: _sqlite3.Connection, session_id: int
) -> dict[str, Any] | None:
    row = con.execute(
        "SELECT id, app_name, started_at, ended_at, duration_s, session_text,"
        "       session_text_source, window_titles, category, confidence,"
        "       session_summary, claude_session_uuid,"
        "       min_frame_id, max_frame_id, frame_count, frame_contributions"
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


def _local_day(started_at: str) -> str:
    """The local calendar day (YYYY-MM-DD) of a session's UTC `started_at`.

    `daily_plan.plan_date` is the dev's *local* day (the dashboard stamps it from
    the browser's local date), but `app_sessions.started_at` is stored UTC. We
    convert UTC → local here so a session is matched to the plan the dev actually
    declared for that day. Returns "" on an unparseable timestamp (→ no boost).
    """
    if not started_at:
        return ""
    try:
        # `astimezone()` with no arg converts an aware datetime to the host's
        # local zone — the same zone the dashboard used to compute plan_date.
        return _dt.datetime.fromisoformat(started_at).astimezone().date().isoformat()
    except ValueError:
        return ""


def _fetch_plan_focus(con: _sqlite3.Connection, plan_date: str) -> list[str]:
    """Ordered task_keys the dev CONFIRMED as their focus for `plan_date`.

    Empty (→ no boost, classification proceeds exactly as before) when the day is
    unconfirmed, explicitly skipped, has no plan rows, or the plan tables don't
    exist yet (pre-migration-041 DB). This is a ranking signal only — never a
    filter — so an empty result can only ever cost the boost, never recall.
    """
    if not plan_date:
        return []
    try:
        meta = con.execute(
            "SELECT confirmed_at, skipped FROM daily_plan_meta WHERE plan_date = ?",
            (plan_date,),
        ).fetchone()
        if meta is None or meta["skipped"] or not meta["confirmed_at"]:
            return []
        rows = con.execute(
            "SELECT task_key FROM daily_plan WHERE plan_date = ? ORDER BY position",
            (plan_date,),
        ).fetchall()
        return [r["task_key"] for r in rows]
    except _sqlite3.OperationalError:
        return []


def _fetch_pm_tasks(
    con: _sqlite3.Connection, focus_keys: list[str] | None = None
) -> list[dict[str, Any]]:
    # Candidate set for classification. Tickets the user explicitly EXCLUDED during
    # onboarding board-cleanup (pm_task_curation.decision = 'excluded') are dropped
    # so a cleaned-up dead ticket can never be a classification target. Everything
    # else flows through, including not-yet-decided `looks_stale` rows — the triage
    # only *proposes*; nothing is removed without the human's confirmed decision.
    # LEFT JOIN keeps this safe if curation has no row for a ticket yet.
    base_cols = (
        "SELECT t.task_key, t.title,"
        "       COALESCE(t.description_text,'') AS description_text,"
        "       COALESCE(t.status_raw,'') AS status_raw,"
        "       COALESCE(t.is_terminal,0) AS is_terminal,"
        "       COALESCE(t.issue_type,'') AS issue_type,"
        "       COALESCE(t.parent_key,'') AS parent_key,"
        "       COALESCE(t.epic_title,'') AS epic_title,"
        "       COALESCE(t.sprint_name,'') AS sprint_name,"
        "       COALESCE(t.tags,'') AS tags"
        " FROM pm_tasks t"
    )
    try:
        rows = con.execute(
            base_cols
            + " LEFT JOIN pm_task_curation c ON c.task_key = t.task_key"
            " WHERE c.decision IS NULL OR c.decision != 'excluded'",
        ).fetchall()
    except _sqlite3.OperationalError:
        # Pre-migration-038 DB (no pm_task_curation): degrade to the unfiltered
        # candidate set rather than crashing the whole /classify_sessions call.
        rows = con.execute(base_cols).fetchall()
    tasks = [dict(r) for r in rows]

    # Today's-focus boost: tag the tickets the dev declared for the day and float
    # them to the top of the candidate list, in their declared order. This is a
    # BOOST, never a filter — every other candidate still follows, so recall is
    # untouched. A focus key that isn't in `tasks` (e.g. excluded by curation)
    # simply has no effect; we never resurrect a filtered-out ticket.
    focus = focus_keys or []
    if focus:
        order = {key: i for i, key in enumerate(focus)}
        for t in tasks:
            t["is_today_focus"] = t["task_key"] in order
        tasks.sort(key=lambda t: (0, order[t["task_key"]]) if t.get("is_today_focus") else (1, 0))
    return tasks


def _get_tokenizer() -> Any:
    """The active model's tokenizer: the resident one if loaded, else a
    weights-free transformers load from the local HF cache (cached). The model's
    chat template matches what outlines/mlx_lm feed the model, so rendering and
    token counts derived here are exact. Raises if no tokenizer is available.
    """
    model_id = _resolve_model_id()
    tok = _tokenizer_cache.get(model_id)
    if tok is None:
        from transformers import AutoTokenizer
        tok = AutoTokenizer.from_pretrained(model_id, trust_remote_code=True)
        _tokenizer_cache[model_id] = tok
    return tok


def _render_model_prompt(messages: list[dict[str, str]]) -> str | None:
    """EXACT text fed to the model: messages through the tokenizer's chat template
    (role markers + special tokens), matching what outlines feeds it. None on
    failure / Apple FM path so the caller can fall back to a plain rendering."""
    try:
        return _get_tokenizer().apply_chat_template(
            messages, tokenize=False, add_generation_prompt=True
        )
    except Exception as exc:  # noqa: BLE001 — best-effort observability
        log.debug("run_task_linker_mlx: chat-template render failed: %s", exc)
        return None


def _count_prompt_tokens(text: str) -> int | None:
    """Exact prompt token count — deterministic tokenization, so it equals MLX's
    reported prompt_tokens (output tokens, by contrast, come from MLX's stats, not
    a re-encode). None if no tokenizer is available (Apple FM path)."""
    try:
        return len(_get_tokenizer().encode(text))
    except Exception as exc:  # noqa: BLE001 — best-effort observability
        log.debug("run_task_linker_mlx: prompt token count failed: %s", exc)
        return None


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

        plan_date  = _local_day(session_raw.get("started_at") or "")
        focus_keys = _fetch_plan_focus(con, plan_date)
        pm_tasks   = _fetch_pm_tasks(con, focus_keys)
        recent     = _fetch_recent_sessions(con, session_id)

        session_text = session_raw.get("session_text") or ""
        # Coding-agent rows (Claude Code / Codex) carry the full transcript in
        # session_text and a concise, high-quality prose summary in
        # session_summary. Classify on the summary, not the multi-MB transcript:
        # cheaper, faster, and it's already the distilled "what was done".
        if session_raw.get("claude_session_uuid") and (session_raw.get("session_summary") or "").strip():
            session_text = session_raw["session_summary"]

        # db_fetch is the SOLE source of "what the model was given" — recorded
        # once, as attributes (no duplicate events repeating these numbers on
        # build_prompt). `candidate_task_keys` is the highest-value debug signal:
        # it answers "was the right ticket even offered, and where was it ranked?"
        # without anyone having to read the prompt. Ranked order is preserved
        # (today's-focus keys float to the front in _fetch_pm_tasks).
        candidate_keys = [t["task_key"] for t in pm_tasks]
        recent_task_keys = [r.get("task_key") for r in recent if r.get("task_key")]
        # Session identity + the app_sessions row metadata, so a trace is
        # self-contained — you know WHICH session and its key fields (app, window
        # titles, time span) without opening meridian.db.
        db_span.set_attribute("session_id", session_id)
        db_span.set_attribute("app_name", str(session_raw.get("app_name") or ""))
        db_span.set_attribute("started_at", str(session_raw.get("started_at") or ""))
        db_span.set_attribute("ended_at", str(session_raw.get("ended_at") or ""))
        try:
            _wts = json.loads(session_raw.get("window_titles") or "[]")
            _wt_names = [str(w.get("window_name", "")) for w in _wts if w.get("window_name")]
        except (TypeError, ValueError):
            _wt_names = []
        db_span.set_attribute("window_titles", " | ".join(_wt_names) if _wt_names else "-")
        db_span.set_attribute("window_title_count", len(_wt_names))
        db_span.set_attribute("duration_s", float(session_raw.get("duration_s") or 0.0))
        db_span.set_attribute("text_source", str(session_raw.get("session_text_source") or ""))
        db_span.set_attribute("session_text_chars", len(session_text))
        # Frame-range attribution: the contiguous screenpipe frame_id window this
        # session was built from (min..max, inclusive) plus the kept frame count.
        # Answers "which capture window fed this classification" — the raw frames
        # live in screenpipe keyed by these ids. Coding-agent rows have no frames
        # (min/max = 0), so guard on a real range before stamping.
        _min_fid = session_raw.get("min_frame_id")
        _max_fid = session_raw.get("max_frame_id")
        if isinstance(_min_fid, int) and isinstance(_max_fid, int) and _max_fid > 0:
            db_span.set_attribute("min_frame_id", _min_fid)
            db_span.set_attribute("max_frame_id", _max_fid)
            db_span.set_attribute("frame_count", int(session_raw.get("frame_count") or 0))
        db_span.set_attribute("pm_tasks_count", len(pm_tasks))
        db_span.set_attribute("today_focus_count", len(focus_keys))
        db_span.set_attribute("recent_sessions_count", len(recent))
        db_span.set_attribute("candidate_task_keys", ", ".join(candidate_keys) if candidate_keys else "-")
        db_span.set_attribute("today_focus_keys", ", ".join(focus_keys) if focus_keys else "-")
        db_span.set_attribute("recent_task_keys", ", ".join(recent_task_keys) if recent_task_keys else "-")

    # ── contributing_frames ─────────────────────────────────────────────────────
    # Which screenpipe frames' OCR/a11y text actually fed this session's
    # session_text, captured at ETL formation time (durable across screenpipe
    # pruning; see migration 045). One self-describing span per session so a
    # misclassification can be traced back to the exact capture frames — without
    # needing screenpipe access. Skipped for sessions with no frame provenance
    # (coding-agent rows, or rows formed before migration 045).
    try:
        _frames = json.loads(session_raw.get("frame_contributions") or "[]")
    except (TypeError, ValueError):
        _frames = []
    if _frames:
        with tracer.start_as_current_span("contributing_frames") as cf_span:
            cf_span.set_attribute("frame_count", len(_frames))
            cf_span.set_attribute(
                "frame_ids", ", ".join(str(f.get("frame_id")) for f in _frames)
            )
            cf_span.set_attribute("first_frame_ts", str(_frames[0].get("timestamp", "")))
            cf_span.set_attribute("last_frame_ts", str(_frames[-1].get("timestamp", "")))
            cf_span.set_attribute("total_chars", sum(int(f.get("chars") or 0) for f in _frames))
            # Full per-frame detail (id, timestamp, source, chars) as a JSON blob,
            # so the exact provenance is one click away in the span attributes.
            cf_span.set_attribute("frames", json.dumps(_frames, separators=(",", ":")))

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

    # ── classifier_input ────────────────────────────────────────────────────────
    # The single drill-down span for "exactly what the classifier was asked".
    # It carries the COMPLETE input, byte-for-byte as handed to the model:
    #   • system_prompt — full system context + the task-classifier SKILL
    #   • llm_input     — full user message: the input session block, the recent
    #                     past-5 sessions, and the ranked candidate tickets
    # Both are captured POST-assembly, so any cap already applied while building
    # the prompt (e.g. SESSION_TEXT_CAP truncating the OCR excerpt) is reflected
    # here EXACTLY as the model saw it — never the pre-cap original. Concatenating
    # system_prompt + llm_input reproduces the model input verbatim.
    # (MLX path: system+user are passed verbatim to the model. The Apple FM
    # fallback rewrites these inside _classify_apple_fm — compact system, user
    # capped at ~8k chars — so on that path the on-span text is the assembled
    # input, not the rewritten one.)
    with tracer.start_as_current_span("classifier_input") as bp_span:
        user_message = build_user_message(session, pm_tasks, recent_sessions=recent)
        messages = [
            {"role": "system", "content": _SYSTEM_PROMPT},
            {"role": "user",   "content": user_message},
        ]
        # `llm_input` on the PARENT span is the input EXACTLY as sent to the model:
        # the two messages run through the tokenizer's chat template (role markers
        # + special tokens), i.e. the literal string the model decodes from. This
        # powers OO's "Input" panel, so one click shows the real wire prompt.
        # If the template can't be rendered (e.g. Apple FM path), fall back to a
        # plain system+user rendering so the panel is never empty. Content is
        # verbatim and post-cap either way. The child spans below break the same
        # input into its readable pieces.
        rendered = _render_model_prompt(messages)
        full_input = rendered if rendered is not None else (
            "===== SYSTEM MESSAGE (role: system) =====\n"
            f"{_SYSTEM_PROMPT}\n\n"
            "===== USER MESSAGE (role: user) =====\n"
            f"{user_message}"
        )
        bp_span.set_attribute("prompt_chars", len(user_message))
        bp_span.set_attribute("system_prompt_chars", len(_SYSTEM_PROMPT))
        bp_span.set_attribute("full_input_chars", len(full_input))
        bp_span.set_attribute("chat_template_rendered", rendered is not None)
        bp_span.set_attribute("llm_input", full_input)
        # Total input size in tokens (OTel GenAI semantic convention). Tokenizing
        # the prompt is deterministic, so this equals MLX's reported input tokens.
        _ptoks = _count_prompt_tokens(full_input)
        if _ptoks is not None:
            bp_span.set_attribute("gen_ai.usage.input_tokens", _ptoks)

        # Each input component is emitted as its OWN child span, so in the OO
        # waterfall `classifier_input` expands into clickable nodes — open any one
        # to read just that section. Each carries the EXACT rendered text the
        # model saw for that piece (same formatters/order as llm_input, post-cap),
        # stored under `llm_input` so OO renders it in that node's Input panel.
        def _input_part(name: str, text: str, **extra: Any) -> None:
            with tracer.start_as_current_span(name) as part:
                part.set_attribute("llm_input", text)
                part.set_attribute("chars", len(text))
                for _k, _v in extra.items():
                    part.set_attribute(_k, _v)

        _input_part("system_prompt", _SYSTEM_PROMPT)       # classifier skill + context
        _input_part("recent_context", _format_recent_sessions(recent))   # past-5 sessions
        _input_part("session_block", _format_session(session))           # the input session
        _input_part("candidate_tickets", _format_candidates(pm_tasks),   # ranked candidates
                    ticket_count=len(pm_tasks))

    # `messages` (built above for the chat-template render) is reused verbatim by
    # inference below — the same object that gets classified.

    # ── llm_inference ─────────────────────────────────────────────────────────
    t0 = time.time()
    with tracer.start_as_current_span("llm_inference") as llm_span:
        model_id = _resolve_model_id()
        # OTel GenAI semantic conventions — the request/operation attributes live
        # on the inference (operation) span; usage/response attrs are on
        # classifier_output, the input token count on classifier_input.
        llm_span.set_attribute("gen_ai.operation.name", "chat")
        llm_span.set_attribute("gen_ai.provider.name", "mlx")
        llm_span.set_attribute("gen_ai.request.model", model_id)
        llm_span.set_attribute("gen_ai.request.max_tokens", _MAX_TOKENS)
        llm_span.set_attribute("gen_ai.request.temperature", _TEMPERATURE)

        # Apple Intelligence path — no in-process MLX model; JSON parsing with retry.
        try:
            from agents.llm_selector import APPLE_INTELLIGENCE_ID
            _use_apple_fm = model_id == APPLE_INTELLIGENCE_ID
        except Exception:
            _use_apple_fm = False

        gen_stats: Any = None  # MLX GenerationResponse (token counts + tok/s), mlx path only
        try:
            if _use_apple_fm:
                result = _classify_apple_fm(messages)
                raw = result.model_dump_json()
            else:
                from mlx_lm import stream_generate
                from mlx_lm.sample_utils import make_sampler
                from outlines.inputs import Chat

                with model_session() as model:
                    sampler = make_sampler(temp=_TEMPERATURE)
                    try:
                        # Drive mlx_lm.stream_generate directly with outlines' FSM
                        # logits processor — identical constrained decoding to the
                        # high-level model(...) call, but we KEEP MLX's own
                        # GenerationResponse stats (prompt/generation token counts +
                        # tok/s + finish_reason) instead of discarding them. The
                        # processor is compiled exactly as outlines does it: a
                        # Generator turns the pydantic schema into the logits
                        # processor; type_adapter wraps it; format_input renders the
                        # chat prompt. The last streamed response carries the
                        # cumulative token counts.
                        from outlines.generator import Generator

                        _gen = Generator(model, SessionClassification)
                        logits_processors = model.type_adapter.format_output_type(
                            _gen.logits_processor
                        )
                        formatted = model.type_adapter.format_input(Chat(messages))
                        parts: list[str] = []
                        for gr in stream_generate(
                            model.model, model.mlx_tokenizer, formatted,
                            max_tokens=_MAX_TOKENS,
                            logits_processors=logits_processors,
                            sampler=sampler,
                        ):
                            parts.append(gr.text)
                            gen_stats = gr
                        raw = "".join(parts)
                    except Exception as stream_exc:  # noqa: BLE001
                        # Fallback to the high-level call if outlines' internals
                        # shift — text only, no stats — so classification never
                        # breaks just because the metadata path did. Logged (not
                        # silent) so the regression is visible.
                        log.warning(
                            "run_task_linker_mlx: stream-stats path failed (%s) — "
                            "falling back to model(...) without token stats",
                            stream_exc,
                        )
                        gen_stats = None
                        raw = model(
                            Chat(messages),
                            output_type=SessionClassification,
                            max_tokens=_MAX_TOKENS,
                            sampler=sampler,
                            verbose=False,
                        )
        except Exception as exc:
            elapsed = time.time() - t0
            outcome = "apple_fm_error" if _use_apple_fm else "mlx_error"
            llm_span.set_attribute("outcome", outcome)
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
                session_id, f"inference error: {exc}", elapsed, outcome
            )

        elapsed = time.time() - t0
        outcome = "apple_fm" if _use_apple_fm else "mlx_direct"
        llm_span.set_attribute("outcome", outcome)
        llm_span.set_attribute("elapsed_s", elapsed)
        # MLX's OWN generation metadata (token counts, throughput, finish_reason)
        # is carried by `gen_stats` and surfaced on the classifier_output span
        # below, co-located with the output. raw size/content also lives there.

    log.debug(
        "run_task_linker_mlx: session %d raw (%.1fs): %.200s",
        session_id, elapsed, raw,
    )

    # ── classifier_output ─────────────────────────────────────────────────────
    with tracer.start_as_current_span("classifier_output") as pr_span:
        # `llm_output` is the COMPLETE raw model output — never truncated, so you
        # can see EXACTLY what came out. Named `llm_output` (mirroring the parent
        # `llm_input`) so OpenObserve renders it in the span's "Output" panel. The
        # verdict (task_key/confidence/session_type/category) lives on the root
        # classify_session span, so it isn't repeated here.
        pr_span.set_attribute("raw_chars", len(raw))
        pr_span.set_attribute("llm_output", raw)
        # MLX's OWN generation metadata, co-located with the output it describes —
        # exact token counts + throughput as REPORTED BY the model (not
        # re-tokenized by us), under the OTel GenAI semantic conventions. tok/s and
        # peak memory have no standard attribute name, so they stay custom. None on
        # the Apple FM path, which has no MLX stats.
        if gen_stats is not None:
            # These stats are TELEMETRY, never load-bearing — so a None/renamed/
            # non-numeric field (early-truncated stream, an mlx_lm field rename)
            # must NOT raise out of here and fail the whole classify batch (the
            # server's _classify_all loop doesn't wrap the per-session call). Coerce
            # defensively and drop any stat that won't convert.
            try:
                _in = int(gen_stats.prompt_tokens)
                _out = int(gen_stats.generation_tokens)
                pr_span.set_attribute("gen_ai.usage.input_tokens", _in)
                pr_span.set_attribute("gen_ai.usage.output_tokens", _out)
                pr_span.set_attribute("gen_ai.usage.total_tokens", _in + _out)
                pr_span.set_attribute("gen_ai.response.model", _resolve_model_id())
                pr_span.set_attribute("prompt_tps", round(float(gen_stats.prompt_tps), 2))
                pr_span.set_attribute("generation_tps", round(float(gen_stats.generation_tps), 2))
                pr_span.set_attribute("peak_memory_gb", round(float(gen_stats.peak_memory), 2))
                if gen_stats.finish_reason is not None:
                    pr_span.set_attribute(
                        "gen_ai.response.finish_reason", str(gen_stats.finish_reason)
                    )
            except (TypeError, ValueError, AttributeError) as exc:
                log.warning("classify: dropping unparseable gen_stats: %s", exc)

        # Both paths converge on a JSON string in `raw`; parse to SessionClassification.
        # Apple FM already validated once inside _classify_apple_fm; re-parsing from
        # model_dump_json() is a no-op that keeps the two paths uniform.
        try:
            result = SessionClassification.model_validate_json(raw)
        except Exception as exc:
            pr_span.set_attribute("outcome", "schema_error")
            pr_span.set_status(StatusCode.ERROR, str(exc))
            # Full raw output is already on `llm_output_raw`; the event only adds
            # the validation error message.
            pr_span.add_event("parse_failure", {"error": str(exc)[:300]})
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
                "error": f"model returned unknown task_key {task_key!r}",
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
        # The routing VERDICT (task_key/confidence/session_type/category) is the
        # one-line answer and is promoted onto the root classify_session span by
        # _annotate_classification_span — not repeated here.
        confidence = max(0.0, min(1.0, result.confidence))
        pr_span.set_attribute("outcome", "ok")

        # Each meaningful output field as its OWN child span, mirroring
        # classifier_input — so classifier_output expands into clickable nodes.
        # These carry the rich, readable output the model produced (the verdict
        # itself stays on the root, above):
        #   • reasoning       — why this task match
        #   • category        — the category justification (+ category/confidence)
        #   • dimensions       — the inferred activity tags
        #   • session_summary  — the PM-facing prose deliverable
        def _output_part(name: str, text: str, **extra: Any) -> None:
            with tracer.start_as_current_span(name) as part:
                part.set_attribute("llm_output", text)
                part.set_attribute("chars", len(text))
                for _k, _v in extra.items():
                    part.set_attribute(_k, _v)

        _output_part("reasoning", result.reasoning)
        _output_part(
            "category", result.category_explanation,
            category=result.category,
            category_confidence=max(0.0, min(1.0, result.category_confidence)),
        )
        _output_part(
            "dimensions", json.dumps(result.dimensions, default=str),
            dimension_keys=", ".join(sorted(result.dimensions.keys())) or "-",
        )
        _output_part("session_summary", result.session_summary)

    return {
        "session_id":           session_id,
        "task_key":             task_key,
        "confidence":           confidence,
        "category":             result.category,
        "category_confidence":  max(0.0, min(1.0, result.category_confidence)),
        "category_explanation": result.category_explanation,
        "session_type":         result.session_type,
        "reasoning":            result.reasoning,
        "method":              outcome,
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


# `method` values that mean the model produced a usable classification.
# Anything else is an error path the dashboard surfaces under errors-only. The
# real error `method` values emitted by `_error_result` are `mlx_parse_error`
# (schema validation / unknown task_key — those names are child-span `outcome`
# attributes, NOT methods) and `mlx_error` (inference failure / session-not-found).
_SUCCESS_METHODS = {"mlx_direct", "apple_fm"}


def _annotate_classification_span(result: dict[str, Any]) -> None:
    """Promote the classification result onto the enclosing `classify_session`
    span so each session is ONE self-describing row in OpenObserve — filterable
    by session_id / session_type / task_key / is_error without joining the child
    spans. Both the server and CLI entry points wrap the call in a
    `classify_session` span, so annotating the current span here covers both.
    """
    span = trace.get_current_span()
    if not span.is_recording():
        return
    method = str(result.get("method", ""))
    task_key = result.get("task_key")
    is_error = method not in _SUCCESS_METHODS
    span.set_attribute("session_id", int(result.get("session_id", 0)))
    span.set_attribute("task_key", task_key or "-")
    span.set_attribute("has_task", task_key is not None)
    span.set_attribute("session_type", str(result.get("session_type", "")))
    span.set_attribute("category", str(result.get("category", "")))
    span.set_attribute("confidence", float(result.get("confidence", 0.0)))
    span.set_attribute(
        "category_confidence", float(result.get("category_confidence", 0.0))
    )
    span.set_attribute("method", method)
    span.set_attribute("elapsed_s", float(result.get("elapsed_s", 0.0)))
    span.set_attribute("is_error", is_error)
    if is_error:
        span.set_status(StatusCode.ERROR, str(result.get("reasoning", method))[:300])


def _classify_one_logged(
    session_id: int,
    con: _sqlite3.Connection,
    run_log: Any,
) -> dict[str, Any]:
    """Classify one session, marking the span is_error=true on ANY failure.

    Wraps the inner worker so an UNHANDLED exception (a sqlite read error,
    malformed window_titles JSON, …) still stamps is_error=true + ERROR status on
    the enclosing classify_session span before propagating — otherwise the
    dashboard's errors-only table (which filters is_error='true') silently misses
    exactly the crashes an operator opens it to find. Handled failures already
    return _error_result dicts that _annotate_classification_span marks.
    """
    try:
        return _classify_one_logged_inner(session_id, con, run_log)
    except Exception as exc:  # noqa: BLE001 — annotate, then re-raise unchanged
        span = trace.get_current_span()
        if span.is_recording():
            span.set_attribute("session_id", int(session_id))
            span.set_attribute("is_error", True)
            span.set_attribute("method", "mlx_error")
            span.set_status(StatusCode.ERROR, str(exc)[:300])
        log.exception(
            "run_task_linker_mlx: unhandled error classifying session %d", session_id
        )
        raise


def _classify_one_logged_inner(
    session_id: int,
    con: _sqlite3.Connection,
    run_log: Any,
) -> dict[str, Any]:
    """Classify one session and append a full record to the run log."""
    # Gather inputs before classification so we can log them even on error.
    session_raw = _fetch_session(con, session_id)
    focus_keys = _fetch_plan_focus(con, _local_day(session_raw.get("started_at") or "")) if session_raw else []
    pm_tasks = _fetch_pm_tasks(con, focus_keys) if session_raw else []
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
    _annotate_classification_span(result)
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

    # Canonicalize and restrict to ~/.meridian/ to prevent path traversal.
    try:
        canonical = Path(db_path).expanduser().resolve()
    except (OSError, ValueError) as exc:
        log.error("run_task_linker_mlx: invalid db path: %s", exc)
        sys.exit(1)
    allowed_root = Path.home() / ".meridian"
    if not str(canonical).startswith(str(allowed_root) + "/") and canonical != allowed_root:
        log.error(
            "run_task_linker_mlx: db path %s is outside allowed directory %s",
            canonical, allowed_root,
        )
        sys.exit(1)
    db_path = str(canonical)

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
                with tracer.start_as_current_span("classify_session"):
                    # _classify_one_logged enriches this span with the full
                    # result (session_id, task_key, session_type, is_error, …).
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

        root_span.set_attribute("results_count", len(results))
        sys.stdout.write(json.dumps({"results": results}) + "\n")
        sys.stdout.flush()

    observability.shutdown()


if __name__ == "__main__":
    main()
