"""Summarise route — /summarise.

Runs a coding-session transcript through the MLX model and returns clean prose
suitable for storage as the session_summary column. Thinking mode is on by
default (the <think> block is stripped); callers under a tight timeout (the
coding-agent summariser fallback) pass enable_thinking=False for the fast path.
"""
from __future__ import annotations

import logging

from fastapi import APIRouter, HTTPException
from pydantic import BaseModel, Field

from agents._state import app_state, model_sem
from agents.thinking import generate_thinking, DEFAULT_PROSE_TEMP

log = logging.getLogger("agents.server")

router = APIRouter()

_MAX_INPUT_CHARS = 500_000
_SUMMARISE_MAX_TOKENS = 16384   # budget for <think> block + prose answer

from agents.prompts.coding_agent_session_summary import SUMMARY_RULES as _SUMMARISE_DEFAULT_SYSTEM


class _SummariseRequest(BaseModel):
    transcript: str = Field(..., max_length=_MAX_INPUT_CHARS)
    system: str | None = None
    max_tokens: int = Field(_SUMMARISE_MAX_TOKENS, ge=1, le=32768)
    # When False, skip the <think> block entirely — the fast non-thinking path
    # the coding-agent summariser fallback uses to fit a tight client timeout.
    enable_thinking: bool = True


class _SummariseResponse(BaseModel):
    summary: str
    input_tokens:  int = 0
    output_tokens: int = 0
    think_tokens:  int = 0
    elapsed_s:     float = 0.0


@router.post("/summarise", response_model=_SummariseResponse)
async def summarise(req: _SummariseRequest) -> _SummariseResponse:
    """Thinking-mode coding session summary (MLX backend only).

    Uses the shared agents.thinking.generate_thinking (unified sampling +
    thinking-budget enforcement, same as every other generative endpoint). The
    <think> block is stripped; the remaining prose is stored as the summary.
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

    try:
        async with model_sem():
            res = await run_in_threadpool(
                generate_thinking, m, messages,
                max_tokens=req.max_tokens, json_mode=False, temp=DEFAULT_PROSE_TEMP,
                enable_thinking=req.enable_thinking)
        summary = res.text
        input_tokens, output_tokens, think_tokens = (
            res.input_tokens, res.output_tokens, res.think_tokens)
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
