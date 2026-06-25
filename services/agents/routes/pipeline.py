"""Pipeline routes — activity_report, distill_hour, rerank, worklog_hour.

These endpoints form the worklog pipeline. Both distill_hour and rerank evict
the resident generative model before loading their own model (embedder and
0.6B reranker respectively), so the single-slot guarantee is maintained — only
one model is ever resident. All endpoints are serialised via model_sem()
alongside all LLM work. worklog_hour orchestrates the others via loopback HTTP
and must NOT hold the semaphore itself.

NOTE: this is the first-pass coarse split (activity + distill + rerank +
worklog in one module), superseded by routes/activity.py, routes/distill.py,
routes/rerank.py, and routes/worklog.py. It is not wired into agents.server.
Kept for parity with the original recovery.
"""
from __future__ import annotations

import logging
import time
from pathlib import Path

from opentelemetry.context import context as _otel_context
from fastapi import APIRouter, HTTPException, Request
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
    report:        str
    input_tokens:  int
    output_tokens: int
    think_tokens:  int
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

        if m is not None:
            m.evict_resident_model()
        try:
            db_path = Path(req.db_path).expanduser() if req.db_path else None
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


@router.post("/rerank", response_model=_RerankResponse)
async def rerank_endpoint(req: _RerankRequest) -> _RerankResponse:
    """Score candidate tickets against the query with Qwen3-Reranker-0.6B.

    HINT ONLY for the matching LLM. Evicts the generative model, loads the
    reranker, scores, unloads — one model resident at a time.
    """
    from fastapi.concurrency import run_in_threadpool

    tracer = app_state.get("tracer") or trace.get_tracer("meridian-agent-server-mlx")
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
            async with model_sem():
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

    self_url = app_state.get("self_url") or str(request.base_url).rstrip("/")
    tracer = app_state.get("tracer") or trace.get_tracer("meridian-agent-server-mlx")
    _daemon_ctx = observability.extract_parent_context(req.traceparent)

    def _run(child_traceparent: str | None) -> dict:
        from agents.worklog_pipeline.workflow import run_hour_workflow

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
        span.set_attribute("workflow_id", "worklog-hour")
        span.set_attribute("agent_id", "meridian-worklog-pipeline")
        span.set_attribute("session_id", f"wl-{req.hour[:10]}")
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
