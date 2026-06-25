"""Activity report route — /activity_report.

Takes session_distiller output and produces a human-readable markdown worklog
entry using the MLX model in thinking mode. Written for a non-technical
audience (PMs, devs, stakeholders). Token counts are from the model's own
generation response.
"""
from __future__ import annotations

import logging

from opentelemetry.context import context as _otel_context
from fastapi import APIRouter, HTTPException
from opentelemetry import trace
from pydantic import BaseModel

from agents import observability
from agents._state import app_state, model_sem

log = logging.getLogger("agents.server")

router = APIRouter()

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


@router.post("/activity_report", response_model=_ActivityReportResponse)
async def activity_report(req: _ActivityReportRequest) -> _ActivityReportResponse:
    """Human-readable worklog entry from distilled session body.

    Uses Qwen3.5-2B in thinking mode (enable_thinking=True) with
    repetition_penalty=1.1 and presence_penalty=1.5 to prevent output loops.
    The <think> block is stripped — only the final answer is returned.
    Token counts are from the model's own generation response.
    """
    from fastapi.concurrency import run_in_threadpool
    import time as _time

    m = app_state.get("mlx_module")
    if m is None:
        raise HTTPException(status_code=503, detail="MLX model is still loading")

    _daemon_ctx = observability.extract_parent_context(req.traceparent)
    tracer = app_state.get("tracer") or trace.get_tracer("meridian-agent-server-mlx")
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
        span.set_attribute("model",            m.MODEL_ID)
        span.set_attribute("is_error",         True)
        try:
            async with model_sem():
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
