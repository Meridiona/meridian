"""Stub for tools.environments.local — used only at import time."""
from __future__ import annotations

import os
import shutil
from pathlib import Path

_HERMES_PROVIDER_ENV_BLOCKLIST = frozenset()


def _find_shell() -> str:  # pragma: no cover
    return os.environ.get("SHELL") or shutil.which("bash") or "/bin/sh"


def _resolve_safe_cwd(cwd: str | None) -> str:  # pragma: no cover
    return cwd or os.getcwd()


def _sanitize_subprocess_env(env: dict | None = None) -> dict:  # pragma: no cover
    return dict(env or os.environ)


class LocalEnvironment:  # pragma: no cover
    """No-op stub — terminal tools are not enabled in meridian-agents."""

    def __init__(self, *_args, **_kwargs) -> None:
        raise NotImplementedError("LocalEnvironment is not available in meridian-agents")
