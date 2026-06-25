"""Summarise route — /summarise.

Runs a coding-session transcript through the MLX model in thinking mode,
strips the <think> block, and returns clean prose suitable for storage as the
session_summary column.
"""
from __future__ import annotations

import logging

from fastapi import APIRouter, HTTPException
from pydantic import BaseModel, Field

from agents._state import app_state, model_sem

log = logging.getLogger("agents.server")

router = APIRouter()

_MAX_INPUT_CHARS = 500_000
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
