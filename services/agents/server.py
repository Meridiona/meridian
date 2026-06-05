# meridian — normalises screenpipe activity into structured app sessions
"""Conversational agent server (FastAPI).

Supports two backends selected via --backend:

  hermes (default) — wraps the hermes AIAgent for conversational classification
  mlx              — loads the MLX model once at startup for direct inference

Usage:
    python -m agents.server                    # hermes backend, port 7823
    python -m agents.server --backend mlx      # MLX backend, port 7823
    python -m agents.server --backend mlx --port 7824

hermes endpoints:
    GET  /health
    POST /chat       {"message": "classify session id 80"}

mlx endpoints:
    GET  /health
    POST /classify   {"input": "<fully-formatted user_message string>"}
                     → {"task_key": ..., "session_type": ..., "reasoning": ...,
                        "confidence": ..., "dimensions": {...}}
"""
from __future__ import annotations

import argparse
import contextlib
import logging
import os
import sqlite3 as _sqlite3
import sys
from contextlib import asynccontextmanager
from pathlib import Path
from typing import Any, AsyncIterator

# Must be set before any hermes import.
_SERVICES_DIR = Path(__file__).parent.parent
os.environ.setdefault("HERMES_HOME", str(_SERVICES_DIR / ".hermes"))

import opentelemetry.context as _otel_context
from fastapi import FastAPI, HTTPException
from opentelemetry import trace
from pydantic import BaseModel, Field

from agents import observability

log = logging.getLogger("agents.server")

_DB_PATH = Path(os.environ.get("MERIDIAN_DB", Path.home() / ".meridian/meridian.db"))

# ---------------------------------------------------------------------------
# Lifespan — model loaded once for MLX backend, no-op for hermes
# ---------------------------------------------------------------------------

_app_state: dict[str, Any] = {}


@asynccontextmanager
async def _lifespan(app: FastAPI) -> AsyncIterator[None]:
    if _app_state.get("backend") == "mlx":
        import datetime
        import agents.run_task_linker_mlx as _mlx
        _app_state["mlx_module"] = _mlx
        _app_state["loaded_at"] = datetime.datetime.now(datetime.timezone.utc).isoformat()
        from agents.llm_selector import APPLE_INTELLIGENCE_ID
        if _mlx._resolve_model_id() == APPLE_INTELLIGENCE_ID:
            log.info("server: 8 GB machine — Apple Intelligence backend, no MLX model to pre-load")
        else:
            log.info("server: loading MLX model at startup…")
            _mlx._get_model()
            log.info("server: MLX model ready")
    yield


app = FastAPI(title="Meridian Agent", version="1.0.0", lifespan=_lifespan)


# ---------------------------------------------------------------------------
# Shared
# ---------------------------------------------------------------------------

@app.get("/health")
async def health() -> dict:
    return {
        "status": "ok",
        "backend": _app_state.get("backend", "hermes"),
        "db": str(_DB_PATH),
        "db_exists": _DB_PATH.exists(),
    }


@app.get("/info")
async def info() -> dict:
    """Return the identity of the loaded model.

    model_id is None for the hermes backend (no model loaded in-process).
    loaded_at is an ISO-8601 UTC timestamp set when the model finished loading.
    """
    m = _app_state.get("mlx_module")
    return {
        "backend":   _app_state.get("backend", "hermes"),
        "model_id":  m._resolve_model_id() if m else None,
        "loaded_at": _app_state.get("loaded_at"),
    }


# ---------------------------------------------------------------------------
# Hermes backend — conversational AIAgent
# ---------------------------------------------------------------------------

class ChatRequest(BaseModel):
    message: str


class ChatResponse(BaseModel):
    response: str


@app.post("/chat", response_model=ChatResponse)
async def chat(req: ChatRequest) -> ChatResponse:
    if _app_state.get("backend") == "mlx":
        raise HTTPException(status_code=404, detail="Use /classify for mlx backend")

    from run_agent import AIAgent
    from agents._system_context import SYSTEM_CONTEXT
    from agents.config import MODEL, BASE_URL, API_KEY, AGENT_MAX_TOKENS

    agent = AIAgent(
        model=MODEL,
        base_url=BASE_URL,
        api_key=API_KEY or "none",
        enabled_toolsets=["terminal", "skills", "memory"],
        ephemeral_system_prompt=SYSTEM_CONTEXT,
        quiet_mode=True,
        skip_context_files=True,
        load_soul_identity=False,
        skip_memory=False,
        max_iterations=20,
        max_tokens=AGENT_MAX_TOKENS,
    )

    log.info("chat: %.120s", req.message)
    with contextlib.redirect_stdout(sys.stderr):
        result = agent.run_conversation(req.message)

    response = str(result.get("final_response") or result.get("response") or "")
    log.info("response: %.120s", response)
    return ChatResponse(response=response)


# ---------------------------------------------------------------------------
# MLX backend — direct in-process inference, model pre-loaded at startup
# ---------------------------------------------------------------------------

_MAX_INPUT_CHARS = 128_000  # ~32k tokens; hard ceiling to prevent resource exhaustion


class ClassifyRequest(BaseModel):
    input: str = Field(..., max_length=_MAX_INPUT_CHARS)  # fully-formatted user_message string


class ClassifyResponse(BaseModel):
    task_key: str | None
    session_type: str
    reasoning: str
    confidence: float
    dimensions: dict


_classify_log: "Any | None" = None  # JSONL file handle, opened at first request


def _get_classify_log() -> "Any":
    global _classify_log
    if _classify_log is None:
        import datetime
        log_dir = _SERVICES_DIR / "logs" / "mlx"
        log_dir.mkdir(parents=True, exist_ok=True)
        ts = datetime.datetime.now().strftime("%Y%m%dT%H%M%S")
        log_path = log_dir / f"server_{ts}.jsonl"
        _classify_log = log_path.open("w", encoding="utf-8")
        log.info("classify: writing request log to %s", log_path)
    return _classify_log


@app.post("/classify", response_model=ClassifyResponse)
async def classify(req: ClassifyRequest) -> ClassifyResponse:
    if _app_state.get("backend") != "mlx":
        raise HTTPException(status_code=404, detail="Use /chat for hermes backend")

    import json as _json
    import time as _time
    from outlines.inputs import Chat
    from mlx_lm.sample_utils import make_sampler

    from agents.llm_selector import APPLE_INTELLIGENCE_ID

    m = _app_state["mlx_module"]
    messages = [
        {"role": "system", "content": m._SYSTEM_PROMPT},
        {"role": "user",   "content": req.input},
    ]
    t0 = _time.time()
    try:
        if m._resolve_model_id() == APPLE_INTELLIGENCE_ID:
            result = m._classify_apple_fm(messages)
        else:
            model = m._get_model()
            raw = model(
                Chat(messages),
                output_type=m.SessionClassification,
                max_tokens=m._MAX_TOKENS,
                sampler=make_sampler(temp=m._TEMPERATURE),
                verbose=False,
            )
            result = m.SessionClassification.model_validate_json(raw)
    except Exception as exc:
        log.warning("classify: inference error: %s", exc)
        raise HTTPException(status_code=500, detail=str(exc)) from exc

    elapsed = _time.time() - t0
    response = ClassifyResponse(
        task_key=result.task_key,
        session_type=result.session_type,
        reasoning=result.reasoning,
        confidence=max(0.0, min(1.0, result.confidence)),
        dimensions=result.dimensions,
    )

    log.info(
        "classify: task_key=%s session_type=%s confidence=%.2f elapsed=%.2fs",
        result.task_key, result.session_type, result.confidence, elapsed,
    )

    # Append a JSONL record to services/logs/mlx/server_<ts>.jsonl
    try:
        fh = _get_classify_log()
        fh.write(_json.dumps({
            "input":        req.input[:500],  # truncated for log readability
            "task_key":     result.task_key,
            "session_type": result.session_type,
            "confidence":   result.confidence,
            "reasoning":    result.reasoning,
            "elapsed_s":    round(elapsed, 2),
        }, ensure_ascii=False) + "\n")
        fh.flush()
    except Exception as exc:
        log.warning("classify: failed to write log entry: %s", exc)

    return response


# ---------------------------------------------------------------------------
# MLX backend — classify_sessions (batch, session-id driven)
# ---------------------------------------------------------------------------

class ClassifySessionsRequest(BaseModel):
    session_ids: list[int]
    meridian_db: str
    traceparent: str | None = None  # W3C traceparent propagated from Rust caller


@app.post("/classify_sessions")
async def classify_sessions(req: ClassifySessionsRequest) -> dict:
    """Classify one or more sessions by ID using the pre-loaded MLX model.

    Only available when the server is started with ``--backend mlx``.
    Returns a results list in the same JSON format the Rust daemon already
    parses from the subprocess stdout.
    """
    from fastapi.concurrency import run_in_threadpool

    if _app_state.get("backend") != "mlx":
        raise HTTPException(
            status_code=503,
            detail="classify_sessions is only available with --backend mlx",
        )

    m = _app_state.get("mlx_module")
    if m is None:
        raise HTTPException(status_code=503, detail="MLX model is still loading")

    fh = _get_classify_log()
    tracer = _app_state.get("tracer") or trace.get_tracer("meridian-agent-server-mlx")
    parent_ctx = observability.extract_parent_context(req.traceparent)

    with tracer.start_as_current_span("classify_sessions", context=parent_ctx) as span:
        span.set_attribute("session_count", len(req.session_ids))

        # Snapshot the OTel context while classify_sessions span is active so we
        # can attach it explicitly inside the threadpool (anyio copies contextvars,
        # but explicit attach is more reliable across anyio versions).
        ctx_snapshot = _otel_context.get_current()

        def _classify_all() -> list[dict]:
            # Attach classify_sessions context so _classify_one sub-spans
            # (db_fetch, build_prompt, llm_inference, parse_response) appear
            # as children of classify_sessions in the OO trace waterfall.
            _tok = _otel_context.attach(ctx_snapshot)
            try:
                # Always use the server's own _DB_PATH — ignoring req.meridian_db avoids
                # path-traversal: the server knows its DB from the environment.
                con = _sqlite3.connect(str(_DB_PATH), check_same_thread=False)
                con.row_factory = _sqlite3.Row
                try:
                    results: list[dict] = []
                    for sid in req.session_ids:
                        with tracer.start_as_current_span(
                            "classify_session",
                            attributes={"session_id": sid},
                        ):
                            result = m._classify_one_logged(sid, con, fh)
                        log.info(
                            "classify_sessions: session_id=%d task_key=%s session_type=%s elapsed_s=%.2f",
                            sid,
                            result.get("task_key"),
                            result.get("session_type"),
                            result.get("elapsed_s", 0.0),
                        )
                        results.append(result)
                    return results
                finally:
                    con.close()
            finally:
                _otel_context.detach(_tok)

        results = await run_in_threadpool(_classify_all)
        span.set_attribute("classified_count", len(results))

    return {"results": results}


# ---------------------------------------------------------------------------
# OpenAI-compatible chat completions
# ---------------------------------------------------------------------------
#
# A minimal `/v1/chat/completions` endpoint that lets agno, the openai
# python SDK, and anything else expecting OpenAI's wire format talk to
# our already-loaded MLX model. Free-form generation only (no FSM /
# structured-output decoding); callers that need JSON shape inject it
# into the system prompt.
#
# Only available with --backend mlx. The hermes backend keeps using
# /chat for its conversational path.


class _OAIMessage(BaseModel):
    role: str
    content: str | list | None = None


class _OAIChatRequest(BaseModel):
    model: str | None = None
    messages: list[_OAIMessage]
    temperature: float | None = None
    max_tokens: int | None = Field(None, ge=1, le=8192)
    top_p: float | None = None
    stop: list[str] | str | None = None
    stream: bool = False
    # Tolerate unknown fields agno / openai-python may add.
    response_format: dict | None = None


def _flatten_message_content(content: Any) -> str:
    """OpenAI allows `content` to be a list of typed parts; we flatten to text."""
    if content is None:
        return ""
    if isinstance(content, str):
        return content
    if isinstance(content, list):
        out: list[str] = []
        for part in content:
            if isinstance(part, dict) and part.get("type") == "text":
                out.append(part.get("text", ""))
            elif isinstance(part, str):
                out.append(part)
        return "\n".join(out)
    return str(content)


# Apple FM context cap: 4096-token combined context window (input + output).
# Reserve ~1024 tokens for the response; ~3072 for the prompt → ~12 000 chars.
_APPLE_FM_USER_CHARS = 12_000


def _infer_apple_fm(msgs: list[dict], max_tokens: int) -> str:  # noqa: ARG001
    """Infer via Apple Foundation Models from an OpenAI-style messages list.

    Extracts the last system message and joins all user/assistant turns.
    Raises on failure — callers must handle and return 500.
    """
    import asyncio
    from apple_fm_sdk import LanguageModelSession  # type: ignore[import]

    system = next(
        (m["content"] for m in reversed(msgs) if m.get("role") == "system"), ""
    )
    user_parts = [m["content"] for m in msgs if m.get("role") in ("user", "assistant")]
    user = "\n".join(user_parts)
    if len(user) > _APPLE_FM_USER_CHARS:
        user = user[:_APPLE_FM_USER_CHARS]

    async def _run() -> str:
        session = LanguageModelSession(instructions=system)
        result = await session.respond(user)
        return result.content if hasattr(result, "content") else str(result)

    loop = asyncio.new_event_loop()
    try:
        return loop.run_until_complete(_run())
    finally:
        loop.close()


@app.post("/v1/chat/completions")
async def openai_chat_completions(req: _OAIChatRequest) -> dict:
    """OpenAI ChatCompletions-shaped wrapper around the MLX model.

    Streaming is rejected for now — agno's structured-output path uses
    non-streaming for JSON mode, so this covers our use case.
    """
    import time as _time
    import uuid as _uuid

    from fastapi.concurrency import run_in_threadpool

    if _app_state.get("backend") != "mlx":
        raise HTTPException(
            status_code=503,
            detail="/v1/chat/completions is only available with --backend mlx",
        )
    if req.stream:
        raise HTTPException(status_code=400, detail="streaming not supported")

    m = _app_state.get("mlx_module")
    if m is None:
        raise HTTPException(status_code=503, detail="MLX model is still loading")

    from mlx_lm.sample_utils import make_sampler
    from outlines.inputs import Chat

    # Normalise messages — OpenAI tolerates list-of-parts; outlines wants strings.
    msgs = [
        {"role": msg.role, "content": _flatten_message_content(msg.content)}
        for msg in req.messages
    ]

    temperature = req.temperature if req.temperature is not None else 0.3
    max_tokens  = req.max_tokens if req.max_tokens else 2048

    from agents.llm_selector import APPLE_INTELLIGENCE_ID

    def _generate() -> str:
        if m._resolve_model_id() == APPLE_INTELLIGENCE_ID:
            return _infer_apple_fm(msgs, max_tokens)
        model = m._get_model()
        return model(
            Chat(msgs),
            max_tokens=max_tokens,
            sampler=make_sampler(temp=temperature),
            verbose=False,
        )

    t0 = _time.time()
    try:
        text = await run_in_threadpool(_generate)
    except Exception as exc:                            # noqa: BLE001
        log.warning("openai_chat_completions: inference error: %s", exc)
        raise HTTPException(status_code=500, detail=str(exc)) from exc
    elapsed = _time.time() - t0

    completion_id = f"chatcmpl-{_uuid.uuid4().hex[:24]}"
    # Token counts are approximations — outlines doesn't expose exact
    # counts without re-tokenising; 4 bytes/token is the OpenAI convention.
    prompt_chars = sum(len(msg["content"]) for msg in msgs)
    prompt_tokens = max(1, prompt_chars // 4)
    completion_tokens = max(1, len(text) // 4)

    log.info(
        "openai_chat_completions: msgs=%d max_tokens=%d temp=%.2f elapsed=%.2fs out_chars=%d",
        len(msgs), max_tokens, temperature, elapsed, len(text),
    )

    return {
        "id":      completion_id,
        "object":  "chat.completion",
        "created": int(_time.time()),
        "model":   req.model or "qwen3.5-9b-instruct",
        "choices": [
            {
                "index":         0,
                "message":       {"role": "assistant", "content": text},
                "finish_reason": "stop",
            }
        ],
        "usage": {
            "prompt_tokens":     prompt_tokens,
            "completion_tokens": completion_tokens,
            "total_tokens":      prompt_tokens + completion_tokens,
        },
    }


class _SummariseRequest(BaseModel):
    transcript: str = Field(..., max_length=_MAX_INPUT_CHARS)
    system: str | None = None
    max_tokens: int = Field(2048, ge=1, le=8192)
    temperature: float = 0.2


class _SummariseResponse(BaseModel):
    summary: str
    blockers: list[str] = []


class _SummarySchema(BaseModel):
    """Outlines FSM-constrains generation to this shape, so a reasoning model
    physically cannot emit chain-of-thought — only the JSON object."""
    summary: str
    blockers: list[str] = []


_SUMMARISE_DEFAULT_SYSTEM = (
    "Summarise the coding-session transcript into a factual prose work-log "
    "summary. Name files edited, commands run, errors hit, decisions made, "
    "tests/validations, and any rework or blockers. State ONLY what is in the "
    "transcript — never invent files, commands, or outcomes."
)


@app.post("/summarise", response_model=_SummariseResponse)
async def summarise(req: _SummariseRequest) -> _SummariseResponse:
    """Schema-constrained session summary (MLX backend only).

    Unlike /v1/chat/completions, this forces the {summary, blockers} schema via
    outlines — the fix for reasoning models leaking chain-of-thought into the
    summary. Used by the coding_agent_summariser's MLX fallback path.
    """
    from fastapi.concurrency import run_in_threadpool

    if _app_state.get("backend") != "mlx":
        raise HTTPException(status_code=503, detail="/summarise requires --backend mlx")
    m = _app_state.get("mlx_module")
    if m is None:
        raise HTTPException(status_code=503, detail="MLX model is still loading")

    from agents.llm_selector import APPLE_INTELLIGENCE_ID

    messages = [
        {"role": "system", "content": req.system or _SUMMARISE_DEFAULT_SYSTEM},
        {"role": "user", "content": req.transcript},
    ]

    if m._resolve_model_id() == APPLE_INTELLIGENCE_ID:
        # outlines FSM decoding is incompatible with Foundation Models.
        # Ask Apple FM for JSON directly; strip fences and retry once on parse error.
        _JSON_HINT = (
            "\n\nRespond ONLY with a JSON object — no markdown, no explanation: "
            '{"summary": "<string>", "blockers": ["<string>", ...]}'
        )

        def _generate_fm() -> _SummarySchema:
            fm_msgs = [
                {"role": "system", "content": messages[0]["content"] + _JSON_HINT},
                {"role": "user",   "content": messages[1]["content"]},
            ]
            raw = _infer_apple_fm(fm_msgs, req.max_tokens)
            try:
                return _SummarySchema.model_validate_json(raw)
            except Exception:
                stripped = raw.strip().removeprefix("```json").removeprefix("```").removesuffix("```").strip()
                return _SummarySchema.model_validate_json(stripped)

        from fastapi.concurrency import run_in_threadpool as _rtp
        try:
            obj = await _rtp(_generate_fm)
        except Exception as exc:  # noqa: BLE001
            log.warning("summarise(apple_fm): parse error: %s", exc)
            raise HTTPException(status_code=500, detail=str(exc)) from exc
        log.info("summarise(apple_fm): out_chars=%d blockers=%d", len(obj.summary), len(obj.blockers))
        return _SummariseResponse(summary=obj.summary.strip(), blockers=obj.blockers)

    from mlx_lm.sample_utils import make_sampler
    from outlines.inputs import Chat

    def _generate() -> str:
        model = m._get_model()
        return model(
            Chat(messages),
            output_type=_SummarySchema,
            max_tokens=req.max_tokens,
            sampler=make_sampler(temp=req.temperature),
            verbose=False,
        )

    try:
        raw = await run_in_threadpool(_generate)
        obj = _SummarySchema.model_validate_json(raw)
    except Exception as exc:                            # noqa: BLE001
        log.warning("summarise: inference/parse error: %s", exc)
        raise HTTPException(status_code=500, detail=str(exc)) from exc

    log.info("summarise: out_chars=%d blockers=%d", len(obj.summary), len(obj.blockers))
    return _SummariseResponse(summary=obj.summary.strip(), blockers=obj.blockers)


# ---------------------------------------------------------------------------
# MLX backend — pm-worklog synth (Stage 4)
#
# Runs the agno Synthesise agent (skill + guardrails + JiraUpdate schema)
# in-process so the model server is the single LLM host for every stage. The
# Rust daemon owns Collect/Ground/Route; this endpoint is the ONLY LLM hop for
# the worklog. Serialisation across stages is guaranteed by the Rust-side global
# LLM gate — this handler does no locking of its own.
# ---------------------------------------------------------------------------

class _SynthWorklogRequest(BaseModel):
    # A SessionBundle.model_dump() — the classified sessions + ticket context
    # Rust collected for one (task, hour) window.
    bundle: dict
    debug: bool = False


def _get_synth_agent() -> "Any":
    """Build the agno Synthesise agent once and cache it on the app state.

    Lazy + cached: agno is a heavy import, and the classify-only use of this
    server must not pay for it. The agent's model is OpenAILike pointed back at
    this same server's /v1 endpoint (loopback) — the existing wiring.
    """
    agent = _app_state.get("synth_agent")
    if agent is not None:
        return agent
    from agno.db.sqlite import SqliteDb

    from agents.pm_worklog_update import agents as pm_agents

    agno_db = SqliteDb(
        db_file=str(_DB_PATH),
        session_table="agno_workflow_sessions",
        memory_table="agno_pm_worklog_memories",
        metrics_table="agno_pm_worklog_metrics",
        eval_table="agno_pm_worklog_eval_runs",
        approvals_table="agno_pm_worklog_approvals",
    )
    agent = pm_agents.build_synth_agent(db=agno_db)
    _app_state["synth_agent"] = agent
    log.info("synthesise_worklog: agno synth agent built")
    return agent


@app.post("/synthesise_worklog")
async def synthesise_worklog(req: _SynthWorklogRequest) -> dict:
    """Synthesise ONE Jira worklog from a collected session bundle (MLX only).

    Returns a JiraUpdate dict. The authoritative scalar fields (task_key,
    window, cycle_index, time_spent_seconds) are stamped from the bundle so the
    LLM can never override them — it only authors the prose + evidence bullets.
    Rust grounds, routes, and posts.
    """
    from fastapi.concurrency import run_in_threadpool

    if _app_state.get("backend") != "mlx":
        raise HTTPException(
            status_code=503, detail="/synthesise_worklog requires --backend mlx"
        )

    from agents.pm_worklog_update import workflow as pm_workflow
    from agents.pm_worklog_update.models import JiraUpdate, SessionBundle

    try:
        bundle = SessionBundle.model_validate(req.bundle)
    except Exception as exc:  # noqa: BLE001
        raise HTTPException(status_code=422, detail=f"bad bundle: {exc}") from exc

    agent = _get_synth_agent()
    user_message = pm_workflow._render_workflow_input(bundle)

    def _run() -> "Any":
        return agent.run(input=user_message)

    # The local model is non-deterministic (temp > 0) and occasionally emits an
    # output that doesn't parse into a JiraUpdate. Retry a couple of times before
    # giving up — a fresh sample almost always parses.
    update = None
    last_detail = "no attempt"
    for attempt in range(1, 4):
        try:
            response = await run_in_threadpool(_run)
        except Exception as exc:  # noqa: BLE001 — never crash the shared server
            last_detail = f"agent run failed: {exc}"
            log.warning("synthesise_worklog: attempt %d %s", attempt, last_detail)
            continue
        raw = getattr(response, "content", response)
        update = pm_workflow._coerce_jira(raw)
        if update is not None:
            break
        last_detail = "agent output did not parse into a JiraUpdate"
        log.warning("synthesise_worklog: attempt %d %s", attempt, last_detail)

    if update is None:
        raise HTTPException(
            status_code=500, detail=f"synth produced no JiraUpdate after 3 attempts ({last_detail})"
        )

    # Stamp authoritative fields from the bundle — never trust the LLM for these.
    update = update.model_copy(
        update={
            "task_key":           bundle.task_key,
            "window_start":       bundle.window_start,
            "window_end":         bundle.window_end,
            "cycle_index":        bundle.cycle_index,
            "time_spent_seconds": bundle.real_seconds,
        }
    )
    log.info(
        "synthesise_worklog: task=%s sessions=%d summary_chars=%d bullets=%d conf=%.2f",
        bundle.task_key, len(bundle.sessions), len(update.summary or ""),
        len(update.bullets), update.confidence,
    )
    return update.model_dump()


@app.get("/v1/models")
async def openai_models_list() -> dict:
    """OpenAI-style models listing — agno/openai-python probe this on first use."""
    model_id = "qwen3.5-9b-instruct"
    if _app_state.get("backend") == "mlx":
        m = _app_state.get("mlx_module")
        if m is not None and hasattr(m, "_resolve_model_id"):
            model_id = m._resolve_model_id()
    return {
        "object": "list",
        "data": [
            {
                "id":       model_id,
                "object":   "model",
                "created":  0,
                "owned_by": "meridian-local",
            }
        ],
    }


# ---------------------------------------------------------------------------
# Entry point
# ---------------------------------------------------------------------------

def main() -> None:
    import uvicorn

    parser = argparse.ArgumentParser(description="Meridian agent server")
    parser.add_argument("--port",    type=int, default=7823)
    parser.add_argument("--host",    default="127.0.0.1")
    parser.add_argument(
        "--backend",
        choices=["hermes", "mlx"],
        default="hermes",
        help="hermes: AIAgent conversational mode  |  mlx: direct in-process inference",
    )
    args = parser.parse_args()

    _app_state["backend"] = args.backend
    tracer = observability.setup(f"meridian-agent-server-{args.backend}")
    _app_state["tracer"] = tracer

    log.info("meridian agent server (%s) on http://%s:%d", args.backend, args.host, args.port)
    uvicorn.run(app, host=args.host, port=args.port, log_level="warning")


if __name__ == "__main__":
    main()
