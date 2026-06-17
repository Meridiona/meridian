"""Unit tests for the candidate-description cap in `_format_candidates`.

The cap is configurable via CANDIDATE_DESC_CAP (default 0 = no cap). These cover
the default uncapped behaviour and an explicit ceiling.

Run: services/.venv/bin/pytest services/tests/test_format_candidates.py -v
"""
from __future__ import annotations

import sys
from pathlib import Path

import pytest

_SERVICES_DIR = Path(__file__).resolve().parent.parent
if str(_SERVICES_DIR) not in sys.path:
    sys.path.insert(0, str(_SERVICES_DIR))

from agents import _prompts  # noqa: E402


def _task(desc: str) -> dict:
    return {"task_key": "K-1", "title": "t", "description_text": desc}


def test_default_no_cap_keeps_full_description(monkeypatch):
    monkeypatch.setattr(_prompts, "CANDIDATE_DESC_CAP", 0)
    desc = "x" * 1000
    out = _prompts._format_candidates([_task(desc)])
    assert desc in out          # full text present
    assert "…" not in out       # no truncation marker


def test_positive_cap_truncates_with_marker(monkeypatch):
    monkeypatch.setattr(_prompts, "CANDIDATE_DESC_CAP", 50)
    out = _prompts._format_candidates([_task("y" * 100)])
    assert "y" * 50 + "…" in out
    assert "y" * 51 not in out  # nothing past the cap


def test_description_under_cap_unchanged(monkeypatch):
    monkeypatch.setattr(_prompts, "CANDIDATE_DESC_CAP", 240)
    out = _prompts._format_candidates([_task("short desc")])
    assert "short desc" in out
    assert "…" not in out
