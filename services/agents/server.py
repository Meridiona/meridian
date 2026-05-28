# meridian — normalises screenpipe activity into structured app sessions
"""Conversational agent server (FastAPI).

Supports two backends selected via --backend:

  hermes (default) — wraps the hermes AIAgent for conversational classification
  mlx              — loads the MLX model once at startup for direct inference

Usage:
    python -m agents.server                    # hermes backend, port 7823
    python -m agents.server --backend mlx      # MLX backend, port 7823
    python -m agents.server --backend mlx --port 7824

hermes endpoints:
    GET  /health
    POST /chat       {"message": "classify session id 80"}

mlx endpoints:
    GET  /health
    POST /classify   {"input": "<fully-formatted user_message string>"}
                     → {"task_key": ..., "session_type": ..., "reasoning": ...,
                        "confidence": ..., "dimensions": {...}}
"""
from __future__ import annotations

import argparse
import contextlib
import logging
import os
import sqlite3 as _sqlite3
import sys
from contextlib import asynccontextmanager
from pathlib import Path
from typing import Any, AsyncIterator

# Must be set before any hermes import.
_SERVICES_DIR = Path(__file__).parent.parent
os.environ.setdefault("HERMES_HOME", str(_SERVICES_DIR / ".hermes"))

import opentelemetry.context as _otel_context
from fastapi import FastAPI, HTTPException
from opentelemetry import trace
from pydantic import BaseModel

from agents import observability

log = logging.getLogger("agents.server")

_DB_PATH = Path(os.environ.get("MERIDIAN_DB", Path.home() / ".meridian/meridian.db"))

# ---------------------------------------------------------------------------
# Lifespan — model loaded once for MLX backend, no-op for hermes
# ---------------------------------------------------------------------------

_app_state: dict[str, Any] = {}


@asynccontextmanager
async def _lifespan(app: FastAPI) -> AsyncIterator[None]:
    if _app_state.get("backend") == "mlx":
        log.info("server: loading MLX model at startup…")
        import agents.run_task_linker_mlx as _mlx
        _mlx._get_model()
        _app_state["mlx_module"] = _mlx
        log.info("server: MLX model ready")
    yield


app = FastAPI(title="Meridian Agent", version="1.0.0", lifespan=_lifespan)


# ---------------------------------------------------------------------------
# Shared
# ---------------------------------------------------------------------------

@app.get("/health")
async def health() -> dict:
    return {
        "status": "ok",
        "backend": _app_state.get("backend", "hermes"),
        "db": str(_DB_PATH),
        "db_exists": _DB_PATH.exists(),
    }


# ---------------------------------------------------------------------------
# Hermes backend — conversational AIAgent
# ---------------------------------------------------------------------------

class ChatRequest(BaseModel):
    message: str


class ChatResponse(BaseModel):
    response: str


@app.post("/chat", response_model=ChatResponse)
async def chat(req: ChatRequest) -> ChatResponse:
    if _app_state.get("backend") == "mlx":
        raise HTTPException(status_code=404, detail="Use /classify for mlx backend")

    from run_agent import AIAgent
    from agents._system_context import SYSTEM_CONTEXT
    from agents.config import MODEL, BASE_URL, API_KEY, AGENT_MAX_TOKENS

    agent = AIAgent(
        model=MODEL,
        base_url=BASE_URL,
        api_key=API_KEY or "none",
        enabled_toolsets=["terminal", "skills", "memory"],
        ephemeral_system_prompt=SYSTEM_CONTEXT,
        quiet_mode=True,
        skip_context_files=True,
        load_soul_identity=False,
        skip_memory=False,
        max_iterations=20,
        max_tokens=AGENT_MAX_TOKENS,
    )

    log.info("chat: %.120s", req.message)
    with contextlib.redirect_stdout(sys.stderr):
        result = agent.run_conversation(req.message)

    response = str(result.get("final_response") or result.get("response") or "")
    log.info("response: %.120s", response)
    return ChatResponse(response=response)


# ---------------------------------------------------------------------------
# MLX backend — direct in-process inference, model pre-loaded at startup
# ---------------------------------------------------------------------------

class ClassifyRequest(BaseModel):
    input: str  # fully-formatted user_message string (from build_user_message)


class ClassifyResponse(BaseModel):
    task_key: str | None
    session_type: str
    reasoning: str
    confidence: float
    dimensions: dict


_classify_log: "Any | None" = None  # JSONL file handle, opened at first request


def _get_classify_log() -> "Any":
    global _classify_log
    if _classify_log is None:
        import datetime
        log_dir = _SERVICES_DIR / "logs" / "mlx"
        log_dir.mkdir(parents=True, exist_ok=True)
        ts = datetime.datetime.now().strftime("%Y%m%dT%H%M%S")
        log_path = log_dir / f"server_{ts}.jsonl"
        _classify_log = log_path.open("w", encoding="utf-8")
        log.info("classify: writing request log to %s", log_path)
    return _classify_log


@app.post("/classify", response_model=ClassifyResponse)
async def classify(req: ClassifyRequest) -> ClassifyResponse:
    if _app_state.get("backend") != "mlx":
        raise HTTPException(status_code=404, detail="Use /chat for hermes backend")

    import json as _json
    import time as _time
    from outlines.inputs import Chat
    from mlx_lm.sample_utils import make_sampler

    m = _app_state["mlx_module"]
    model = m._get_model()
    messages = [
        {"role": "system", "content": m._SYSTEM_PROMPT},
        {"role": "user",   "content": req.input},
    ]
    t0 = _time.time()
    try:
        raw = model(
            Chat(messages),
            output_type=m.SessionClassification,
            max_tokens=m._MAX_TOKENS,
            sampler=make_sampler(temp=m._TEMPERATURE),
            verbose=False,
        )
        result = m.SessionClassification.model_validate_json(raw)
    except Exception as exc:
        log.warning("classify: inference error: %s", exc)
        raise HTTPException(status_code=500, detail=str(exc)) from exc

    elapsed = _time.time() - t0
    response = ClassifyResponse(
        task_key=result.task_key,
        session_type=result.session_type,
        reasoning=result.reasoning,
        confidence=max(0.0, min(1.0, result.confidence)),
        dimensions=result.dimensions,
    )

    log.info(
        "classify: task_key=%s session_type=%s confidence=%.2f elapsed=%.2fs",
        result.task_key, result.session_type, result.confidence, elapsed,
    )

    # Append a JSONL record to services/logs/mlx/server_<ts>.jsonl
    try:
        fh = _get_classify_log()
        fh.write(_json.dumps({
            "input":        req.input[:500],  # truncated for log readability
            "task_key":     result.task_key,
            "session_type": result.session_type,
            "confidence":   result.confidence,
            "reasoning":    result.reasoning,
            "elapsed_s":    round(elapsed, 2),
        }, ensure_ascii=False) + "\n")
        fh.flush()
    except Exception as exc:
        log.warning("classify: failed to write log entry: %s", exc)

    return response


# ---------------------------------------------------------------------------
# MLX backend — classify_sessions (batch, session-id driven)
# ---------------------------------------------------------------------------

class ClassifySessionsRequest(BaseModel):
    session_ids: list[int]
    meridian_db: str
    traceparent: str | None = None  # W3C traceparent propagated from Rust caller


@app.post("/classify_sessions")
async def classify_sessions(req: ClassifySessionsRequest) -> dict:
    """Classify one or more sessions by ID using the pre-loaded MLX model.

    Only available when the server is started with ``--backend mlx``.
    Returns a results list in the same JSON format the Rust daemon already
    parses from the subprocess stdout.
    """
    from fastapi.concurrency import run_in_threadpool

    if _app_state.get("backend") != "mlx":
        raise HTTPException(
            status_code=503,
            detail="classify_sessions is only available with --backend mlx",
        )

    m = _app_state.get("mlx_module")
    if m is None:
        raise HTTPException(status_code=503, detail="MLX model is still loading")

    fh = _get_classify_log()
    tracer = _app_state.get("tracer") or trace.get_tracer("meridian-agent-server-mlx")
    parent_ctx = observability.extract_parent_context(req.traceparent)

    with tracer.start_as_current_span("classify_sessions", context=parent_ctx) as span:
        span.set_attribute("session_count", len(req.session_ids))

        # Snapshot the OTel context while classify_sessions span is active so we
        # can attach it explicitly inside the threadpool (anyio copies contextvars,
        # but explicit attach is more reliable across anyio versions).
        ctx_snapshot = _otel_context.get_current()

        def _classify_all() -> list[dict]:
            # Attach classify_sessions context so _classify_one sub-spans
            # (db_fetch, build_prompt, llm_inference, parse_response) appear
            # as children of classify_sessions in the OO trace waterfall.
            _tok = _otel_context.attach(ctx_snapshot)
            try:
                # Always use the server's own _DB_PATH — ignoring req.meridian_db avoids
                # path-traversal: the server knows its DB from the environment.
                con = _sqlite3.connect(str(_DB_PATH), check_same_thread=False)
                con.row_factory = _sqlite3.Row
                try:
                    results: list[dict] = []
                    for sid in req.session_ids:
                        with tracer.start_as_current_span(
                            "classify_session",
                            attributes={"session_id": sid},
                        ):
                            result = m._classify_one_logged(sid, con, fh)
                        log.info(
                            "classify_sessions: session_id=%d task_key=%s session_type=%s elapsed_s=%.2f",
                            sid,
                            result.get("task_key"),
                            result.get("session_type"),
                            result.get("elapsed_s", 0.0),
                        )
                        results.append(result)
                    return results
                finally:
                    con.close()
            finally:
                _otel_context.detach(_tok)

        results = await run_in_threadpool(_classify_all)
        span.set_attribute("classified_count", len(results))

    return {"results": results}


# ---------------------------------------------------------------------------
# Entry point
# ---------------------------------------------------------------------------

def main() -> None:
    import uvicorn

    parser = argparse.ArgumentParser(description="Meridian agent server")
    parser.add_argument("--port",    type=int, default=7823)
    parser.add_argument("--host",    default="127.0.0.1")
    parser.add_argument(
        "--backend",
        choices=["hermes", "mlx"],
        default="hermes",
        help="hermes: AIAgent conversational mode  |  mlx: direct in-process inference",
    )
    args = parser.parse_args()

    _app_state["backend"] = args.backend
    tracer = observability.setup(f"meridian-agent-server-{args.backend}")
    _app_state["tracer"] = tracer

    log.info("meridian agent server (%s) on http://%s:%d", args.backend, args.host, args.port)
    uvicorn.run(app, host=args.host, port=args.port, log_level="warning")


if __name__ == "__main__":
    main()
