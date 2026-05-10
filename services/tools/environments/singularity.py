"""Stub for tools.environments.singularity — used only at import time."""
from __future__ import annotations

from pathlib import Path


def _get_scratch_dir() -> Path:  # pragma: no cover
    p = Path.home() / ".meridian" / "scratch"
    p.mkdir(parents=True, exist_ok=True)
    return p


class SingularityEnvironment:  # pragma: no cover
    def __init__(self, *_args, **_kwargs) -> None:
        raise NotImplementedError("SingularityEnvironment not available in meridian-agents")
