"""Health routes — /health and /info."""
from __future__ import annotations

import os
from pathlib import Path

from fastapi import APIRouter

from agents._state import app_state

router = APIRouter()

_DB_PATH = Path(os.environ.get("MERIDIAN_DB", Path.home() / ".meridian/meridian.db"))


@router.get("/health")
async def health() -> dict:
    return {
        "status": "ok",
        "backend": "mlx",
        "db": str(_DB_PATH),
        "db_exists": _DB_PATH.exists(),
    }


@router.get("/info")
async def info() -> dict:
    """Return the identity of the model and its live memory state.

    `active_memory_gb` reads `mx.get_active_memory()` — the ONLY honest measure
    of the model's footprint, since Metal unified memory is invisible to `ps`
    and Activity Monitor (they undercount the model by ~6.5 GB).
    """
    m = app_state.get("mlx_module")
    return {
        "backend":          "mlx",
        "model_id":         m.MODEL_ID if m else None,
        "loaded_at":        app_state.get("loaded_at"),
        "model_resident":   m.model_resident() if m else False,
        "active_memory_gb": m.model_active_memory_gb() if m else None,
    }
