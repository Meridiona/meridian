"""conftest.py — stubs out observability imports so tests run without an LLM installed."""
import sys
from unittest.mock import MagicMock

_observability = MagicMock()
_observability.setup = lambda *a, **kw: MagicMock()
sys.modules["agents.observability"] = _observability
