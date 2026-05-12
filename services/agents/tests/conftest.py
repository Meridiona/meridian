# meridian — normalises screenpipe activity into structured app sessions
"""Shared pytest fixtures and sys.path setup for the agents test suite.

The tests are written so they can be invoked from `services/` with:

    cd services && source .venv/bin/activate && python -m pytest agents/tests/ -v

The conftest prepends `services/` to sys.path so `from agents import …` works
without requiring the package to be installed.
"""
from __future__ import annotations

import sys
from pathlib import Path

import pytest

# services/ is two levels up from this conftest (agents/tests/conftest.py).
_SERVICES_DIR = Path(__file__).resolve().parent.parent.parent
if str(_SERVICES_DIR) not in sys.path:
    sys.path.insert(0, str(_SERVICES_DIR))


@pytest.fixture
def make_session():
    """Factory that builds a session dict from kwargs.

    Mirrors the shape produced by `agents.db.fetch_unprocessed_sessions` so
    rule fixtures can drive the rule registry without round-tripping through
    SQLite. Anything not passed defaults to a sensible empty value.
    """
    def _factory(**kw) -> dict:
        return {
            "id":              kw.get("id", 1),
            "app_name":        kw.get("app_name", ""),
            "duration_s":      kw.get("duration_s", 0),
            "window_titles":   kw.get("window_titles", []),
            "ocr_samples":     kw.get("ocr_samples", []),
            "audio_snippets":  kw.get("audio_snippets", []),
            "category":        kw.get("category", ""),
            "confidence":      kw.get("confidence", 0.0),
        }

    return _factory


@pytest.fixture
def pm_task():
    """Factory that builds a pm_task row from kwargs (mirror of pm_tasks schema)."""
    def _factory(**kw) -> dict:
        return {
            "task_key":         kw.get("task_key", "KAN-1"),
            "title":            kw.get("title", ""),
            "description_text": kw.get("description_text", ""),
            "issue_type":       kw.get("issue_type", "task"),
            "project_key":      kw.get("project_key", "KAN"),
            "status":           kw.get("status", "Open"),
        }

    return _factory
