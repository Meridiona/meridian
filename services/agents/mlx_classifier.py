"""MLX model lifecycle — load, session context manager, idle eviction.

Manages a single shared MLX generative model (Qwen3) for the agent server.
Exposes model_session() for inference, evict_resident_model() for the
single-slot guarantee (never two models resident simultaneously), and
_get_tokenizer() for callers that need the HF tokenizer directly.
"""
from __future__ import annotations

import gc
import logging
import os
import threading
import time
from contextlib import contextmanager
from typing import Any, Iterator

from agents import model_registry, observability

log = logging.getLogger("agents.mlx_classifier")
tracer = observability.setup("meridian-mlx-classifier")

# Generative/classifier checkpoint — resolved from the model registry (the single
# source of truth for all three pipeline models), env-overridable via MERIDIAN_LLM_ID.
# Exposed as a module attribute for external compatibility; the live value is read
# from the registry per-call inside _get_model() so a runtime env change is honoured.
MODEL_ID = model_registry.llm_id()

_IDLE_EVICT_S = float(os.environ.get("MLX_IDLE_EVICT_S", "120"))


class _ModelBundle:
    """Thin wrapper so callers can access model.model and model.mlx_tokenizer."""
    __slots__ = ("model", "mlx_tokenizer")

    def __init__(self, model: Any, tokenizer: Any) -> None:
        self.model = model
        self.mlx_tokenizer = tokenizer


_model_cache: dict[str, _ModelBundle] = {}
_tokenizer_cache: dict[str, Any] = {}
_model_lock = threading.Lock()
_in_flight = 0
_last_used = time.monotonic()


def _get_model() -> _ModelBundle:
    """Return the loaded model bundle, loading from disk on the first call.

    Cache-miss load is done under _model_lock (double-checked) so concurrent
    callers can't double-load and the idle evictor can't race the load.
    """
    model_id = model_registry.llm_id()
    cached = _model_cache.get(model_id)
    if cached is not None:
        return cached

    with _model_lock:
        cached = _model_cache.get(model_id)
        if cached is not None:
            return cached
        try:
            import mlx_lm
        except ImportError as exc:
            raise ImportError(
                f"Required package not installed: {exc}. "
                "Install with: pip install 'mlx-lm>=0.22'"
            ) from exc

        log.info("mlx_classifier: loading %s (first call this process)", model_id)
        t0 = time.time()
        mlx_model, tokenizer = mlx_lm.load(
            model_id,
            tokenizer_config={"trust_remote_code": True},
        )
        bundle = _ModelBundle(mlx_model, tokenizer)
        log.info("mlx_classifier: model loaded in %.1fs", time.time() - t0)

        _model_cache[model_id] = bundle
        _tokenizer_cache[model_id] = tokenizer
        return bundle


def _get_tokenizer() -> Any:
    """Return the HF tokenizer for the current model, loading the model if needed."""
    model_id = model_registry.llm_id()
    tok = _tokenizer_cache.get(model_id)
    if tok is not None:
        return tok
    return _get_model().mlx_tokenizer


@contextmanager
def model_session() -> Iterator[_ModelBundle]:
    """Yield the loaded model bundle, marking it in-flight so the idle evictor
    never frees it mid-inference. Wrap every direct model call in this.
    """
    global _in_flight, _last_used
    with _model_lock:
        _in_flight += 1
    try:
        yield _get_model()
    finally:
        with _model_lock:
            _in_flight -= 1
            _last_used = time.monotonic()


def maybe_evict_idle(idle_s: float | None = None) -> float | None:
    """Evict the model if it's resident, nothing is in flight, and it's been
    idle longer than ``idle_s`` (default MLX_IDLE_EVICT_S). Returns the GB freed,
    or None if no eviction happened. Safe to call from a threadpool worker.

    Uses a non-blocking lock acquire: if an inference/load is mutating state we
    simply skip this tick and try again on the next one.
    """
    ttl = _IDLE_EVICT_S if idle_s is None else idle_s
    if ttl <= 0:
        return None
    if not _model_lock.acquire(blocking=False):
        return None
    try:
        if _in_flight > 0 or not _model_cache:
            return None
        if (time.monotonic() - _last_used) < ttl:
            return None
        try:
            import mlx.core as mx
            before = mx.get_active_memory()
        except Exception:  # noqa: BLE001
            mx, before = None, 0
        _model_cache.clear()
        _tokenizer_cache.clear()
        gc.collect()
        freed = 0.0
        if mx is not None:
            mx.clear_cache()
            freed = max(0.0, (before - mx.get_active_memory()) / 1e9)
        log.info(
            "mlx_classifier: evicted idle model (idle ≥ %.0fs), freed ~%.1f GB",
            ttl, freed,
        )
        return freed
    finally:
        _model_lock.release()


def evict_resident_model() -> float | None:
    """Force-evict the resident generative model NOW, ignoring the idle timer.

    The single-slot guarantee: the reranker must never be resident alongside
    the generative model. Callers that are about to load a different model
    call this first. Respects _in_flight — returns None if an inference is
    running (the worklog pipeline is serialised, so nothing is in flight at a
    phase boundary). Returns GB freed, or None if nothing was evicted.
    """
    if not _model_lock.acquire(blocking=False):
        return None
    try:
        if _in_flight > 0 or not _model_cache:
            return None
        try:
            import mlx.core as mx
            before = mx.get_active_memory()
        except Exception:  # noqa: BLE001
            mx, before = None, 0
        _model_cache.clear()
        _tokenizer_cache.clear()
        gc.collect()
        freed = 0.0
        if mx is not None:
            mx.clear_cache()
            freed = max(0.0, (before - mx.get_active_memory()) / 1e9)
        log.info("mlx_classifier: force-evicted resident model, freed ~%.1f GB", freed)
        return freed
    finally:
        _model_lock.release()


def model_resident() -> bool:
    """True if the MLX model is currently loaded in memory."""
    return bool(_model_cache)


def model_active_memory_gb() -> float | None:
    """Live Metal active-memory footprint in GB, or None if MLX is unavailable.

    Process-wide Metal active memory (≈ the model when resident — the model
    dominates, though a transient load allocation can briefly inflate it), and
    the only honest measure: ps/Activity Monitor can't see Metal unified
    memory (they undercount by ~6.5 GB).
    """
    try:
        import mlx.core as mx
        return round(mx.get_active_memory() / 1e9, 2)
    except Exception:  # noqa: BLE001
        return None
