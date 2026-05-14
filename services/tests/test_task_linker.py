"""Tests for run_task_linker.py — JSON bridge contract.

These tests exercise _build_session and _classify_one without requiring
hermes or any LLM to be installed. conftest.py stubs the heavy imports.
"""
import json
import subprocess
import sys
from pathlib import Path
from typing import Any
from unittest.mock import MagicMock, patch

import pytest

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
    method: str = "agent_tiebreak",
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
        with patch("agents.run_task_linker.agent_tiebreak", return_value=decision):
            result = _classify_one(raw, {"KAN-42": {}}, [])

        assert result["session_id"] == 5
        assert result["task_key"] == "KAN-42"
        assert result["confidence"] == 0.9
        assert result["routing"] == "auto"
        assert result["dimensions"] == {"activity": ["coding"]}
        assert "elapsed_s" in result

    def test_relabels_agent_tiebreak_method_to_llm_standalone(self):
        raw = make_session_raw(id=3)
        decision = make_decision(method="agent_tiebreak")
        with patch("agents.run_task_linker.agent_tiebreak", return_value=decision):
            result = _classify_one(raw, {}, [])
        assert result["method"] == "llm_standalone", \
            "agent_tiebreak must be renamed to llm_standalone in output"

    def test_preserves_non_tiebreak_method_names(self):
        raw = make_session_raw(id=4)
        decision = make_decision(method="rule_match")
        with patch("agents.run_task_linker.agent_tiebreak", return_value=decision):
            result = _classify_one(raw, {}, [])
        assert result["method"] == "rule_match"

    def test_exception_returns_llm_error_result_not_exception(self):
        raw = make_session_raw(id=7)
        with patch("agents.run_task_linker.agent_tiebreak",
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
        with patch("agents.run_task_linker.agent_tiebreak", return_value=decision):
            result = _classify_one(raw, {}, [])
        assert result["task_key"] is None
        assert result["routing"] == "skip"

    def test_elapsed_s_is_float(self):
        raw = make_session_raw(id=11)
        decision = make_decision(elapsed_s=0.123)
        with patch("agents.run_task_linker.agent_tiebreak", return_value=decision):
            result = _classify_one(raw, {}, [])
        assert isinstance(result["elapsed_s"], float)

    def test_dimensions_empty_dict_when_none_returned(self):
        raw = make_session_raw(id=13)
        decision = make_decision(dimensions={})
        with patch("agents.run_task_linker.agent_tiebreak", return_value=decision):
            result = _classify_one(raw, {}, [])
        assert result["dimensions"] == {}


# ---------------------------------------------------------------------------
# Subprocess JSON-contract test (requires python3 + no hermes needed)
# ---------------------------------------------------------------------------

class TestSubprocessContract:
    """Runs run_task_linker as a subprocess with stub hermes to verify
    the stdout contract without touching a real LLM."""

    SERVICES_DIR = Path(__file__).parent.parent

    def _run(self, payload: dict) -> dict:
        env_override = {
            **__import__("os").environ,
            "PYTHONPATH": str(self.SERVICES_DIR),
        }
        # Inject conftest stubs via PYTHONSTARTUP is unreliable; instead
        # we write a tiny wrapper script that patches sys.modules first.
        wrapper = (
            "import sys\n"
            "from unittest.mock import MagicMock\n"
            "sys.modules['agents._hermes_setup'] = MagicMock()\n"
            "m = MagicMock(); m.setup = lambda *a, **kw: MagicMock()\n"
            "sys.modules['agents.observability'] = m\n"
            # stub agent_tiebreak to return a fixed decision
            "from unittest.mock import MagicMock as _M\n"
            "_d = _M(); _d.chosen_task_key = 'KAN-1'; _d.confidence = 0.85\n"
            "_d.routing = 'auto'; _d.method = 'agent_tiebreak'\n"
            "_d.dimensions = {}; _d.elapsed_s = 0.01; _d.reasoning = 'stub'\n"
            "_tb = _M(); _tb.agent_tiebreak = lambda *a, **kw: _d\n"
            "_tb.AgentDecision = _M(); _tb.MODE_STANDALONE = 'standalone'\n"
            "sys.modules['agents.agent_tiebreaker'] = _tb\n"
            "import agents.run_task_linker as m\n"
            "m.main()\n"
        )
        proc = subprocess.run(
            [sys.executable, "-c", wrapper],
            input=json.dumps(payload),
            capture_output=True,
            text=True,
            timeout=10,
        )
        assert proc.returncode == 0, f"run_task_linker failed:\n{proc.stderr}"
        return json.loads(proc.stdout.strip())

    def test_output_has_results_key(self):
        output = self._run({"sessions": [], "pm_tasks": []})
        assert "results" in output

    def test_empty_sessions_returns_empty_results(self):
        output = self._run({"sessions": [], "pm_tasks": []})
        assert output["results"] == []

    def test_one_session_returns_one_result(self):
        payload = {
            "sessions": [make_session_raw(id=42)],
            "pm_tasks": [],
        }
        output = self._run(payload)
        assert len(output["results"]) == 1
        r = output["results"][0]
        assert r["session_id"] == 42
        assert "task_key" in r
        assert "confidence" in r
        assert "routing" in r
        assert "method" in r
        assert "dimensions" in r
        assert "elapsed_s" in r

    def test_multiple_sessions_all_get_results(self):
        sessions = [make_session_raw(id=i) for i in range(1, 6)]
        output = self._run({"sessions": sessions, "pm_tasks": []})
        assert len(output["results"]) == 5
        ids = {r["session_id"] for r in output["results"]}
        assert ids == {1, 2, 3, 4, 5}
