"""conftest.py — stubs out hermes and observability imports so tests run
without hermes or an LLM installed."""
import sys
from unittest.mock import MagicMock

# Stub modules that require hermes or external services at import time.
# These must be inserted before run_task_linker is imported.
_hermes_setup = MagicMock()
_hermes_setup.ensure_hermes_importable = lambda: None
sys.modules["agents._hermes_setup"] = _hermes_setup

_observability = MagicMock()
_observability.setup = lambda *a, **kw: MagicMock()
sys.modules["agents.observability"] = _observability
