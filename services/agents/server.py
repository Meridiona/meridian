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
import time
from contextlib import asynccontextmanager
from pathlib import Path
from typing import Any, AsyncIterator

import opentelemetry.context as _otel_context
from fastapi import FastAPI, HTTPException, Request
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
    that concurrent requests never compete on the GPU.
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
    import agents.mlx_classifier as _mlx
    _app_state["mlx_module"] = _mlx
    _app_state["loaded_at"] = datetime.datetime.now(datetime.timezone.utc).isoformat()
    _app_state["model_sem"] = asyncio.Semaphore(1)

    # Ensure the global TracerProvider + agno instrumentation are live regardless
    # of how the server was launched. Under `uvicorn --reload` (dev) __main__ never
    # runs, so this is the only place that guarantees the provider exists and agno's
    # Agent/Workflow runs export OpenInference spans to OpenObserve.
    observability.setup("meridian-agent-server-mlx")
    _app_state.setdefault("tracer", trace.get_tracer("meridian-agent-server-mlx"))
    observability.instrument_agno()
    evictor: "asyncio.Task | None" = None
    if _mlx._IDLE_EVICT_S > 0:
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


@app.post("/classify", response_model=ClassifyResponse)
async def classify(req: ClassifyRequest) -> ClassifyResponse:

    import json as _json
    import time as _time
    from fastapi.concurrency import run_in_threadpool
    from outlines.inputs import Chat
    from mlx_lm.sample_utils import make_sampler

    m = _app_state["mlx_module"]
    messages = [
        {"role": "system", "content": m._SYSTEM_PROMPT},
        {"role": "user",   "content": req.input},
    ]
    t0 = _time.time()

    def _run_classify() -> "m.SessionClassification":
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

    return response


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

    def _generate() -> str:
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

    # decoding mode: 'outlines_fsm' when a json_schema was supplied (token-level
    # grammar-constrained), else 'free_form'. Makes the structured-output path
    # observable rather than assumed.
    decode_mode = "outlines_fsm" if output_type is not None else "free_form"
    log.info(
        "openai_chat_completions: msgs=%d max_tokens=%d temp=%.2f decode=%s elapsed=%.2fs out_chars=%d",
        len(msgs), max_tokens, temperature, decode_mode, elapsed, len(text),
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


_SUMMARISE_MAX_TOKENS = 16384   # budget for <think> block + prose answer

from agents.prompts.coding_agent_session_summary import SUMMARY_RULES as _SUMMARISE_DEFAULT_SYSTEM


class _SummariseRequest(BaseModel):
    transcript: str = Field(..., max_length=_MAX_INPUT_CHARS)
    system: str | None = None
    max_tokens: int = Field(_SUMMARISE_MAX_TOKENS, ge=1, le=32768)


class _SummariseResponse(BaseModel):
    summary: str
    input_tokens:  int = 0
    output_tokens: int = 0
    think_tokens:  int = 0
    elapsed_s:     float = 0.0


@app.post("/summarise", response_model=_SummariseResponse)
async def summarise(req: _SummariseRequest) -> _SummariseResponse:
    """Thinking-mode coding session summary (MLX backend only).

    Uses Qwen3.5-2B with enable_thinking=True, temp=1.0 (Qwen3 thinking default),
    and a light repetition_penalty=1.05 — no presence penalty so file names and
    commands can repeat naturally in factual prose. The <think> block is stripped;
    the remaining prose is stored as the summary.
    """
    from fastapi.concurrency import run_in_threadpool
    import time as _time

    m = _app_state.get("mlx_module")
    if m is None:
        raise HTTPException(status_code=503, detail="MLX model is still loading")

    messages = [
        {"role": "system", "content": req.system or _SUMMARISE_DEFAULT_SYSTEM},
        {"role": "user",   "content": req.transcript},
    ]
    t_start = _time.time()

    def _generate() -> tuple[str, int, int, int]:
        """Returns (summary, input_tokens, output_tokens, think_tokens).

        Uses enable_thinking=True (same as /activity_report). The <think> block
        is stripped — the remaining prose is stored directly as the summary.
        """
        from mlx_lm import generate
        from mlx_lm.sample_utils import make_sampler, make_logits_processors

        # Qwen3 thinking-mode defaults: temp=1.0 required (lower suppresses
        # reasoning quality). No presence_penalty — factual summaries must freely
        # repeat file names, commands, and identifiers from the transcript.
        sampler = make_sampler(temp=1.0, top_p=0.95, top_k=20)
        logits_processors = make_logits_processors(
            repetition_penalty=1.05,
            repetition_context_size=64,
        )
        hf_tokenizer = m._get_tokenizer()
        prompt_ids = hf_tokenizer.apply_chat_template(
            messages,
            add_generation_prompt=True,
            enable_thinking=True,
        )
        if hasattr(prompt_ids, "keys") and "input_ids" in prompt_ids:
            prompt_ids = prompt_ids["input_ids"]
        input_tokens = len(prompt_ids)

        with m.model_session() as model:
            raw = generate(
                model.model, hf_tokenizer,
                prompt=prompt_ids,
                max_tokens=req.max_tokens,
                sampler=sampler,
                logits_processors=logits_processors,
                verbose=False,
            )

        think_tokens = 0
        if "</think>" in raw:
            think_part, raw = raw.split("</think>", 1)
            think_tokens = len(hf_tokenizer.encode(think_part + "</think>"))
            raw = raw.strip()

        output_tokens = len(hf_tokenizer.encode(raw))
        return raw, input_tokens, output_tokens, think_tokens

    try:
        async with _model_sem():
            summary, input_tokens, output_tokens, think_tokens = await run_in_threadpool(_generate)
    except Exception as exc:  # noqa: BLE001
        log.warning("summarise: inference/parse error: %s", exc)
        raise HTTPException(status_code=500, detail=str(exc)) from exc

    elapsed = round(_time.time() - t_start, 2)
    log.info(
        "summarise: in_tok=%d out_tok=%d think_tok=%d elapsed=%.1fs",
        input_tokens, output_tokens, think_tokens, elapsed,
        extra={
            "input_tokens":  input_tokens,
            "output_tokens": output_tokens,
            "think_tokens":  think_tokens,
        },
    )
    return _SummariseResponse(
        summary=summary,
        input_tokens=input_tokens,
        output_tokens=output_tokens,
        think_tokens=think_tokens,
        elapsed_s=elapsed,
    )


# ---------------------------------------------------------------------------
# Activity reporter — human-readable worklog from distilled session body
#
# Takes session_distiller.distil_hour() / distil_range() output and produces
# a free-form markdown worklog entry written for the whole team (PMs, devs,
# stakeholders). Uses Qwen3.5-2B in thinking mode with repetition/presence
# penalties to prevent output loops. Token counts (not char counts) are
# reported for accurate cost tracking.
# ---------------------------------------------------------------------------

_ACTIVITY_REPORT_MAX_TOKENS = 32768   # budget for <think> block + answer


class _ActivityReportRequest(BaseModel):
    body:        str
    label:       str
    max_tokens:  int = _ACTIVITY_REPORT_MAX_TOKENS
    traceparent: str | None = None


class _ActivityReportResponse(BaseModel):
    report:        str    # free-form markdown worklog
    input_tokens:  int
    output_tokens: int
    think_tokens:  int    # tokens spent in <think> block (stripped from report)
    elapsed_s:     float


@app.post("/activity_report", response_model=_ActivityReportResponse)
async def activity_report(req: _ActivityReportRequest) -> _ActivityReportResponse:
    """Human-readable worklog entry from distilled session body.

    Uses Qwen3.5-2B in thinking mode (enable_thinking=True) with
    repetition_penalty=1.1 and presence_penalty=1.5 to prevent output loops.
    The <think> block is stripped — only the final answer is returned.
    Token counts are from the model's own generation response.
    """
    from fastapi.concurrency import run_in_threadpool
    import time as _time

    m = _app_state.get("mlx_module")
    if m is None:
        raise HTTPException(status_code=503, detail="MLX model is still loading")

    _daemon_ctx = observability.extract_parent_context(req.traceparent)
    tracer = _app_state.get("tracer") or trace.get_tracer("meridian-agent-server-mlx")
    t_start = _time.time()

    from agents.prompts.activity_report import SYSTEM as _AR_SYSTEM

    messages = [
        {"role": "system", "content": _AR_SYSTEM},
        {"role": "user",   "content": req.body},
    ]

    def _generate() -> tuple[str, int, int, int]:
        """Returns (report_markdown, input_tokens, output_tokens, think_tokens).

        Uses the server's already-loaded model (model.model is the raw MLX net;
        m._get_tokenizer() is the HF tokenizer with Qwen3.5 chat template).
        enable_thinking=True requires the HF tokenizer's apply_chat_template, not
        outlines' Chat() shim, so we call mlx_lm.generate directly.
        """
        from mlx_lm import generate
        from mlx_lm.sample_utils import make_sampler, make_logits_processors

        sampler = make_sampler(temp=1.0, top_p=0.95, top_k=20)
        logits_processors = make_logits_processors(
            repetition_penalty=1.1,
            repetition_context_size=64,
            presence_penalty=1.5,
        )
        hf_tokenizer = m._get_tokenizer()
        prompt_ids = hf_tokenizer.apply_chat_template(
            messages,
            add_generation_prompt=True,
            enable_thinking=True,
        )
        # A fresh HF tokenizer returns a BatchEncoding ({input_ids, attention_mask});
        # the mlx_lm wrapper returns a plain token list. generate() needs the list.
        if hasattr(prompt_ids, "keys") and "input_ids" in prompt_ids:
            prompt_ids = prompt_ids["input_ids"]
        input_tokens = len(prompt_ids)

        with m.model_session() as model:
            raw = generate(
                model.model, hf_tokenizer,
                prompt=prompt_ids,
                max_tokens=req.max_tokens,
                sampler=sampler,
                logits_processors=logits_processors,
                verbose=False,
            )

        # Strip <think>…</think> block; count its tokens
        think_tokens = 0
        if "</think>" in raw:
            think_part, answer = raw.split("</think>", 1)
            think_tokens = len(hf_tokenizer.encode(think_part + "</think>"))
            raw = answer.strip()

        output_tokens = len(hf_tokenizer.encode(raw))
        return raw, input_tokens, output_tokens, think_tokens

    with tracer.start_as_current_span(
        "activity_report",
        context=_daemon_ctx if _daemon_ctx is not None else _otel_context.Context(),
    ) as span:
        span.set_attribute("distil_label",     req.label)
        span.set_attribute("input_chars",      len(req.body))
        span.set_attribute("model",            m._resolve_model_id())
        span.set_attribute("is_error",         True)
        try:
            async with _model_sem():
                report, input_tokens, output_tokens, think_tokens = await run_in_threadpool(_generate)
        except Exception as exc:  # noqa: BLE001
            span.set_status(trace.StatusCode.ERROR, str(exc))
            log.error(
                "activity_report: inference error label=%s: %s",
                req.label, exc,
                extra={"label": req.label},
            )
            raise HTTPException(status_code=500, detail=str(exc)) from exc

        elapsed = round(_time.time() - t_start, 2)
        span.set_attribute("input_tokens",  input_tokens)
        span.set_attribute("output_tokens", output_tokens)
        span.set_attribute("think_tokens",  think_tokens)
        span.set_attribute("output_chars",  len(report))
        span.set_attribute("elapsed_s",     elapsed)
        span.set_attribute("is_error",      False)

    log.info(
        "activity_report: label=%s in_tok=%d out_tok=%d think_tok=%d elapsed=%.1fs",
        req.label, input_tokens, output_tokens, think_tokens, elapsed,
        extra={
            "label":         req.label,
            "input_tokens":  input_tokens,
            "output_tokens": output_tokens,
            "think_tokens":  think_tokens,
        },
    )
    return _ActivityReportResponse(
        report=report,
        input_tokens=input_tokens,
        output_tokens=output_tokens,
        think_tokens=think_tokens,
        elapsed_s=elapsed,
    )


# ---------------------------------------------------------------------------
# Worklog pipeline — single-slot model host endpoints (distill + rerank)
#
# Both evict the resident generative model before loading their own model, so
# the embedder (distill) and reranker (0.6B) never coexist with the 2B. Each
# unloads its model afterwards; the generative model lazily reloads on the next
# /v1 or /activity_report call. Serialised via _model_sem alongside all LLM work.
# ---------------------------------------------------------------------------


class _DistillHourRequest(BaseModel):
    hour:        str            # 'YYYY-MM-DDTHH'
    db_path:     str | None = None
    traceparent: str | None = None


class _DistillHourResponse(BaseModel):
    body:          str
    label:         str
    nsess:         int
    raw_chars:     int
    out_chars:     int
    reduction_pct: float
    elapsed_s:     float


@app.post("/distill_hour", response_model=_DistillHourResponse)
async def distill_hour_endpoint(req: _DistillHourRequest) -> _DistillHourResponse:
    """Distil one hour of app_sessions into a compact body (85-92% reduction).

    Loads the Qwen3-Embedding-0.6B embedder in THIS process after evicting the
    resident generative model, so only one model is ever resident. The embedder
    is evicted again before returning.
    """
    from fastapi.concurrency import run_in_threadpool
    from pathlib import Path

    m = _app_state.get("mlx_module")
    tracer = _app_state.get("tracer") or trace.get_tracer("meridian-agent-server-mlx")
    _daemon_ctx = observability.extract_parent_context(req.traceparent)

    def _run() -> "_DistillHourResponse":
        from agents.session_distiller import distil_hour, evict_embedder

        # Single-slot: free the generative model before the embedder loads.
        if m is not None:
            m.evict_resident_model()
        try:
            db_path = Path(req.db_path).expanduser() if req.db_path else None
            # Coding-agent rows are folded into the worklog activity summary
            # VERBATIM from their session_summary (worklog_pipeline), not via this
            # OCR compressor — so exclude them from the distilled body here.
            body, ds = distil_hour(req.hour, db_path, exclude_coding=True)
        finally:
            evict_embedder()
        return _DistillHourResponse(
            body=body,
            label=ds.hour,
            nsess=ds.nsess,
            raw_chars=ds.raw_chars,
            out_chars=ds.out_chars,
            reduction_pct=ds.reduction_pct,
            elapsed_s=ds.elapsed_s,
        )

    with tracer.start_as_current_span(
        "distill_hour",
        context=_daemon_ctx if _daemon_ctx is not None else _otel_context.Context(),
    ) as span:
        span.set_attribute("hour", req.hour)
        span.set_attribute("is_error", True)
        try:
            async with _model_sem():
                resp = await run_in_threadpool(_run)
        except Exception as exc:  # noqa: BLE001
            span.set_status(trace.StatusCode.ERROR, str(exc))
            log.error("distill_hour: error hour=%s: %s", req.hour, exc,
                      extra={"hour": req.hour})
            raise HTTPException(status_code=500, detail=str(exc)) from exc
        span.set_attribute("nsess", resp.nsess)
        span.set_attribute("out_chars", resp.out_chars)
        span.set_attribute("reduction_pct", resp.reduction_pct)
        span.set_attribute("elapsed_s", resp.elapsed_s)
        span.set_attribute("is_error", False)

    log.info("distill_hour: hour=%s nsess=%d out_chars=%d (%.0f%%)",
             req.hour, resp.nsess, resp.out_chars, resp.reduction_pct,
             extra={"hour": req.hour, "nsess": resp.nsess})
    return resp


class _RerankCandidate(BaseModel):
    task_key: str
    doc:      str          # rendered ticket text (title + epic + description)


class _RerankRequest(BaseModel):
    query:       str               # activity-report / worklog text
    candidates:  list[_RerankCandidate]
    traceparent: str | None = None


class _RerankResponse(BaseModel):
    ranked: list[dict]             # [{"task_key": str, "score": float}], desc


@app.post("/rerank", response_model=_RerankResponse)
async def rerank_endpoint(req: _RerankRequest) -> _RerankResponse:
    """Score candidate tickets against the query with Qwen3-Reranker-0.6B.

    HINT ONLY for the matching LLM. Evicts the generative model, loads the
    reranker, scores, unloads — one model resident at a time.
    """
    from fastapi.concurrency import run_in_threadpool

    tracer = _app_state.get("tracer") or trace.get_tracer("meridian-agent-server-mlx")
    _daemon_ctx = observability.extract_parent_context(req.traceparent)

    def _run() -> list[dict]:
        from agents import reranker

        cands = [{"task_key": c.task_key, "doc": c.doc} for c in req.candidates]
        return reranker.score_candidates(req.query, cands)

    with tracer.start_as_current_span(
        "rerank",
        context=_daemon_ctx if _daemon_ctx is not None else _otel_context.Context(),
    ) as span:
        span.set_attribute("n_candidates", len(req.candidates))
        span.set_attribute("query_chars", len(req.query))
        span.set_attribute("is_error", True)
        try:
            async with _model_sem():
                ranked = await run_in_threadpool(_run)
        except Exception as exc:  # noqa: BLE001
            span.set_status(trace.StatusCode.ERROR, str(exc))
            log.error("rerank: error: %s", exc)
            raise HTTPException(status_code=500, detail=str(exc)) from exc
        span.set_attribute("is_error", False)
        if ranked:
            span.set_attribute("top_key", ranked[0]["task_key"])
            span.set_attribute("top_score", ranked[0]["score"])

    log.info("rerank: n=%d top=%s", len(ranked),
             f"{ranked[0]['task_key']}@{ranked[0]['score']:.3f}" if ranked else "—")
    return _RerankResponse(ranked=ranked)


# ---------------------------------------------------------------------------
# Worklog pipeline — the agno Workflow entry point
#
# Runs the full hour pipeline (distil → report → rerank → match → worklog/propose
# → persist) as an agno Workflow. Orchestrates the other endpoints on THIS server
# (loopback), so it must NOT hold _model_sem — each sub-call acquires it instead.
# ---------------------------------------------------------------------------


class _WorklogHourRequest(BaseModel):
    hour:        str                    # 'YYYY-MM-DDTHH'
    db_path:     str
    cycle_index: int | None = None
    dry_run:     bool = False
    traceparent: str | None = None


@app.post("/worklog_hour")
async def worklog_hour(req: _WorklogHourRequest, request: Request) -> dict:
    """Run the worklog pipeline for one hour; returns the HourResult dict."""
    from fastapi.concurrency import run_in_threadpool

    # Loopback URL for the sub-calls (/distill_hour, /rerank, /v1, ...) — derive
    # it from the incoming request so it's correct no matter how the server was
    # started (uvicorn --reload in dev never runs __main__, where self_url is set).
    self_url = _app_state.get("self_url") or str(request.base_url).rstrip("/")
    tracer = _app_state.get("tracer") or trace.get_tracer("meridian-agent-server-mlx")
    _daemon_ctx = observability.extract_parent_context(req.traceparent)

    def _run(child_traceparent: str | None) -> dict:
        from agents.worklog_pipeline.workflow import run_hour_workflow

        # Stages forward `child_traceparent` (this worklog.hour span), so the
        # loopback distill/report/rerank spans + the in-process agno spans all
        # nest UNDER this root rather than as siblings of the Rust caller.
        result = run_hour_workflow(
            req.hour, db_path=req.db_path, server_url=self_url,
            cycle_index=req.cycle_index, dry_run=req.dry_run,
            run_id=f"wl-{req.hour}", traceparent=child_traceparent,
        )
        return result.as_dict()

    _t0 = time.monotonic()
    with tracer.start_as_current_span(
        "worklog.hour",
        context=_daemon_ctx if _daemon_ctx is not None else _otel_context.Context(),
    ) as span:
        span.set_attribute("wl_hour", req.hour)
        span.set_attribute("wl_cycle_index", req.cycle_index if req.cycle_index is not None else -1)
        span.set_attribute("is_error", True)
        # Link this trace to the agno workflow/agent registered in agno_viewer.py so
        # the DatabaseSpanExporter populates workflow_id/agent_id and the AgentOS
        # Traces tab shows it (the UI filters out traces where both are NULL).
        span.set_attribute("workflow_id", "worklog-hour")
        span.set_attribute("agent_id", "meridian-worklog-pipeline")
        # session_id groups all hours for the same day into one session so the
        # AgentOS Sessions view (group_by=sessions) shows them together.
        span.set_attribute("session_id", f"wl-{req.hour[:10]}")
        # This span's own traceparent — handed to the pipeline as the parent the
        # stages continue, so the whole hour is one connected trace.
        child_tp = observability.current_traceparent()
        try:
            result = await run_in_threadpool(_run, child_tp)
        except Exception as exc:  # noqa: BLE001
            span.set_status(trace.StatusCode.ERROR, str(exc))
            log.error("worklog_hour: error hour=%s: %s", req.hour, exc,
                      extra={"hour": req.hour})
            raise HTTPException(status_code=500, detail=str(exc)) from exc
        matched = result.get("matched", [])
        proposed = result.get("proposed")
        span.set_attribute("wl_nsess", result.get("nsess", 0))
        span.set_attribute("wl_tier_used", result.get("tier_used", 0))
        span.set_attribute("wl_n_matched", len(matched))
        span.set_attribute("wl_matched_keys", ",".join(m.get("task_key", "") for m in matched))
        span.set_attribute("wl_worklog_ids", ",".join(str(i) for i in result.get("worklog_ids", [])))
        span.set_attribute("wl_proposed", proposed is not None)
        span.set_attribute("wl_proposed_title", (proposed or {}).get("title", "") if proposed else "")
        span.set_attribute("wl_proposed_id", result.get("proposed_id") if result.get("proposed_id") is not None else -1)
        span.set_attribute("wl_note", result.get("note", ""))
        span.set_attribute("wl_elapsed_s", round(time.monotonic() - _t0, 2))
        span.set_attribute("is_error", False)

    log.info("worklog_hour: hour=%s nsess=%d tier=%d matched=%d proposed=%s",
             req.hour, result.get("nsess", 0), result.get("tier_used", 0),
             len(result.get("matched", [])), result.get("proposed") is not None,
             extra={"hour": req.hour})
    return result




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
    # Loopback URL for endpoints that orchestrate other endpoints on this same
    # server (e.g. /worklog_hour calling /distill_hour, /activity_report, ...).
    _app_state["self_url"] = f"http://127.0.0.1:{args.port}"
    tracer = observability.setup("meridian-agent-server-mlx")
    _app_state["tracer"] = tracer

    log.info("meridian agent server (mlx) on http://%s:%d", args.host, args.port)
    uvicorn.run(app, host=args.host, port=args.port, log_level="warning")


if __name__ == "__main__":
    main()
