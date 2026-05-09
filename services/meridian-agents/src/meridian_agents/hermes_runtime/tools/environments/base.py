# meridian — normalises screenpipe activity into structured app sessions
# STUB — see tools/environments/__init__.py for context.

from typing import Any, Callable


def set_activity_callback(callback: Callable[..., Any]) -> None:
    """Stub: no-op in meridian-agents. The real hermes implementation
    registers a callback that gets called from inside terminal/browser
    tool runs. We never run those tools, so the registration is harmless."""
    return None


def get_activity_callback() -> Callable[..., Any] | None:
    return None
