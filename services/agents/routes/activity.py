"""Activity report route — /activity_report.

Takes session_distiller output and produces a human-readable markdown worklog
entry using the MLX model in thinking mode with a capped thinking budget.
Written for a non-technical audience (PMs, devs, stakeholders). Token counts
are from the model's own generation response.
"""
from __future__ import annotations

import logging

from fastapi import APIRouter, HTTPException
from opentelemetry import trace
from opentelemetry.context import context as _otel_context
from pydantic import BaseModel

from agents import observability
from agents._state import app_state, model_sem
from agents.thinking import generate_thinking, DEFAULT_PROSE_TEMP, DEFAULT_THINKING_BUDGET

log = logging.getLogger("agents.server")

router = APIRouter()

_ACTIVITY_REPORT_MAX_TOKENS = 16384   # budget for <think> block + answer


class _ActivityReportRequest(BaseModel):
    body:        str    # the full consolidated input (distilled OCR + coding sessions)
    label:       str
    max_tokens:  int = _ACTIVITY_REPORT_MAX_TOKENS
    traceparent: str | None = None
    enable_thinking: bool = True  # set False to disable thinking mode for comparison


class _ActivityReportResponse(BaseModel):
    report:        str    # free-form markdown worklog
    input_tokens:  int
    output_tokens: int
    think_tokens:  int    # tokens spent in <think> block (stripped from report)
    elapsed_s:     float


@router.post("/activity_report", response_model=_ActivityReportResponse)
async def activity_report(req: _ActivityReportRequest) -> _ActivityReportResponse:
    """Human-readable worklog entry from distilled session body.

    Uses the shared agents.thinking.generate_thinking (unified sampling +
    thinking-budget enforcement). The thinking-budget processor caps the <think>
    block so it can't consume the whole max_tokens window; the block is stripped
    and only the final markdown answer is returned. req.enable_thinking=False
    disables reasoning entirely (for comparison). Token counts are the model's own.
    """
    from fastapi.concurrency import run_in_threadpool
    import time as _time

    m = app_state.get("mlx_module")
    if m is None:
        raise HTTPException(status_code=503, detail="MLX model is still loading")

    _parent_ctx = observability.extract_parent_context(req.traceparent)
    tracer = app_state.get("tracer") or trace.get_tracer("meridian-mlx-server")
    t_start = _time.time()

    from agents.prompts.activity_report import SYSTEM as _AR_SYSTEM

    # The caller (worklog_pipeline.stage_report) sends the FULL consolidated input —
    # distilled OCR + labeled coding-agent sessions — so this endpoint just runs it.
    body = req.body
    messages = [
        {"role": "system", "content": _AR_SYSTEM},
        {"role": "user",   "content": body},
    ]

    with tracer.start_as_current_span(
        "activity_report",
        context=_parent_ctx if _parent_ctx is not None else _otel_context.Context(),
    ) as span:
        span.set_attribute("gen_ai.operation.name", "chat")
        span.set_attribute("gen_ai.system",         "mlx")
        span.set_attribute("distil_label", req.label)
        span.set_attribute("input_chars",  len(body))
        span.set_attribute("model",        m.MODEL_ID)
        span.set_attribute("is_error",     True)

        # Span 1: system prompt — llm_input → OO renders as dedicated "Input" panel
        with tracer.start_as_current_span("activity_report.prompt") as prompt_span:
            prompt_span.set_attribute("label",       req.label)
            prompt_span.set_attribute("total_chars", len(_AR_SYSTEM))
            prompt_span.set_attribute("llm_input",   _AR_SYSTEM)
            prompt_span.set_attribute("body",        _AR_SYSTEM)

        # Span 2: user input (distilled text)
        # llm_input → OO dedicated Input panel (preview); body → full text in OO Attributes section
        with tracer.start_as_current_span("activity_report.input") as input_span:
            input_span.set_attribute("label",       req.label)
            input_span.set_attribute("total_chars", len(body))
            input_span.set_attribute("llm_input",   body)
            input_span.set_attribute("body",        body)

        try:
            async with model_sem():
                _res = await run_in_threadpool(
                    generate_thinking, m, messages,
                    max_tokens=req.max_tokens, enable_thinking=req.enable_thinking,
                    json_mode=False, temp=DEFAULT_PROSE_TEMP)
            report = _res.text
            input_tokens, output_tokens, think_tokens = (
                _res.input_tokens, _res.output_tokens, _res.think_tokens)
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
        observability.record_gen_params(
            span, temp=DEFAULT_PROSE_TEMP, max_tokens=req.max_tokens,
            thinking_budget=DEFAULT_THINKING_BUDGET, budget_forced=_res.budget_forced,
            enable_thinking=req.enable_thinking)
        span.set_attribute("is_error",      False)

        # Span 3: model output — llm_output → OO renders as dedicated "Output" panel
        with tracer.start_as_current_span("activity_report.output") as output_span:
            output_span.set_attribute("label",        req.label)
            output_span.set_attribute("total_chars",  len(report))
            output_span.set_attribute("input_tokens", input_tokens)
            output_span.set_attribute("output_tokens",output_tokens)
            output_span.set_attribute("think_tokens", think_tokens)
            output_span.set_attribute("llm_output",   observability.preview(report, max_chars=8000))
        _msg_prefix = "worklog.activity_report" if req.traceparent else "activity_report"
        _extra = {
            "ar_label":        req.label,
            "ar_input_tokens":  input_tokens,
            "ar_output_tokens": output_tokens,
            "ar_think_tokens":  think_tokens,
            "ar_elapsed_s":     elapsed,
        }
        log.info(
            "%s: label=%s in_tok=%d out_tok=%d think_tok=%d elapsed=%.1fs",
            _msg_prefix, req.label, input_tokens, output_tokens, think_tokens, elapsed,
            extra=_extra,
        )
    return _ActivityReportResponse(
        report=report,
        input_tokens=input_tokens,
        output_tokens=output_tokens,
        think_tokens=think_tokens,
        elapsed_s=elapsed,
    )
