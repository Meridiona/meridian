"""Metrics for the Stage 3 session→task classifier eval suite.

Import metric lists from here into eval files. Do not construct metrics inline
in eval files — keep them here so thresholds stay in one place.

expected_output format (JSON string):
    {"task_key": "KAN-107" | "none", "session_type": "task"|"overhead"|"untracked",
     "reasoning": "<ground truth reasoning from original classifier>"}

actual_output format (JSON string from classifier):
    Same shape — task_key, session_type, reasoning.
    Callers may also pass a plain task_key string for backward compat.

Two evaluation levels:
  AGENT_E2E_METRICS   — end-to-end trace, attached to @observe on outer agent fn
  CLASSIFIER_METRICS  — component span, exact-match on task_key + session_type
"""
from __future__ import annotations

import json
import os
from pathlib import Path
import sys

from deepeval.metrics import BaseMetric, TaskCompletionMetric
from deepeval.models import OllamaModel
from deepeval.test_case import LLMTestCase

_SERVICES_DIR = Path(__file__).parent.parent.parent
if str(_SERVICES_DIR) not in sys.path:
    sys.path.insert(0, str(_SERVICES_DIR))

_MODEL = os.environ.get("OLLAMA_MODEL", "gemma4:31b")
_HOST  = os.environ.get("OLLAMA_HOST",  "http://localhost:11434")

_judge = OllamaModel(model=_MODEL, base_url=_HOST)

_NULL_LITERALS = {"none", "null", "n/a", "nil", "undefined", ""}


def _normalise_key(value: str | None) -> str | None:
    if value is None:
        return None
    stripped = value.strip().lower()
    return None if stripped in _NULL_LITERALS else stripped


def _parse_expected(raw: str | None) -> dict:
    """Parse expected_output — JSON string or plain task_key string."""
    if not raw:
        return {"task_key": None, "session_type": None, "reasoning": ""}
    try:
        return json.loads(raw)
    except (json.JSONDecodeError, ValueError):
        return {"task_key": raw.strip(), "session_type": None, "reasoning": ""}


def _parse_actual(raw: str | None) -> dict:
    """Parse actual_output — JSON string or plain task_key string."""
    return _parse_expected(raw)


class TaskKeyMatchMetric(BaseMetric):
    """Exact-match on task_key — no LLM call required.

    Handles both JSON expected_output and plain task_key strings.
    None / 'none' / 'null' / 'n/a' / '' treated as equivalent null labels.
    """

    def __init__(self, threshold: float = 1.0):
        self.threshold = threshold
        self.score: float = 0.0
        self.success: bool = False
        self.reason: str = ""
        self.error: str | None = None

    @property
    def __name__(self) -> str:
        return "TaskKeyMatch"

    def measure(self, test_case: LLMTestCase) -> float:
        try:
            predicted = _normalise_key(_parse_actual(test_case.actual_output).get("task_key"))
            expected  = _normalise_key(_parse_expected(test_case.expected_output).get("task_key"))
            self.score = 1.0 if predicted == expected else 0.0
            self.reason = f"predicted={predicted!r} expected={expected!r}"
            self.success = self.score >= self.threshold
        except Exception as exc:
            self.error = str(exc)
            self.score = 0.0
            self.success = False
            raise
        return self.score

    async def a_measure(self, test_case: LLMTestCase) -> float:
        return self.measure(test_case)

    def is_successful(self) -> bool:
        if self.error is not None:
            self.success = False
        else:
            self.success = self.score >= self.threshold
        return self.success


class SessionTypeMatchMetric(BaseMetric):
    """Exact-match on session_type (task / overhead / untracked) — no LLM call."""

    def __init__(self, threshold: float = 1.0):
        self.threshold = threshold
        self.score: float = 0.0
        self.success: bool = False
        self.reason: str = ""
        self.error: str | None = None

    @property
    def __name__(self) -> str:
        return "SessionTypeMatch"

    def measure(self, test_case: LLMTestCase) -> float:
        try:
            predicted = (_parse_actual(test_case.actual_output).get("session_type") or "").strip().lower()
            expected  = (_parse_expected(test_case.expected_output).get("session_type") or "").strip().lower()
            self.score = 1.0 if predicted == expected else 0.0
            self.reason = f"predicted={predicted!r} expected={expected!r}"
            self.success = self.score >= self.threshold
        except Exception as exc:
            self.error = str(exc)
            self.score = 0.0
            self.success = False
            raise
        return self.score

    async def a_measure(self, test_case: LLMTestCase) -> float:
        return self.measure(test_case)

    def is_successful(self) -> bool:
        if self.error is not None:
            self.success = False
        else:
            self.success = self.score >= self.threshold
        return self.success


# ---------------------------------------------------------------------------
# Metric lists — import these in eval files
# ---------------------------------------------------------------------------

AGENT_E2E_METRICS = [
    TaskCompletionMetric(threshold=0.5, model=_judge, include_reason=True),
]

CLASSIFIER_METRICS = [
    TaskKeyMatchMetric(threshold=1.0),
    SessionTypeMatchMetric(threshold=1.0),
]
