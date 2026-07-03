"""Unit tests for agents.schemas — the FSM (outlines) JSON output contracts.

These guard the shape the grammar-constrained endpoints (/classify_tasks,
/generate_worklog, /propose_ticket) are compiled against. They are model-free:
they validate the Pydantic schemas directly, the same objects outlines turns into
a logits processor. If a field name / type / default here drifts from what the
route handlers read, these break before a real worklog hour does.
"""
from __future__ import annotations

import pytest
from pydantic import ValidationError

from agents.schemas import ClassifyMatch, ClassifyOut, WorklogOut, ProposeOut


# ─────────────────────── ClassifyOut ──────────────────────────────────────────
def test_classify_out_parses_typical_fsm_json():
    obj = ClassifyOut.model_validate_json(
        '{"reasoning":"matches the OO work","matches":'
        '[{"task_key":"KAN-241","confidence":0.9,"why":"added tracing"}]}'
    )
    assert obj.reasoning.startswith("matches")
    assert len(obj.matches) == 1
    assert obj.matches[0].task_key == "KAN-241"
    assert obj.matches[0].confidence == pytest.approx(0.9)


def test_classify_out_empty_matches_is_valid():
    """An empty match list is the valid 'nothing matched' answer."""
    obj = ClassifyOut.model_validate_json('{"reasoning":"no overlap","matches":[]}')
    assert obj.matches == []


def test_classify_out_matches_defaults_to_empty():
    obj = ClassifyOut(reasoning="x")
    assert obj.matches == []


def test_classify_match_requires_core_fields():
    with pytest.raises(ValidationError):
        ClassifyMatch(confidence=0.5, why="missing task_key")  # type: ignore[call-arg]


# ─────────────────────── WorklogOut ───────────────────────────────────────────
def test_worklog_out_parses_and_defaults_lists():
    obj = WorklogOut.model_validate_json('{"summary":"did the thing","confidence":0.8}')
    assert obj.summary == "did the thing"
    assert obj.what_shipped == []
    assert obj.decisions == []
    assert obj.confidence == pytest.approx(0.8)


def test_worklog_out_confidence_defaults_zero():
    obj = WorklogOut(summary="s")
    assert obj.confidence == 0.0


# ─────────────────────── ProposeOut ───────────────────────────────────────────
def test_propose_out_abstention_shape():
    obj = ProposeOut.model_validate_json(
        '{"reasoning":"covered already","should_propose":false}'
    )
    assert obj.should_propose is False
    assert obj.title == ""
    assert obj.issue_type == "Task"   # default


def test_propose_out_full_draft():
    obj = ProposeOut.model_validate_json(
        '{"reasoning":"new defect","should_propose":true,"issue_type":"Bug",'
        '"title":"Fix flaky parser","description":"It drops matches."}'
    )
    assert obj.should_propose is True
    assert obj.issue_type == "Bug"
    assert obj.title == "Fix flaky parser"


def test_propose_out_issue_type_is_constrained():
    """issue_type is a Literal['Task','Bug'] — anything else is rejected."""
    with pytest.raises(ValidationError):
        ProposeOut(reasoning="x", should_propose=True, issue_type="Epic")  # type: ignore[arg-type]
