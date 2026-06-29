"""Distill route — /distill_hour.

Distils one hour of app_sessions into a compact body (85-92% reduction) using
the Qwen3-Embedding-0.6B embedder. Evicts the resident generative model before
loading the embedder and evicts the embedder again before returning, so only
one model is ever resident.
"""
from __future__ import annotations

import logging
from pathlib import Path

from opentelemetry.context import context as _otel_context
from fastapi import APIRouter, HTTPException
from opentelemetry import trace
from pydantic import BaseModel

from agents import observability
from agents._state import app_state, model_sem

log = logging.getLogger("agents.server")

router = APIRouter()


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


@router.post("/distill_hour", response_model=_DistillHourResponse)
async def distill_hour_endpoint(req: _DistillHourRequest) -> _DistillHourResponse:
    """Distil one hour of app_sessions into a compact body (85-92% reduction).

    Loads the Qwen3-Embedding-0.6B embedder in THIS process after evicting the
    resident generative model, so only one model is ever resident. The embedder
    is evicted again before returning.
    """
    from fastapi.concurrency import run_in_threadpool

    m = app_state.get("mlx_module")
    tracer = app_state.get("tracer") or trace.get_tracer("meridian-agent-server-mlx")
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
            async with model_sem():
                resp = await run_in_threadpool(_run)
        except Exception as exc:  # noqa: BLE001
            span.set_status(trace.StatusCode.ERROR, str(exc))
            log.error("distill_hour: error hour=%s: %s", req.hour, exc,
                      extra={"hour": req.hour})
            raise HTTPException(status_code=500, detail=str(exc)) from exc
        span.set_attribute("nsess",         resp.nsess)
        span.set_attribute("out_chars",     resp.out_chars)
        span.set_attribute("reduction_pct", resp.reduction_pct)
        span.set_attribute("elapsed_s",     resp.elapsed_s)
        span.set_attribute("distil_output", observability.preview(resp.body, max_chars=4000))
        span.set_attribute("is_error",      False)

    log.info("distill_hour: hour=%s nsess=%d out_chars=%d (%.0f%%)",
             req.hour, resp.nsess, resp.out_chars, resp.reduction_pct,
             extra={"hour": req.hour, "nsess": resp.nsess})
    return resp
