"""Prefetch routes — /prefetch_model and /prefetch_status.

Eager, spec-aware model download for the onboarding wizard. The wizard's Model
step calls /prefetch_model right after the runtime is chosen, then polls
/prefetch_status for live progress. Downloads run in a background thread so the
event loop is never blocked; progress is shared via prefetch_state in agents._state.
"""
from __future__ import annotations

import logging
import threading
from pathlib import Path

from fastapi import APIRouter, HTTPException
from opentelemetry import trace

from agents._state import app_state, prefetch_state, prefetch_lock

log = logging.getLogger("agents.server")

router = APIRouter()


def _hf_cache_dir_for(model_id: str) -> Path:
    """The HF hub cache directory for `model_id` (where partial + complete blobs land)."""
    from huggingface_hub.constants import HF_HUB_CACHE
    return Path(HF_HUB_CACHE) / ("models--" + model_id.replace("/", "--"))


def _dir_size_bytes(path: Path) -> int:
    """Sum of all file sizes under `path` (includes HF `.incomplete` partials → live progress)."""
    total = 0
    if path.exists():
        for f in path.rglob("*"):
            try:
                if f.is_file():
                    total += f.stat().st_size
            except OSError:
                pass
    return total


def _prefetch_total_bytes(model_id: str) -> int:
    """Authoritative download size: sum HF sibling sizes filtered to the load() patterns.

    Computed upfront so the wizard's progress bar has a stable denominator, instead
    of summing concurrent per-file tqdm totals (which lurch as new bars spawn).
    """
    import fnmatch
    from huggingface_hub import HfApi

    # Lazy import avoids a circular import: agents.server imports this module at
    # load time, so prefetch.py cannot import from agents.server at module scope.
    from agents.server import _MODEL_ALLOW_PATTERNS

    info = HfApi().model_info(model_id, files_metadata=True)
    total = 0
    for sib in info.siblings or []:
        if any(fnmatch.fnmatch(sib.rfilename, pat) for pat in _MODEL_ALLOW_PATTERNS):
            total += sib.size or 0
    return total


def _run_prefetch(model_id: str) -> None:
    """Background worker: download the model's weights to the HF cache (no load)."""
    from agents.server import _MODEL_ALLOW_PATTERNS

    tracer = trace.get_tracer(__name__)
    with tracer.start_as_current_span("model_prefetch") as span:
        span.set_attribute("model_id", model_id)
        try:
            try:
                from mlx_lm.utils import _download as _mlx_download
                _mlx_download(model_id)  # exact fileset load() resolves; download-only
            except (ImportError, AttributeError):
                # Private primitive unavailable — replicate load()'s default patterns.
                from huggingface_hub import snapshot_download
                snapshot_download(model_id, allow_patterns=_MODEL_ALLOW_PATTERNS)
            received = _dir_size_bytes(_hf_cache_dir_for(model_id))
            with prefetch_lock:
                prefetch_state["received"] = received or prefetch_state["total"]
                prefetch_state["state"] = "done"
            span.set_attribute("received_bytes", received)
            log.info("server: model prefetch complete", extra={"model_id": model_id, "received_bytes": received})
        except Exception as exc:  # noqa: BLE001 — report, never crash the server
            with prefetch_lock:
                prefetch_state["state"] = "error"
                prefetch_state["error"] = str(exc)
            span.set_status(trace.Status(trace.StatusCode.ERROR, str(exc)))
            log.error("server: model prefetch failed", extra={"model_id": model_id, "error": str(exc)})


@router.post("/prefetch_model")
async def prefetch_model() -> dict:
    """Start the eager, spec-aware model download (idempotent). Returns current status.

    Apple Intelligence backend → nothing to download (no-op `done`). Re-POSTing
    while `downloading`/`done` returns the live state without spawning a second
    download; an earlier `error` is retried.
    """
    from fastapi.concurrency import run_in_threadpool

    from agents.llm_selector import APPLE_INTELLIGENCE_ID
    m = app_state.get("mlx_module")
    model_id = m.MODEL_ID if m else None
    if model_id == APPLE_INTELLIGENCE_ID or model_id is None:
        return {"state": "done", "model_id": model_id, "received": 0, "total": 0, "error": None}

    with prefetch_lock:
        # Idempotent only for the SAME model: a completed/in-flight prefetch for
        # one model must not block starting a different one after the user changes
        # their model preference (which changes what MODEL_ID returns).
        same_model = prefetch_state.get("model_id") == model_id
        if same_model and prefetch_state["state"] in ("downloading", "done"):
            return dict(prefetch_state)  # idempotent — no duplicate downloads
        prefetch_state.update(state="downloading", model_id=model_id, received=0, total=0, error=None)

    try:
        total = await run_in_threadpool(_prefetch_total_bytes, model_id)
    except Exception as exc:  # noqa: BLE001 — size probe is best-effort; download still runs
        total = 0
        log.warning("server: prefetch size-probe failed (bar will be indeterminate)", extra={"error": str(exc)})
    with prefetch_lock:
        prefetch_state["total"] = total

    threading.Thread(target=_run_prefetch, args=(model_id,), daemon=True).start()
    log.info("server: model prefetch started", extra={"model_id": model_id, "total_bytes": total})
    with prefetch_lock:
        return dict(prefetch_state)


@router.get("/prefetch_status")
async def prefetch_status() -> dict:
    """Live prefetch progress. `received` is recomputed from the cache dir while downloading."""
    with prefetch_lock:
        st = dict(prefetch_state)
    if st["state"] == "downloading" and st["model_id"]:
        st["received"] = _dir_size_bytes(_hf_cache_dir_for(st["model_id"]))
    return st
