"""Unit tests for agents.worklog_pipeline.models — the pipeline DTO helpers.

Covers ``classification_keys`` (the extractor the classifier uses to pull
(key, confidence, why) tuples out of a ClassificationResult) and the ``_clamp01``
confidence guard, since FSM decoding cannot enforce the [0,1] numeric bound.
"""
from __future__ import annotations

import pytest

from agents.worklog_pipeline.models import (
    ClassificationResult,
    TaskClassification,
    classification_keys,
    _clamp01,
)


# ─────────────────────── _clamp01 ─────────────────────────────────────────────
@pytest.mark.parametrize("raw,expected", [
    (0.5, 0.5), (0.0, 0.0), (1.0, 1.0),
    (-0.3, 0.0), (1.7, 1.0), (42.0, 1.0),
])
def test_clamp01(raw, expected):
    assert _clamp01(raw) == pytest.approx(expected)


# ─────────────────────── classification_keys ──────────────────────────────────
def test_classification_keys_extracts_and_clamps():
    result = ClassificationResult(
        reasoning="r",
        matches=[
            TaskClassification(task_key="KAN-1", confidence=0.9, why="a"),
            TaskClassification(task_key="KAN-2", confidence=5.0, why="b"),  # out of range
        ],
    )
    keys = classification_keys(result)
    assert keys == [("KAN-1", pytest.approx(0.9), "a"), ("KAN-2", pytest.approx(1.0), "b")]


def test_classification_keys_empty():
    assert classification_keys(ClassificationResult(reasoning="r", matches=[])) == []
