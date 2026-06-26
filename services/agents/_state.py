"""Process-global server state shared between the FastAPI app and route modules."""
from __future__ import annotations

import asyncio
import threading
from typing import Any

# Populated in _lifespan (mlx_module, loaded_at, model_sem, tracer, self_url, …)
# and read by every route module.
app_state: dict[str, Any] = {}

# Shared prefetch progress, guarded by prefetch_lock. states: idle|downloading|done|error.
# `received`/`total` are AGGREGATE byte counts summed across every pipeline model
# (llm + reranker + embedder), so the wizard's progress bar has a single honest
# denominator. The wire contract the Rust tray decodes is exactly
# state/received/total/error (tray/src-tauri/src/mlx_server.rs::PrefetchStatus);
# `models` is an additive per-model breakdown the tray ignores.
prefetch_state: dict[str, Any] = {
    "state": "idle",
    "received": 0,
    "total": 0,
    # Live transfer rate in bytes/sec, sourced from HF's own tqdm progress
    # (see routes/prefetch._SpeedTqdm); 0 when not actively transferring.
    "speed": 0.0,
    "error": None,
    # Per-model rows: {"role", "model_id", "loader", "received", "total", "state"}.
    "models": [],
}
prefetch_lock = threading.Lock()


def model_sem() -> "asyncio.Semaphore":
    """Return the process-global single-slot model semaphore.

    Created once in _lifespan and stored in app_state. Every endpoint that
    runs a model inference acquires this before calling run_in_threadpool so
    that concurrent requests never compete on the GPU.
    """
    sem = app_state.get("model_sem")
    if sem is None:  # fallback if called before lifespan (e.g. tests)
        sem = asyncio.Semaphore(1)
        app_state["model_sem"] = sem
    return sem
