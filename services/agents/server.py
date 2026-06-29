# ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
"""MLX agent server (FastAPI).

Usage:
    python -m agents.server           # port 7823
    python -m agents.server --port 7824

Endpoints:
    GET  /health
    GET  /info
    POST /prefetch_model
    GET  /prefetch_status
    POST /v1/chat/completions
    GET  /v1/models
    POST /summarise
    POST /activity_report
    POST /distill_hour
    POST /rerank
    POST /worklog_hour
"""
from __future__ import annotations

import argparse
import logging
from contextlib import asynccontextmanager
from typing import Any, AsyncIterator

from fastapi import FastAPI
from opentelemetry import trace

from agents import observability
from agents._state import app_state
from agents.routes import (
    activity,
    chat,
    distill,
    health,
    prefetch,
    rerank,
    summarise,
    worklog,
)

log = logging.getLogger("agents.server")


# ---------------------------------------------------------------------------
# Lifespan — model loaded lazily, evicted when idle
# ---------------------------------------------------------------------------


async def _idle_evictor(mlx_module: Any) -> None:
    """Background loop: evict the MLX model after it has been idle long enough.

    Runs the (briefly blocking) eviction in a threadpool so it never stalls the
    event loop, and never raises out — the evictor must outlive transient errors.
    """
    import asyncio
    from fastapi.concurrency import run_in_threadpool

    ttl = mlx_module._IDLE_EVICT_S
    if ttl <= 0:
        return
    interval = max(15.0, ttl / 4.0)   # check ~4× per idle window
    while True:
        await asyncio.sleep(interval)
        try:
            await run_in_threadpool(mlx_module.maybe_evict_idle)
        except Exception as exc:       # noqa: BLE001 — evictor must never die
            log.warning("server: idle-evictor error: %s", exc)


@asynccontextmanager
async def _lifespan(app: FastAPI) -> AsyncIterator[None]:
    import asyncio
    import datetime
    import agents.mlx_classifier as _mlx
    app_state["mlx_module"] = _mlx
    app_state["loaded_at"] = datetime.datetime.now(datetime.timezone.utc).isoformat()
    app_state["model_sem"] = asyncio.Semaphore(1)

    # Ensure the global TracerProvider + agno instrumentation are live regardless
    # of how the server was launched. Under `uvicorn --reload` (dev) __main__ never
    # runs, so this is the only place that guarantees the provider exists and agno's
    # Agent/Workflow runs export OpenInference spans to OpenObserve.
    observability.setup("meridian-mlx-server")
    app_state.setdefault("tracer", trace.get_tracer("meridian-mlx-server"))
    # OpenObserve export is off for now (MERIDIAN_OO_EXPORT unset) → meridian's
    # manual spans are non-recording. agno's native spans go to the agno trace
    # DB (read by agno_viewer.py) via an explicit, non-global provider so the
    # AgentOS dashboard shows agno's tracing output alone — no custom spans.
    app_state["agno_tracer_provider"] = observability.setup_agno_tracing()
    evictor: "asyncio.Task | None" = None
    if _mlx._IDLE_EVICT_S > 0:
        # Lazy: the ~7 GB model loads on the first inference and is evicted after
        # MLX_IDLE_EVICT_S of inactivity, so the server idles light (~0.4 GB)
        # instead of pinning ~7 GB of Metal memory for the whole process life.
        log.info(
            "server: MLX model loads on first request; idle-evict after %.0fs",
            _mlx._IDLE_EVICT_S,
        )
        evictor = asyncio.create_task(_idle_evictor(_mlx))
    else:
        # Eviction disabled — don't spawn a no-op evictor task just to cancel it.
        log.info("server: MLX model loads on first request; idle-eviction disabled (MLX_IDLE_EVICT_S=0)")
    try:
        yield
    finally:
        if evictor is not None:
            import contextlib
            evictor.cancel()
            with contextlib.suppress(asyncio.CancelledError):
                await evictor


app = FastAPI(title="Meridian Agent", version="1.0.0", lifespan=_lifespan)

app.include_router(health.router)
app.include_router(prefetch.router)
app.include_router(chat.router)
app.include_router(summarise.router)
app.include_router(activity.router)
app.include_router(distill.router)
app.include_router(rerank.router)
app.include_router(worklog.router)


# ---------------------------------------------------------------------------
# Entry point
# ---------------------------------------------------------------------------

def main() -> None:
    import uvicorn

    parser = argparse.ArgumentParser(description="Meridian agent server")
    parser.add_argument("--port",    type=int, default=7823)
    parser.add_argument("--host",    default="127.0.0.1")
    args = parser.parse_args()

    app_state["backend"] = "mlx"
    # Loopback URL for endpoints that orchestrate other endpoints on this same
    # server (e.g. /worklog_hour calling /distill_hour, /activity_report, ...).
    app_state["self_url"] = f"http://{args.host}:{args.port}"
    tracer = observability.setup("meridian-mlx-server")
    app_state["tracer"] = tracer

    log.info("meridian agent server (mlx) on http://%s:%d", args.host, args.port)
    uvicorn.run(app, host=args.host, port=args.port, log_level="warning")


if __name__ == "__main__":
    main()
