# ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
"""MLX agent server (FastAPI).

Usage:
    python -m agents.server           # port 7823
    python -m agents.server --port 7824

Endpoints:
    GET  /health
    POST /classify   {"input": "<fully-formatted user_message string>"}
                     → {"task_key": ..., "session_type": ..., "reasoning": ...,
                        "confidence": ..., "dimensions": {...}}
"""
from __future__ import annotations

import argparse
import logging
import os
import sqlite3 as _sqlite3
from contextlib import asynccontextmanager
from pathlib import Path
from typing import Any, AsyncIterator

_SERVICES_DIR = Path(__file__).parent.parent

import opentelemetry.context as _otel_context
from fastapi import FastAPI, HTTPException
from opentelemetry import trace
from pydantic import BaseModel, Field

from agents import observability

log = logging.getLogger("agents.server")

_DB_PATH = Path(os.environ.get("MERIDIAN_DB", Path.home() / ".meridian/meridian.db"))

# ---------------------------------------------------------------------------
# Lifespan — model loaded once at startup
# ---------------------------------------------------------------------------

_app_state: dict[str, Any] = {}


async def _idle_evictor(mlx_module: Any) -> None:
    """Background loop: evict the MLX model after it has been idle long enough.

    Runs the (briefly blocking) eviction in a threadpool so it never stalls the
    event loop, and never raises out — the evictor must outlive transient errors.
    """
    import asyncio
    from fastapi.concurrency import run_in_threadpool

    ttl = mlx_module._IDLE_EVICT_S
    if ttl <= 0:
        return
    interval = max(15.0, ttl / 4.0)   # check ~4× per idle window
    while True:
        await asyncio.sleep(interval)
        try:
            await run_in_threadpool(mlx_module.maybe_evict_idle)
        except Exception as exc:       # noqa: BLE001 — evictor must never die
            log.warning("server: idle-evictor error: %s", exc)


def _model_sem() -> "asyncio.Semaphore":
    """Return the process-global single-slot model semaphore.

    Created once in _lifespan and stored in _app_state. Every endpoint that
    runs a model inference acquires this before calling run_in_threadpool so
    that classify, synthesise_worklog, and summarise never compete on the GPU.
    The synthesise path is indirectly serialised: /synthesise_worklog itself
    does NOT hold the semaphore (agno calls /v1/chat/completions internally),
    so /v1/chat/completions acquires it instead — no nested acquisition,
    no deadlock.
    """
    import asyncio
    sem = _app_state.get("model_sem")
    if sem is None:  # fallback if called before lifespan (e.g. tests)
        sem = asyncio.Semaphore(1)
        _app_state["model_sem"] = sem
    return sem


@asynccontextmanager
async def _lifespan(app: FastAPI) -> AsyncIterator[None]:
    import asyncio
    import datetime
    import agents.run_task_linker_mlx as _mlx
    _app_state["mlx_module"] = _mlx
    _app_state["loaded_at"] = datetime.datetime.now(datetime.timezone.utc).isoformat()
    _app_state["model_sem"] = asyncio.Semaphore(1)
    from agents.llm_selector import APPLE_INTELLIGENCE_ID
    evictor: "asyncio.Task | None" = None
    if _mlx._resolve_model_id() == APPLE_INTELLIGENCE_ID:
        log.info("server: Apple Intelligence backend — no MLX model to load")
    elif _mlx._IDLE_EVICT_S > 0:
        # Lazy: the ~7 GB model loads on the first inference and is evicted after
        # MLX_IDLE_EVICT_S of inactivity, so the server idles light (~0.4 GB)
        # instead of pinning ~7 GB of Metal memory for the whole process life.
        log.info(
            "server: MLX model loads on first request; idle-evict after %.0fs",
            _mlx._IDLE_EVICT_S,
        )
        evictor = asyncio.create_task(_idle_evictor(_mlx))
    else:
        # Eviction disabled — don't spawn a no-op evictor task just to cancel it.
        log.info("server: MLX model loads on first request; idle-eviction disabled (MLX_IDLE_EVICT_S=0)")
    try:
        yield
    finally:
        if evictor is not None:
            import contextlib
            evictor.cancel()
            with contextlib.suppress(asyncio.CancelledError):
                await evictor


app = FastAPI(title="Meridian Agent", version="1.0.0", lifespan=_lifespan)


# ---------------------------------------------------------------------------
# Shared
# ---------------------------------------------------------------------------

@app.get("/health")
async def health() -> dict:
    return {
        "status": "ok",
        "backend": "mlx",
        "db": str(_DB_PATH),
        "db_exists": _DB_PATH.exists(),
    }


@app.get("/info")
async def info() -> dict:
    """Return the identity of the model and its live memory state.

    `active_memory_gb` reads `mx.get_active_memory()` — the ONLY honest measure
    of the model's footprint, since Metal unified memory is invisible to `ps`
    and Activity Monitor (they undercount the model by ~6.5 GB).
    """
    m = _app_state.get("mlx_module")
    return {
        "backend":          "mlx",
        "model_id":         m._resolve_model_id() if m else None,
        "loaded_at":        _app_state.get("loaded_at"),
        "model_resident":   m.model_resident() if m else False,
        "active_memory_gb": m.model_active_memory_gb() if m else None,
    }


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

    import json as _json
    import time as _time
    from fastapi.concurrency import run_in_threadpool
    from outlines.inputs import Chat
    from mlx_lm.sample_utils import make_sampler

    from agents.llm_selector import APPLE_INTELLIGENCE_ID

    m = _app_state["mlx_module"]
    messages = [
        {"role": "system", "content": m._SYSTEM_PROMPT},
        {"role": "user",   "content": req.input},
    ]
    t0 = _time.time()

    def _run_classify() -> "m.SessionClassification":
        if m._resolve_model_id() == APPLE_INTELLIGENCE_ID:
            # _classify_apple_fm uses asyncio.new_event_loop() internally;
            # must run in a thread (no existing loop) not in the async handler.
            return m._classify_apple_fm(messages)
        # Use the SAME inference core as the production classify_session path:
        # the FSM logits-processor is compiled once and cached (~6 s/call saved)
        # and _generate_constrained reuses the static system+skill prefix's KV
        # cache across sessions. The previous naive model(Chat, output_type=...)
        # call rebuilt the FSM and re-prefilled the whole prompt every request,
        # making /classify ~3x slower than production for identical output.
        with m.model_session() as model:
            sampler = make_sampler(temp=m._TEMPERATURE)
            try:
                logits_processors = m._get_constrained_logits_processors(model)
                full_ids = m._get_tokenizer().apply_chat_template(
                    messages, tokenize=True, add_generation_prompt=True
                )
                raw, _gen_stats, _hit = m._generate_constrained(
                    model, full_ids, logits_processors, sampler
                )
            except Exception as stream_exc:  # noqa: BLE001
                # Mirror classify_session's fallback: if outlines' internals shift,
                # drop the shared prefix cache and fall back to the high-level call
                # so classification never breaks just because the fast path did.
                log.warning(
                    "classify: stream-stats path failed (%s) — falling back to "
                    "model(...) without prefix cache",
                    stream_exc,
                )
                with m._prompt_cache_lock:
                    m._invalidate_prompt_cache()
                raw = model(
                    Chat(messages),
                    output_type=m.SessionClassification,
                    max_tokens=m._MAX_TOKENS,
                    sampler=sampler,
                    verbose=False,
                )
        return m.SessionClassification.model_validate_json(raw)

    try:
        result = await run_in_threadpool(_run_classify)
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
    """Classify one or more sessions by ID using the pre-loaded MLX model."""
    from fastapi.concurrency import run_in_threadpool

    m = _app_state.get("mlx_module")
    if m is None:
        raise HTTPException(status_code=503, detail="MLX model is still loading")

    fh = _get_classify_log()
    tracer = _app_state.get("tracer") or trace.get_tracer("meridian-agent-server-mlx")

    parent_ctx = observability.extract_parent_context(req.traceparent)

    # No batch-wrapper span: each session emits a single `classify_session` span
    # attached directly to the Rust caller's context (via the propagated
    # traceparent). This keeps the debug trace minimal — one self-describing span
    # per session with no redundant N=1 wrapper. For N>1, the sessions appear as
    # sibling classify_session spans under the same daemon trace.
    def _classify_all() -> list[dict]:
        _tok = _otel_context.attach(parent_ctx) if parent_ctx is not None else None
        try:
            # Always use the server's own _DB_PATH — ignoring req.meridian_db avoids
            # path-traversal: the server knows its DB from the environment.
            con = _sqlite3.connect(str(_DB_PATH), check_same_thread=False)
            con.row_factory = _sqlite3.Row
            try:
                results: list[dict] = []
                for sid in req.session_ids:
                    # _classify_one_logged owns this span's attributes (session_id,
                    # task_key, confidence, is_error, …) via _annotate_classification_span
                    # and emits db_fetch / build_prompt / llm_inference / parse_response
                    # as its children — one source of truth, matching the CLI path.
                    with tracer.start_as_current_span("classify_session") as cs_span:
                        result = m._classify_one_logged(sid, con, fh)
                        # Stamp THIS classification span's W3C traceparent onto the
                        # result so Rust can persist it (app_sessions.classify_traceparent)
                        # and later link a worklog_draft span back to this exact span —
                        # the worklog→session→classification backtrack in OpenObserve.
                        _csc = cs_span.get_span_context()
                        if _csc.is_valid:
                            result["classify_traceparent"] = (
                                f"00-{_csc.trace_id:032x}-{_csc.span_id:016x}-"
                                f"{int(_csc.trace_flags):02x}"
                            )
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
            if _tok is not None:
                _otel_context.detach(_tok)

    async with _model_sem():
        results = await run_in_threadpool(_classify_all)
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

    # Honour OpenAI `response_format: {"type":"json_schema", ...}` by
    # FSM-constraining decoding to that schema via outlines. Without this, a
    # reasoning model is free to emit chain-of-thought prose instead of the JSON
    # the caller (e.g. agno's structured-output path) expects, and the parse
    # fails. `{"type":"json_object"}` carries no schema, so it stays free-form.
    output_type = None
    rf = req.response_format
    if isinstance(rf, dict) and rf.get("type") == "json_schema":
        schema = (rf.get("json_schema") or {}).get("schema")
        if schema:
            from outlines.types import JsonSchema
            output_type = JsonSchema(schema)

    from agents.llm_selector import APPLE_INTELLIGENCE_ID

    # A `json_schema` request cannot be honoured on Apple Foundation Models:
    # outlines FSM-constrained decoding is incompatible with FM, so the schema
    # would be silently dropped and a structured-output caller (e.g. agno) would
    # get free-form text that fails to parse downstream. Reject explicitly with a
    # 4xx rather than emit unconstrained output that breaks later.
    if output_type is not None and m._resolve_model_id() == APPLE_INTELLIGENCE_ID:
        raise HTTPException(
            status_code=400,
            detail="response_format=json_schema is not supported on Apple "
            "Foundation Models (no FSM-constrained decoding available)",
        )

    def _generate() -> str:
        if m._resolve_model_id() == APPLE_INTELLIGENCE_ID:
            # outlines FSM decoding is incompatible with Foundation Models;
            # Apple FM falls back to free-form (json_object / no schema only).
            return _infer_apple_fm(msgs, max_tokens)
        with m.model_session() as model:
            return model(
                Chat(msgs),
                output_type=output_type,
                max_tokens=max_tokens,
                sampler=make_sampler(temp=temperature),
                verbose=False,
            )

    t0 = _time.time()
    try:
        async with _model_sem():
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
        with m.model_session() as model:
            return model(
                Chat(messages),
                output_type=_SummarySchema,
                max_tokens=req.max_tokens,
                sampler=make_sampler(temp=req.temperature),
                verbose=False,
            )

    try:
        async with _model_sem():
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
    # W3C traceparent propagated from the Rust worklog-draft span. When present,
    # this synth becomes its OWN trace linked back to the daemon's draft trace
    # (same pattern as /classify_sessions) — so it shows as one clean trace.
    traceparent: str | None = None


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


def _set_token_metrics(span: Any, metrics: Any) -> None:
    """Set OTel GenAI token/duration attributes from an agno metrics object,
    coercing DEFENSIVELY: a None/missing/non-numeric field is skipped, never
    raised. These run AFTER a successful `agent.run`, so a bare `int(metrics.x)`
    on a truthy-non-int value (a string/list, depending on agno/model-server
    version) would turn a successful synth into a 500 and lose the draft. Telemetry
    must never do that.
    """
    if metrics is None:
        return

    def _num(name: str) -> "int | float | None":
        v = getattr(metrics, name, None)
        # bool is an int subclass — exclude it so a stray True/False is dropped.
        return v if isinstance(v, (int, float)) and not isinstance(v, bool) else None

    _in, _out = _num("input_tokens"), _num("output_tokens")
    _tot, _dur = _num("total_tokens"), _num("duration")
    if _in is not None:
        span.set_attribute("gen_ai.usage.input_tokens", int(_in))
    if _out is not None:
        span.set_attribute("gen_ai.usage.output_tokens", int(_out))
    if _tot is not None:
        span.set_attribute("gen_ai.usage.total_tokens", int(_tot))
    if _dur is not None:
        span.set_attribute("duration_s", round(float(_dur), 2))


def _record_wire_prompt(tracer: Any, in_span: Any, response: Any, user_message: str) -> None:
    """Populate the worklog_input span with the EXACT messages agno sent to the
    model (system + user), plus a clickable child span per role — mirroring the
    classifier's classifier_input. Faithful to "what actually went in": when agno
    does not expose `response.messages`, it sets `wire_prompt_captured=False` and
    keeps just the rendered user message rather than pretending it had the system
    prompt. The assistant turn is skipped here — that is the OUTPUT, captured by
    worklog_output.
    """
    import json
    from opentelemetry.trace import set_span_in_context

    wire = getattr(response, "messages", None) if response is not None else None
    system_text: str | None = None
    parts: list[str] = []
    if wire:
        for msg in wire:
            role = getattr(msg, "role", "") or ""
            if role == "assistant":
                continue
            content = getattr(msg, "content", "")
            text = content if isinstance(content, str) else json.dumps(content, default=str)
            if role == "system":
                system_text = text
            parts.append(f"<<{role}>>\n{text}")

    captured = bool(parts)
    full_input = "\n\n".join(parts) if captured else user_message
    in_span.set_attribute("llm_input", full_input)
    in_span.set_attribute("full_input_chars", len(full_input))
    in_span.set_attribute("wire_prompt_captured", captured)
    if system_text is not None:
        in_span.set_attribute("system_prompt_chars", len(system_text))

    in_ctx = set_span_in_context(in_span)

    def _part(name: str, text: str) -> None:
        with tracer.start_as_current_span(name, context=in_ctx) as part:
            part.set_attribute("llm_input", text)
            part.set_attribute("chars", len(text))

    if system_text is not None:
        _part("system_prompt", system_text)
    _part("user_message", user_message)


@app.post("/synthesise_worklog")
async def synthesise_worklog(req: _SynthWorklogRequest) -> dict:
    """Synthesise ONE Jira worklog from a collected session bundle.

    Returns a JiraUpdate dict. The authoritative scalar fields (task_key,
    window, cycle_index, time_spent_seconds) are stamped from the bundle so the
    LLM can never override them — it only authors the prose + evidence bullets.
    Rust grounds, routes, and posts.
    """
    from fastapi.concurrency import run_in_threadpool

    from agents.pm_worklog_update import workflow as pm_workflow
    from agents.pm_worklog_update.models import JiraUpdate, SessionBundle

    try:
        bundle = SessionBundle.model_validate(req.bundle)
    except Exception as exc:  # noqa: BLE001
        raise HTTPException(status_code=422, detail=f"bad bundle: {exc}") from exc

    agent = _get_synth_agent()
    user_message = pm_workflow._render_workflow_input(bundle)

    tracer = _app_state.get("tracer") or trace.get_tracer("meridian-agent-server-mlx")
    # CONTINUE the daemon's worklog_draft trace: parent the synth spans to the
    # worklog_draft span propagated via traceparent, so the LLM input/output nests
    # directly INSIDE that one trace — the whole worklog (contributing sessions +
    # detailed LLM input + output) reads as a single tree, exactly the way
    # classifier_input / llm_inference / classifier_output sit under classify_session.
    # Falls back to a fresh root span only when called without a parent (manual curl).
    _daemon_ctx = observability.extract_parent_context(req.traceparent)

    def _synthesise() -> dict:
        with tracer.start_as_current_span(
            "synthesise_worklog",
            context=_daemon_ctx if _daemon_ctx is not None else _otel_context.Context(),
            attributes={"task_key": bundle.task_key},
        ) as root:
            root.set_attribute("window_start", bundle.window_start)
            root.set_attribute("window_end", bundle.window_end)
            # Default to is_error=True and flip to False only on full success
            # below. The pm-worklog dashboard buckets on is_error in
            # ('true','false'), so an UNSET value (left by an unexpected raise in
            # agno internals / _coerce_jira / _record_wire_prompt) would vanish
            # from BOTH the Failed count and the Avg-confidence panel. Pessimistic
            # default guarantees every escaped error is counted.
            root.set_attribute("is_error", True)

            # ── worklog_input ─ created up-front so it leads the waterfall. The
            # EXACT wire prompt (system + user, as agno actually composes and
            # sends it) is filled in AFTER the run from response.messages — the
            # only place agno exposes the composed messages — mirroring the
            # classifier's classifier_input. start_span (not current) keeps it a
            # sibling of synth_inference, not its parent.
            in_span = tracer.start_span("worklog_input")
            response_obj = None
            update = None
            last_detail = "no attempt"
            metrics = None
            model_name = None
            attempts = 0
            try:
                in_span.set_attribute("prompt_chars", len(user_message))
                in_span.set_attribute("session_count", len(bundle.sessions))
                in_span.set_attribute("real_seconds", bundle.real_seconds)
                in_span.set_attribute("total_seconds", bundle.total_seconds)
                in_span.set_attribute("pm_task_status", bundle.pm_task_status or "-")
                in_span.set_attribute("pm_task_is_terminal", bundle.pm_task_is_terminal)
                in_span.set_attribute("is_heavy", bundle.is_heavy)

                # ── synth_inference ─ the agno run (loops back to /v1/chat/completions).
                # Retry a few times: the local model occasionally emits unparseable JSON.
                with tracer.start_as_current_span("synth_inference") as inf:
                    inf.set_attribute("gen_ai.operation.name", "chat")
                    inf.set_attribute("gen_ai.provider.name", "mlx")
                    for attempt in range(1, 4):
                        attempts = attempt
                        try:
                            response = agent.run(input=user_message)
                        except Exception as exc:  # noqa: BLE001 — never crash the shared server
                            last_detail = f"agent run raised {type(exc).__name__}: {exc}"
                            log.warning("synthesise_worklog: attempt %d %s", attempt, last_detail)
                            if attempt < 3:
                                import time as _t
                                _t.sleep(5 * attempt)  # 5s, 10s between retries
                            continue
                        raw = getattr(response, "content", response)
                        if raw is None:
                            last_detail = "agent returned no content (guardrail likely blocked the run)"
                            log.warning("synthesise_worklog: attempt %d %s", attempt, last_detail)
                            continue
                        update = pm_workflow._coerce_jira(raw)
                        if update is not None:
                            response_obj = response
                            metrics = getattr(response, "metrics", None)
                            model_name = getattr(response, "model", None)
                            break
                        last_detail = f"agent output did not parse into a JiraUpdate (raw type={type(raw).__name__})"
                        log.warning("synthesise_worklog: attempt %d %s | raw=%.200s", attempt, last_detail, str(raw))

                    inf.set_attribute("attempts", attempts)
                    if model_name:
                        inf.set_attribute("gen_ai.request.model", str(model_name))
                        inf.set_attribute("gen_ai.response.model", str(model_name))
                    # agno reports the model's OWN token accounting — surface it under
                    # the OTel GenAI semantic conventions, same as the classifier.
                    _set_token_metrics(inf, metrics)
                    if update is None:
                        inf.set_status(trace.StatusCode.ERROR, last_detail)
                        root.set_attribute("is_error", True)
                        root.set_status(trace.StatusCode.ERROR, last_detail)
                        raise HTTPException(
                            status_code=500,
                            detail=f"synth produced no JiraUpdate after 3 attempts ({last_detail})",
                        )

                # Now that the run succeeded, fill worklog_input with the EXACT
                # composed messages agno sent (system + user) + clickable child parts.
                _record_wire_prompt(tracer, in_span, response_obj, user_message)
            finally:
                in_span.end()

            # Stamp authoritative fields from the bundle — never trust the LLM.
            update = update.model_copy(
                update={
                    "task_key":           bundle.task_key,
                    "window_start":       bundle.window_start,
                    "window_end":         bundle.window_end,
                    "cycle_index":        bundle.cycle_index,
                    "time_spent_seconds": bundle.real_seconds,
                }
            )

            # ── worklog_output ─ the COMPLETE synth output, plus a clickable child
            # per output section (summary / bullets / reasoning / risk flags).
            with tracer.start_as_current_span("worklog_output") as out_span:
                out_span.set_attribute("llm_output", update.model_dump_json())
                out_span.set_attribute("confidence", update.confidence)
                out_span.set_attribute("summary_chars", len(update.summary or ""))
                out_span.set_attribute("bullet_count", len(update.bullets))
                out_span.set_attribute(
                    "risk_flags", ", ".join(f.value for f in update.risk_flags) or "-"
                )

                def _out_part(name: str, text: str, **extra: Any) -> None:
                    with tracer.start_as_current_span(name) as part:
                        part.set_attribute("llm_output", text)
                        part.set_attribute("chars", len(text))
                        for _k, _v in extra.items():
                            part.set_attribute(_k, _v)

                _bullets_text = "\n".join(
                    f"• {b.text}  [evidence: {', '.join(map(str, b.evidence_refs)) or 'none'}]"
                    for b in update.bullets
                )
                _next_steps = "\n".join(f"• {s}" for s in update.next_steps)
                _out_part("summary", update.summary or "")
                _out_part("bullets", _bullets_text, bullet_count=len(update.bullets))
                _out_part("next_steps", _next_steps, count=len(update.next_steps))
                _out_part("reasoning", update.reasoning or "")
                _out_part(
                    "risk_flags",
                    ", ".join(f.value for f in update.risk_flags) or "-",
                    flag_count=len(update.risk_flags),
                )

            root.set_attribute("confidence", update.confidence)
            root.set_attribute("summary_chars", len(update.summary or ""))
            root.set_attribute("bullet_count", len(update.bullets))
            root.set_attribute("time_spent_seconds", update.time_spent_seconds)
            root.set_attribute("session_count", len(bundle.sessions))
            root.set_attribute("is_error", False)
            # Promote the model's token accounting + latency onto the root so one
            # dashboard row per draft carries them without joining the child span.
            _set_token_metrics(root, metrics)

            log.info(
                "synthesise_worklog: task=%s sessions=%d summary_chars=%d bullets=%d conf=%.2f attempts=%d",
                bundle.task_key, len(bundle.sessions), len(update.summary or ""),
                len(update.bullets), update.confidence, attempts,
            )
            return update.model_dump()

    return await run_in_threadpool(_synthesise)


@app.get("/v1/models")
async def openai_models_list() -> dict:
    """OpenAI-style models listing — agno/openai-python probe this on first use."""
    model_id = "qwen3.5-9b-instruct"
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
    args = parser.parse_args()

    _app_state["backend"] = "mlx"
    tracer = observability.setup("meridian-agent-server-mlx")
    _app_state["tracer"] = tracer

    log.info("meridian agent server (mlx) on http://%s:%d", args.host, args.port)
    uvicorn.run(app, host=args.host, port=args.port, log_level="warning")


if __name__ == "__main__":
    main()
