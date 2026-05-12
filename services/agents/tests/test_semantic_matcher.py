# meridian — normalises screenpipe activity into structured app sessions
"""Unit tests for Semantic Matcher scoring math.

We don't load the real bge-small encoder here — every test feeds the pure
helpers (`_cos_to_unit`, `_jaccard`, `_blend_score`, `_routing_decision`,
`derive_expected_dims`) deterministic inputs and checks the math.
"""
from __future__ import annotations

import math

import pytest

from agents import semantic_matcher


def test_cos_to_unit_rescales_extremes():
    """cosine -1/0/+1 map to 0/0.5/1 in unit space."""
    assert semantic_matcher._cos_to_unit(-1.0) == 0.0
    assert semantic_matcher._cos_to_unit(0.0) == 0.5
    assert semantic_matcher._cos_to_unit(1.0) == 1.0


def test_cos_to_unit_clamps_out_of_range():
    """Numerical noise outside [-1,1] is clamped, not propagated."""
    assert semantic_matcher._cos_to_unit(-2.0) == 0.0
    assert semantic_matcher._cos_to_unit(2.0) == 1.0


def test_jaccard_disjoint_identical_half():
    """Jaccard returns 0 for disjoint, 1 for identical, 0.5 for half-overlap."""
    assert semantic_matcher._jaccard(set(), set()) == 0.0
    assert semantic_matcher._jaccard({"a", "b"}, {"c", "d"}) == 0.0
    assert semantic_matcher._jaccard({"a", "b"}, {"a", "b"}) == 1.0
    # {a,b} vs {a,c}: intersection {a}, union {a,b,c} → 1/3
    assert math.isclose(semantic_matcher._jaccard({"a", "b"}, {"a", "c"}), 1 / 3)
    # {a,b} vs {a,b,c,d}: intersection {a,b}, union 4 → 0.5
    assert semantic_matcher._jaccard({"a", "b"}, {"a", "b", "c", "d"}) == 0.5


def test_activity_match_only_on_intersection():
    """activity_match returns 1 only when session activity intersects expected."""
    sess_dims = {"activity": {"coding"}}
    assert semantic_matcher._activity_match(sess_dims, {"activity": ["coding"]}) == 1.0
    assert semantic_matcher._activity_match(sess_dims, {"activity": ["debugging"]}) == 0.0
    assert semantic_matcher._activity_match(sess_dims, None) == 0.0
    assert semantic_matcher._activity_match({}, {"activity": ["coding"]}) == 0.0


def test_dim_overlap_score_blends_with_documented_weights():
    """activity / topic / tool blend at 0.40 / 0.35 / 0.25."""
    sess_dims = {
        "activity": {"coding"},
        "topic":    {"rust", "sqlite"},
        "tool":     {"vscode"},
    }
    expected = {
        "activity": ["coding"],         # match → 1.0
        "topic":    ["rust", "sqlite"], # identical → 1.0
        "tool":     ["vscode"],         # identical → 1.0
    }
    score, detail = semantic_matcher._dim_overlap_score(sess_dims, expected)
    # All three sub-scores are 1 → total = 0.40 + 0.35 + 0.25 = 1.0
    assert math.isclose(score, 1.0)
    assert detail["activity_match"] is True
    assert detail["topic_jaccard"] == 1.0
    assert detail["tool_jaccard"] == 1.0


def test_dim_overlap_score_weights_partial_overlap():
    """Activity-only overlap → 0.40 of the blend; topic/tool zero."""
    sess_dims = {"activity": {"coding"}, "topic": set(), "tool": set()}
    expected = {"activity": ["coding"], "topic": ["rust"], "tool": ["vscode"]}
    score, _detail = semantic_matcher._dim_overlap_score(sess_dims, expected)
    assert math.isclose(score, 0.40)


def test_blend_score_full_mode_weights():
    """has_dim and has_past → 0.55 cosine + 0.30 dim + 0.15 past."""
    out = semantic_matcher._blend_score(0.8, 0.6, 0.4, has_dim=True, has_past=True)
    assert math.isclose(out, 0.55 * 0.8 + 0.30 * 0.6 + 0.15 * 0.4, abs_tol=1e-9)


def test_blend_score_cold_start_no_past():
    """No past_vote → renormalise cosine/dim to 0.65/0.35."""
    out = semantic_matcher._blend_score(0.8, 0.6, 0.0, has_dim=True, has_past=False)
    assert math.isclose(out, 0.65 * 0.8 + 0.35 * 0.6, abs_tol=1e-9)


def test_blend_score_cold_start_no_dim():
    """No dim_overlap → renormalise cosine/past to 0.75/0.25."""
    out = semantic_matcher._blend_score(0.8, 0.0, 0.4, has_dim=False, has_past=True)
    assert math.isclose(out, 0.75 * 0.8 + 0.25 * 0.4, abs_tol=1e-9)


def test_blend_score_cosine_only():
    """No dim and no past → cosine carries the full weight."""
    out = semantic_matcher._blend_score(0.8, 0.0, 0.0, has_dim=False, has_past=False)
    assert math.isclose(out, 0.8, abs_tol=1e-9)


def test_routing_decision_auto_when_high_and_separated():
    """top1>=0.62 with gap>=0.08 routes to auto."""
    assert semantic_matcher._routing_decision(0.70, 0.60) == "auto"


def test_routing_decision_queue_when_top_passes_floor_only():
    """top1>=0.40 but gap too small → queue."""
    assert semantic_matcher._routing_decision(0.65, 0.60) == "queue"  # gap 0.05 < 0.08
    assert semantic_matcher._routing_decision(0.50, 0.10) == "queue"  # below auto floor


def test_routing_decision_skip_when_below_floor():
    """top1<0.40 → skip."""
    assert semantic_matcher._routing_decision(0.39, 0.0) == "skip"


def test_derive_expected_dims_maps_issue_types():
    """task→coding, bug→debugging, story→coding+planning, spike→research."""
    assert semantic_matcher.derive_expected_dims({"issue_type": "task", "title": "", "description_text": ""})["activity"] == ["coding"]
    assert semantic_matcher.derive_expected_dims({"issue_type": "bug",  "title": "", "description_text": ""})["activity"] == ["debugging"]
    assert semantic_matcher.derive_expected_dims({"issue_type": "spike", "title": "", "description_text": ""})["activity"] == ["research"]
    story = semantic_matcher.derive_expected_dims({"issue_type": "story", "title": "", "description_text": ""})
    assert "coding" in story["activity"] and "planning" in story["activity"]
    # Unknown issue_type falls through to the documented default ['coding'].
    assert semantic_matcher.derive_expected_dims({"issue_type": "weird", "title": "", "description_text": ""})["activity"] == ["coding"]


def test_derive_expected_dims_extracts_topics_and_tools_from_text():
    """Topic regexes and URL hosts apply to the title + description blob."""
    out = semantic_matcher.derive_expected_dims({
        "issue_type": "task",
        "title": "Wire sqlx pool into rust ETL",
        "description_text": "Track the work in github.com/foo/bar; coordinate via slack.",
        "project_key": "MER",
    })
    assert "rust" in out["topic"]
    assert "sqlite" in out["topic"]
    assert "github" in out["tool"]
