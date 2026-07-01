"""Unit tests for the pure helpers in agents.routes.generate.

``_clamp01`` and ``_str_list`` shape the model's (FSM-validated but still
value-unbounded) JSON into the WorklogDraft fields the pipeline persists. They run
on every generated worklog, so a regression here corrupts every draft.
"""
from __future__ import annotations

import pytest

from agents.routes.generate import _clamp01, _str_list


# ─────────────────────── _clamp01 ─────────────────────────────────────────────
@pytest.mark.parametrize("raw,expected", [
    (0.5, 0.5), (1.0, 1.0), (0.0, 0.0),
    (-1.0, 0.0), (2.5, 1.0),
    ("0.7", 0.7),          # numeric strings coerce
    (None, 0.0), ("nope", 0.0),  # junk → 0.0, never raises
])
def test_clamp01(raw, expected):
    assert _clamp01(raw) == pytest.approx(expected)


# ─────────────────────── _str_list ────────────────────────────────────────────
def test_str_list_keeps_nonempty_strings_trimmed():
    assert _str_list(["  a ", "b", "", "   "]) == ["a", "b"]


def test_str_list_drops_non_strings():
    assert _str_list(["ok", 3, None, {"x": 1}, "fine"]) == ["ok", "fine"]


@pytest.mark.parametrize("bad", [None, "a string", 5, {"a": 1}])
def test_str_list_non_list_returns_empty(bad):
    assert _str_list(bad) == []
