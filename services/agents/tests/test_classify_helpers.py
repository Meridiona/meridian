"""Unit tests for agents.routes.classify._normalize_confidence.

The regression under test: a confidence that overshoots the 0–1 ceiling by a
little (1.1, 1.5) must NOT be divided by 100 (which would push a high-confidence
match below classifier._MIN_CONFIDENCE=0.5 and silently drop a real ticket). Only
clearly percentage-scale values (>=2, e.g. 50, 90) are divided. Model-free.
"""
from __future__ import annotations

import pytest

from agents.routes.classify import _normalize_confidence


@pytest.mark.parametrize("raw,expected", [
    # in-range 0–1 values pass through untouched
    (0.0, 0.0), (0.5, 0.5), (0.9, 0.9), (1.0, 1.0),
    # (1,2) overshoots are clamped to 1.0 — NOT divided (the bug: 1.1 -> 0.011)
    (1.1, 1.0), (1.5, 1.0), (1.99, 1.0),
    # >=2 is percentage scale -> /100
    (2.0, 0.02), (50.0, 0.5), (90.0, 0.9), (100.0, 1.0), (150.0, 1.0),
    # negatives clamp to 0
    (-0.3, 0.0),
])
def test_normalize_confidence(raw, expected):
    assert _normalize_confidence(raw) == pytest.approx(expected)


def test_overshoot_stays_above_min_confidence():
    """A 1.1/1.5 overshoot must remain >= _MIN_CONFIDENCE (the silent-drop guard)."""
    from agents.worklog_pipeline.classifier import _MIN_CONFIDENCE
    assert _normalize_confidence(1.1) >= _MIN_CONFIDENCE
    assert _normalize_confidence(1.5) >= _MIN_CONFIDENCE


@pytest.mark.parametrize("bad", [None, "", "abc", [], {}])
def test_non_numeric_yields_zero(bad):
    assert _normalize_confidence(bad) == 0.0
