"""Inference routes — OpenAI-compatible chat completions and session summarisation.

A minimal `/v1/chat/completions` endpoint lets agno, the openai python SDK,
and anything else expecting OpenAI's wire format talk to the already-loaded
MLX model. `/summarise` runs a coding-session transcript through the same
model in thinking mode, stripping the <think> block and returning clean prose.

NOTE: this is the first-pass coarse split (chat + summarise in one module),
superseded by routes/chat.py + routes/summarise.py. It is not wired into
agents.server. Kept for parity with the original recovery.
"""
from __future__ import annotations

import logging
from typing import Any

from fastapi import APIRouter, HTTPException
from pydantic import BaseModel, Field

from agents._state import app_state, model_sem

log = logging.getLogger("agents.server")

router = APIRouter()

_MAX_INPUT_CHARS = 500_000


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


@router.post("/v1/chat/completions")
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

    m = app_state.get("mlx_module")
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
        async with model_sem():
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

    decode_mode = "outlines_fsm" if output_type is not None else "free_form"
    log.info(
        "openai_chat_completions: msgs=%d max_tokens=%d temp=%.2f decode=%s elapsed=%.2fs out_chars=%d",
        len(msgs), max_tokens, temperature, decode_mode, elapsed, len(text),
    )

    return {
        "id":      completion_id,
        "object":  "chat.completion",
        "created": int(_time.time()),
        "model":   req.model or "qwen3.5-2b-instruct",
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


@router.post("/summarise", response_model=_SummariseResponse)
async def summarise(req: _SummariseRequest) -> _SummariseResponse:
    """Thinking-mode coding session summary (MLX backend only).

    Uses Qwen3.5-2B with enable_thinking=True, temp=1.0 (Qwen3 thinking default),
    and a light repetition_penalty=1.05 — no presence penalty so file names and
    commands can repeat naturally in factual prose. The <think> block is stripped;
    the remaining prose is stored as the summary.
    """
    from fastapi.concurrency import run_in_threadpool
    import time as _time

    m = app_state.get("mlx_module")
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
        async with model_sem():
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


@router.get("/v1/models")
async def openai_models_list() -> dict:
    """OpenAI-style models listing — agno/openai-python probe this on first use."""
    model_id = "qwen3.5-2b-instruct"
    m = app_state.get("mlx_module")
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
