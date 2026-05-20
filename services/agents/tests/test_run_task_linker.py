# meridian — normalises screenpipe activity into structured app sessions
"""Tests for agents.run_task_linker — input validation and subprocess boundary.

Protocol: Rust sends {"sessions": [...], "pm_tasks": [...]} via stdin.
Python classifies and returns {"results": [...]} on stdout.

Subprocess tests use an inline wrapper that stubs hermes so no LLM is needed.
Unit tests patch classify_session directly via the conftest stub.
"""
from __future__ import annotations

import json
import subprocess
import sys
from pathlib import Path
from unittest.mock import MagicMock, patch

import pytest

_SERVICES_DIR = Path(__file__).resolve().parent.parent.parent

# ── subprocess helper ──────────────────────────────────────────────────────────

_WRAPPER = """\
import sys
from unittest.mock import MagicMock
# stub heavy imports before loading run_task_linker
_tc = MagicMock()
_d = MagicMock()
_d.chosen_task_key = 'KAN-1'; _d.confidence = 0.85
_d.routing = 'auto'; _d.method = 'task_classifier'
_d.dimensions = {}; _d.elapsed_s = 0.01
_d.reasoning = 'stub'; _d.debug = {}
_tc.classify_session = lambda *a, **kw: _d
_tc.ClassifierDecision = MagicMock()
sys.modules['agents.task_classifier_agent'] = _tc
_obs = MagicMock()
_obs.setup = lambda *a, **kw: MagicMock()
_obs.extract_parent_context = lambda *a, **kw: None
sys.modules['agents.observability'] = _obs
import agents.run_task_linker as m
m.main()
"""


def _run_subprocess(payload: dict, *, timeout: int = 10) -> subprocess.CompletedProcess:
    return subprocess.run(
        [sys.executable, "-c", _WRAPPER],
        input=json.dumps(payload),
        capture_output=True,
        text=True,
        timeout=timeout,
        env={**__import__("os").environ, "PYTHONPATH": str(_SERVICES_DIR)},
    )


# ── subprocess boundary tests ──────────────────────────────────────────────────

def test_empty_sessions_exits_cleanly():
    """Empty sessions list → exit 0, {"results": []}."""
    proc = _run_subprocess({"sessions": [], "pm_tasks": []})
    assert proc.returncode == 0, f"stderr: {proc.stderr[:500]}"
    assert json.loads(proc.stdout.strip()) == {"results": []}


def test_one_session_returns_one_result():
    """A single session produces one result with all required fields."""
    payload = {
        "sessions": [{"id": 42, "app_name": "Xcode", "duration_s": 60, "session_text": "coding"}],
        "pm_tasks": [],
    }
    proc = _run_subprocess(payload)
    assert proc.returncode == 0, f"stderr: {proc.stderr[:500]}"
    out = json.loads(proc.stdout.strip())
    assert len(out["results"]) == 1
    r = out["results"][0]
    for field in ("session_id", "task_key", "confidence", "routing", "method", "dimensions", "elapsed_s"):
        assert field in r, f"missing field: {field}"


def test_invalid_sessions_type_exits_with_error():
    """Non-list 'sessions' value must cause a non-zero exit."""
    proc = _run_subprocess({"sessions": "not-a-list", "pm_tasks": []})
    assert proc.returncode != 0, "expected non-zero exit when sessions is not a list"


def test_malformed_stdin_exits_with_error():
    """Non-JSON stdin must cause a non-zero exit."""
    proc = subprocess.run(
        [sys.executable, "-c", _WRAPPER],
        input="this is not json",
        capture_output=True,
        text=True,
        timeout=10,
        env={**__import__("os").environ, "PYTHONPATH": str(_SERVICES_DIR)},
    )
    assert proc.returncode != 0


# ── unit tests (no subprocess, classify_session is patched) ───────────────────

def _make_decision(**kw) -> MagicMock:
    d = MagicMock()
    d.chosen_task_key = kw.get("task_key", "KAN-1")
    d.confidence      = kw.get("confidence", 0.85)
    d.routing         = kw.get("routing", "auto")
    d.method          = kw.get("method", "task_classifier")
    d.dimensions      = kw.get("dimensions", {})
    d.elapsed_s       = kw.get("elapsed_s", 0.01)
    d.reasoning       = kw.get("reasoning", "stub")
    d.debug           = {}
    return d


def test_classify_one_returns_expected_shape():
    from agents.run_task_linker import _classify_one
    decision = _make_decision(task_key="KAN-42", confidence=0.9, routing="auto",
                              dimensions={"activity": ["coding"]})
    with patch("agents.run_task_linker.classify_session", return_value=decision):
        result = _classify_one({"id": 5, "app_name": "VSCode", "duration_s": 60}, {}, [])
    assert result["session_id"] == 5
    assert result["task_key"] == "KAN-42"
    assert result["confidence"] == 0.9
    assert result["routing"] == "auto"
    assert result["dimensions"] == {"activity": ["coding"]}
    assert isinstance(result["elapsed_s"], float)


def test_classify_one_relabels_task_classifier_to_llm_standalone():
    from agents.run_task_linker import _classify_one
    decision = _make_decision(method="task_classifier")
    with patch("agents.run_task_linker.classify_session", return_value=decision):
        result = _classify_one({"id": 3}, {}, [])
    assert result["method"] == "llm_standalone"


def test_classify_one_preserves_other_method_names():
    from agents.run_task_linker import _classify_one
    decision = _make_decision(method="rule_match")
    with patch("agents.run_task_linker.classify_session", return_value=decision):
        result = _classify_one({"id": 4}, {}, [])
    assert result["method"] == "rule_match"


def test_classify_one_exception_returns_llm_error():
    from agents.run_task_linker import _classify_one
    with patch("agents.run_task_linker.classify_session", side_effect=RuntimeError("timeout")):
        result = _classify_one({"id": 7}, {}, [])
    assert result["session_id"] == 7
    assert result["task_key"] is None
    assert result["routing"] == "skip"
    assert result["method"] == "llm_error"
    assert "timeout" in result["reasoning"]
