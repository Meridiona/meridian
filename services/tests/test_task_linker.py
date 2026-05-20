"""Tests for run_task_linker.py — function-level unit tests.

These tests exercise _build_session and _classify_one without requiring
hermes or any LLM to be installed. conftest.py stubs the heavy imports.
"""
import sys
from pathlib import Path
from typing import Any
from unittest.mock import MagicMock, patch

# Make services/ importable
sys.path.insert(0, str(Path(__file__).parent.parent))

from agents.run_task_linker import _build_session, _classify_one  # noqa: E402


# ---------------------------------------------------------------------------
# Fixtures
# ---------------------------------------------------------------------------

def make_session_raw(**overrides: Any) -> dict[str, Any]:
    base = {
        "id": 1,
        "app_name": "Xcode",
        "duration_s": 120,
        "session_text": "implementing hermes bridge",
        "window_titles": ["Xcode — meridian"],
        "category": "coding",
        "confidence": 0.8,
    }
    return {**base, **overrides}


def make_decision(
    task_key: str | None = "KAN-42",
    confidence: float = 0.9,
    routing: str = "auto",
    method: str = "task_classifier",
    dimensions: dict | None = None,
    elapsed_s: float = 0.05,
) -> MagicMock:
    d = MagicMock()
    d.chosen_task_key = task_key
    d.confidence = confidence
    d.routing = routing
    d.method = method
    d.dimensions = dimensions if dimensions is not None else {}
    d.elapsed_s = elapsed_s
    d.reasoning = "test reasoning"
    return d


# ---------------------------------------------------------------------------
# _build_session
# ---------------------------------------------------------------------------

class TestBuildSession:
    def test_maps_all_explicit_fields(self):
        raw = {"id": 5, "app_name": "VSCode", "duration_s": 60,
               "session_text": "hello", "window_titles": ["w"]}
        result = _build_session(raw)
        assert result["id"] == 5
        assert result["app_name"] == "VSCode"
        assert result["duration_s"] == 60
        assert result["session_text"] == "hello"
        assert result["window_titles"] == ["w"]

    def test_defaults_for_missing_optional_fields(self):
        result = _build_session({"id": 1})
        assert result["session_text"] == ""
        assert result["window_titles"] == []
        assert result["audio_snippets"] == []
        assert result["confidence"] == 0.0
        assert result["category"] is None

    def test_passes_audio_snippets_through(self):
        raw = {"id": 2, "audio_snippets": ["spoken text here"]}
        result = _build_session(raw)
        assert result["audio_snippets"] == ["spoken text here"]


# ---------------------------------------------------------------------------
# _classify_one
# ---------------------------------------------------------------------------

class TestClassifyOne:
    def test_maps_decision_to_result_shape(self):
        raw = make_session_raw(id=5)
        decision = make_decision(task_key="KAN-42", confidence=0.9, routing="auto",
                                  dimensions={"activity": ["coding"]})
        with patch("agents.run_task_linker.classify_session", return_value=decision):
            result = _classify_one(raw, {"KAN-42": {}}, [])

        assert result["session_id"] == 5
        assert result["task_key"] == "KAN-42"
        assert result["confidence"] == 0.9
        assert result["routing"] == "auto"
        assert result["dimensions"] == {"activity": ["coding"]}
        assert "elapsed_s" in result

    def test_hermes_aiagent_method_on_success(self):
        """Successful hermes classification returns method=hermes_aiagent."""
        raw = make_session_raw(id=3)
        # At e370944, run_task_linker directly returns hermes_aiagent method
        # (no task_classifier_agent.py intermediary)
        with patch("agents.run_task_linker._call_hermes") as mock_hermes:
            mock_hermes.return_value = (
                "KAN-42",  # task_key
                0.9,       # confidence
                "Coding work",  # reasoning
                {"activity": ["coding"]},  # dimensions
                0.5,       # elapsed_s
            )
            result = _classify_one(raw, {}, [])
        assert result["method"] == "hermes_aiagent"

    def test_preserves_non_tiebreak_method_names(self):
        raw = make_session_raw(id=4)
        decision = make_decision(method="rule_match")
        with patch("agents.run_task_linker.classify_session", return_value=decision):
            result = _classify_one(raw, {}, [])
        assert result["method"] == "rule_match"

    def test_exception_returns_llm_error_result_not_exception(self):
        raw = make_session_raw(id=7)
        with patch("agents.run_task_linker.classify_session",
                   side_effect=RuntimeError("LLM timeout")):
            result = _classify_one(raw, {}, [])

        assert result["session_id"] == 7
        assert result["task_key"] is None
        assert result["routing"] == "skip"
        assert result["method"] == "llm_error"
        assert result["confidence"] == 0.0
        assert "LLM timeout" in result["reasoning"]

    def test_overhead_routing_when_no_task_key(self):
        raw = make_session_raw(id=9)
        decision = make_decision(task_key=None, routing="skip", confidence=0.15)
        with patch("agents.run_task_linker.classify_session", return_value=decision):
            result = _classify_one(raw, {}, [])
        assert result["task_key"] is None
        assert result["routing"] == "skip"

    def test_elapsed_s_is_float(self):
        raw = make_session_raw(id=11)
        decision = make_decision(elapsed_s=0.123)
        with patch("agents.run_task_linker.classify_session", return_value=decision):
            result = _classify_one(raw, {}, [])
        assert isinstance(result["elapsed_s"], float)

    def test_dimensions_empty_dict_when_none_returned(self):
        raw = make_session_raw(id=13)
        decision = make_decision(dimensions={})
        with patch("agents.run_task_linker.classify_session", return_value=decision):
            result = _classify_one(raw, {}, [])
        assert result["dimensions"] == {}
