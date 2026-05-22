"""Metrics for the Stage 3 session→task classifier eval suite.

Import metric lists from here into eval files. Do not construct metrics inline
in eval files — keep them here so thresholds stay in one place.

Two evaluation levels (as per DeepEval agent eval docs):
  AGENT_E2E_METRICS   — end-to-end trace, attached to @observe on outer agent fn
  CLASSIFIER_METRICS  — component span, attached to @observe on inner classify fn
"""
from __future__ import annotations

import os
from pathlib import Path
import sys

from deepeval.metrics import BaseMetric, TaskCompletionMetric
from deepeval.models import OllamaModel
from deepeval.test_case import LLMTestCase

_SERVICES_DIR = Path(__file__).parent.parent.parent
if str(_SERVICES_DIR) not in sys.path:
    sys.path.insert(0, str(_SERVICES_DIR))

# Use the same Ollama model already configured for hermes as the judge.
# Fully offline — no OpenAI API key or Confident AI login required.
_MODEL  = os.environ.get("OLLAMA_MODEL", "gemma4:31b")
_HOST   = os.environ.get("OLLAMA_HOST",  "http://localhost:11434")

_judge = OllamaModel(model=_MODEL, base_url=_HOST)

_NULL_LITERALS = {"none", "null", "n/a", "nil", "undefined", ""}


def _normalise(value: str | None) -> str | None:
    if value is None:
        return None
    stripped = value.strip().lower()
    return None if stripped in _NULL_LITERALS else stripped


class TaskKeyMatchMetric(BaseMetric):
    """Exact-match on the extracted task_key — no LLM call required.

    Treats None, "none", "null", "n/a", "", "nil", "undefined" as equivalent
    null labels so overhead/untracked sessions compare correctly.
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
            predicted = _normalise(test_case.actual_output)
            expected = _normalise(test_case.expected_output)
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
# Metric lists — import these in eval files, do not construct inline
# ---------------------------------------------------------------------------

# End-to-end (trace level): attached to @observe on the outer agent function.
# LLM judge asks "did the agent successfully complete the session classification task?"
# TaskCompletionMetric infers both the task and the outcome from the full trace —
# no expected_output needed at this level.
AGENT_E2E_METRICS = [
    TaskCompletionMetric(
        threshold=0.5,
        model=_judge,
        include_reason=True,
    ),
]

# Component level (span level): attached to @observe on the inner classify function.
# Exact match on the extracted task_key — no LLM judge needed.
CLASSIFIER_METRICS = [
    TaskKeyMatchMetric(threshold=1.0),
]
