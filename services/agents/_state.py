"""Process-global server state shared between the FastAPI app and route modules."""
from __future__ import annotations

import asyncio
import threading
from typing import Any

# Populated in _lifespan (mlx_module, loaded_at, model_sem, tracer, self_url, …)
# and read by every route module.
app_state: dict[str, Any] = {}

# Shared prefetch progress, guarded by prefetch_lock. states: idle|downloading|done|error
prefetch_state: dict[str, Any] = {
    "state": "idle",
    "model_id": None,
    "received": 0,
    "total": 0,
    "error": None,
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
