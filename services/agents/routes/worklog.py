"""Worklog hour route — /worklog_hour.

Entry point for the full agno worklog pipeline: distil → activity report →
rerank → match → draft/propose → persist. Orchestrates the other endpoints on
this server via loopback HTTP, so it must NOT hold model_sem — each sub-call
acquires it independently.
"""
from __future__ import annotations

import logging
import time

from opentelemetry.context import context as _otel_context
from fastapi import APIRouter, HTTPException, Request
from opentelemetry import trace
from pydantic import BaseModel

from agents import observability
from agents._state import app_state

log = logging.getLogger("agents.server")

router = APIRouter()


class _WorklogHourRequest(BaseModel):
    hour:        str                    # 'YYYY-MM-DDTHH'
    db_path:     str
    cycle_index: int | None = None
    dry_run:     bool = False
    traceparent: str | None = None


@router.post("/worklog_hour")
async def worklog_hour(req: _WorklogHourRequest, request: Request) -> dict:
    """Run the worklog pipeline for one hour; returns the HourResult dict."""
    from fastapi.concurrency import run_in_threadpool

    # Loopback URL for the sub-calls (/distill_hour, /rerank, /v1, ...) — derive
    # it from the incoming request so it's correct no matter how the server was
    # started (uvicorn --reload in dev never runs __main__, where self_url is set).
    self_url = app_state.get("self_url") or str(request.base_url).rstrip("/")
    tracer = app_state.get("tracer") or trace.get_tracer("meridian-agent-server-mlx")
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
        _elapsed = round(time.monotonic() - _t0, 2)
        span.set_attribute("wl_note", result.get("note", ""))
        span.set_attribute("wl_elapsed_s", _elapsed)
        span.set_attribute("is_error", False)
        # Log INSIDE the span so trace_id is populated in OO.
        log.info(
            "worklog_hour: hour=%s nsess=%d tier=%d matched=%d proposed=%s elapsed=%.1fs",
            req.hour, result.get("nsess", 0), result.get("tier_used", 0),
            len(result.get("matched", [])), result.get("proposed") is not None, _elapsed,
            extra={
                "wl_hour":        req.hour,
                "wl_nsess":       result.get("nsess", 0),
                "wl_tier":        result.get("tier_used", 0),
                "wl_n_matched":   len(result.get("matched", [])),
                "wl_proposed":    1 if result.get("proposed") else 0,
                "wl_worklog_ids": ",".join(str(i) for i in result.get("worklog_ids", [])),
                "wl_proposed_id": result.get("proposed_id") or 0,
                "wl_elapsed_s":   _elapsed,
            })
    return result
