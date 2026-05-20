"""Metrics for the Stage 3 session→task classifier eval suite.

Import metric lists from here into test files. Do not construct metrics inline
in test files — keep them here so thresholds stay in one place.
"""
from __future__ import annotations

from deepeval.metrics import BaseMetric
from deepeval.test_case import LLMTestCase

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
# Metric lists — import these in test files, do not construct inline
# ---------------------------------------------------------------------------

# Primary metric for all classifier evals: did the model pick the right key?
CLASSIFIER_METRICS = [
    TaskKeyMatchMetric(threshold=1.0),
]
