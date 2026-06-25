"""Rerank route — /rerank.

Scores candidate tickets against a query using Qwen3-Reranker-0.6B. Evicts
the resident generative model, loads the reranker, scores, and unloads —
maintaining the single-slot model guarantee. Results are hints only; the
matching LLM makes the final decision.
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
