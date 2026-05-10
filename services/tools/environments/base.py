"""Stub for tools.environments.base — only the symbols we re-export are used."""
from __future__ import annotations


def touch_activity_if_due(*_args, **_kwargs) -> None:  # pragma: no cover
    """No-op: meridian-agents doesn't run code-execution toolchains."""
    return None
