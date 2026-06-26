"""OpenAI-compatible chat completions and models listing.

Provides a minimal `/v1/chat/completions` endpoint so agno, the openai python
SDK, and anything else expecting OpenAI's wire format can talk to the loaded
MLX model. `/v1/models` is the companion probe endpoint openai-python calls
on first connection.
"""
from __future__ import annotations

import logging
from typing import Any

from fastapi import APIRouter, HTTPException
from pydantic import BaseModel, Field

from agents._state import app_state, model_sem
from agents.mlx_classifier import MODEL_ID

log = logging.getLogger("agents.server")

router = APIRouter()


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
        from outlines.models import from_mlxlm
        with m.model_session() as bundle:
            om = from_mlxlm(bundle.model, bundle.mlx_tokenizer)
            return om(
                Chat(msgs),
                output_type=output_type,
                max_tokens=max_tokens,
                sampler=make_sampler(temp=temperature),
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
        "model":   MODEL_ID,
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


@router.get("/v1/models")
async def openai_models_list() -> dict:
    """OpenAI-style models listing — agno/openai-python probe this on first use."""
    model_id = "qwen3.5-2b-instruct"
    m = app_state.get("mlx_module")
    if m is not None and hasattr(m, "MODEL_ID"):
        model_id = m.MODEL_ID
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
