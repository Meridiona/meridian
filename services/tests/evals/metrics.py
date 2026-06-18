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
from deepeval.test_case import LLMTestCase

_SERVICES_DIR = Path(__file__).parent.parent.parent
if str(_SERVICES_DIR) not in sys.path:
    sys.path.insert(0, str(_SERVICES_DIR))

_MODEL = os.environ.get("OLLAMA_MODEL", "gemma4:31b")
_HOST  = os.environ.get("OLLAMA_HOST",  "http://localhost:11434")


def _make_judge() -> "object | None":
    """Build the LLM judge — ONLY the agent-e2e TaskCompletionMetric needs it.

    The classifier metrics below (TaskKeyMatch / SessionTypeMatch) are pure
    exact-match and require no judge. Importing this module must therefore NOT
    hard-depend on Ollama: if the `ollama` package or server is unavailable we
    return None and the classifier eval runs unaffected. Construction is inside
    the function (not at import) because OllamaModel() pulls in `ollama` only
    when instantiated.
    """
    try:
        from deepeval.models import OllamaModel

        return OllamaModel(model=_MODEL, base_url=_HOST)
    except Exception as exc:  # noqa: BLE001 — missing pkg, server down, etc.
        import warnings

        warnings.warn(
            f"LLM judge unavailable ({exc}); agent-e2e metrics disabled. "
            "Classifier exact-match metrics are unaffected.",
            stacklevel=2,
        )
        return None


_judge = _make_judge()

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


class UntrackedNotTaskMetric(BaseMetric):
    """The failure mode that matters: does the classifier mark non-task work as
    `task` (a false-positive link)?  Every forced `task` injects work that never
    happened into a real ticket's worklog AND buries the genuine untracked work.

    Scope: this metric judges ONLY goldens whose ground truth is NOT a task
    (expected task_key is null/none — i.e. untracked or overhead). For those it
    scores 1.0 when the prediction is also non-task (no task_key AND session_type
    != 'task'), 0.0 when the model forced a task. For goldens that ARE genuine
    tasks it abstains (score 1.0, reason 'n/a — expected task') so it neither
    rewards nor penalises task recall — use TaskKeyMatch/SessionType for that.
    No LLM call.
    """

    def __init__(self, threshold: float = 1.0):
        self.threshold = threshold
        self.score: float = 0.0
        self.success: bool = False
        self.reason: str = ""
        self.error: str | None = None

    @property
    def __name__(self) -> str:
        return "UntrackedNotTask"

    @staticmethod
    def _is_task(parsed: dict) -> bool:
        key = _normalise_key(parsed.get("task_key"))
        stype = (parsed.get("session_type") or "").strip().lower()
        return key is not None or stype == "task"

    def measure(self, test_case: LLMTestCase) -> float:
        try:
            expected = _parse_expected(test_case.expected_output)
            actual = _parse_actual(test_case.actual_output)
            expected_is_task = self._is_task(expected)
            if expected_is_task:
                # Abstain — this metric only guards the non-task cases.
                self.score = 1.0
                self.reason = "n/a — expected task"
                self.success = True
                return self.score
            predicted_is_task = self._is_task(actual)
            self.score = 0.0 if predicted_is_task else 1.0
            self.reason = (
                f"expected=non-task ({expected.get('session_type')}), "
                f"predicted={'TASK ' + str(actual.get('task_key')) if predicted_is_task else actual.get('session_type')}"
            )
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

# Only built when a judge is available — otherwise empty so importing this module
# (e.g. for the classifier eval) never requires Ollama.
AGENT_E2E_METRICS = (
    [TaskCompletionMetric(threshold=0.5, model=_judge, include_reason=True)]
    if _judge is not None
    else []
)

CLASSIFIER_METRICS = [
    TaskKeyMatchMetric(threshold=1.0),
    SessionTypeMatchMetric(threshold=1.0),
]

# Untracked-focused suite: adds the false-positive guard. Use this for the
# real_labeled golden set, whose whole point is "don't force a task".
UNTRACKED_FOCUS_METRICS = [
    TaskKeyMatchMetric(threshold=1.0),
    SessionTypeMatchMetric(threshold=1.0),
    UntrackedNotTaskMetric(threshold=1.0),
]
