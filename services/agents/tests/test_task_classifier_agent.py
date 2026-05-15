# meridian — normalises screenpipe activity into structured app sessions
"""Unit tests for Task Classifier Agent JSON parsing and routing.

The actual hermes AIAgent call is never reached — every test exercises
`parse_response`, `extract_json`, `_repair_truncated_json`, or
`routing_for` in isolation.
"""
from __future__ import annotations

import pytest

from agents._parser import parse_response, routing_for, _repair_truncated_json


VALID_KEYS = {"KAN-86", "KAN-99"}
AUTO_FLOOR  = 0.65
QUEUE_FLOOR = 0.40


def test_parse_response_accepts_clean_json():
    """A naked JSON object parses straight through."""
    raw = '{"task_key": "KAN-86", "confidence": 0.85, "reasoning": "obvious match"}'
    key, conf, reason, _dims, err = parse_response(raw, VALID_KEYS)
    assert err is None
    assert key == "KAN-86"
    assert conf == 0.85
    assert reason == "obvious match"


def test_parse_response_strips_markdown_fences():
    """A model that wraps the object in ```json``` fences still parses."""
    raw = """Sure! Here's my answer:
```json
{"task_key": "KAN-99", "confidence": 0.7, "reasoning": "fenced output"}
```
Hope that helps.
"""
    key, conf, reason, _dims, err = parse_response(raw, VALID_KEYS)
    assert err is None
    assert key == "KAN-99"
    assert conf == 0.7


def test_parse_response_recovers_truncated_json():
    """Truncated JSON with a trailing comma — repaired by stripping the orphan
    comma and balancing braces. (Was xfail until the trailing-comma fix in
    `_repair_truncated_json`; the test now confirms the regression hole is
    closed.)"""
    raw = '{"task_key": "KAN-86", "confidence": 0.85,'
    key, conf, _reason, _dims, err = parse_response(raw, VALID_KEYS)
    assert err is None
    assert key == "KAN-86"
    assert conf == 0.85


def test_parse_response_recovers_truncated_orphan_key():
    """Tail like `, "k2":` should drop the orphan key, not produce invalid JSON."""
    raw = '{"task_key": "KAN-86", "confidence": 0.85, "reasoning":'
    key, conf, _reason, _dims, err = parse_response(raw, VALID_KEYS)
    assert err is None
    assert key == "KAN-86"
    assert conf == 0.85


def test_parse_response_recovers_truncated_json_without_trailing_comma():
    """Sibling case that does not hit the trailing-comma bug — repair succeeds."""
    raw = '{"task_key": "KAN-86", "confidence": 0.85'
    key, conf, _reason, _dims, err = parse_response(raw, VALID_KEYS)
    assert err is None
    assert key == "KAN-86"
    assert conf == 0.85


@pytest.mark.parametrize("literal", ["None", "null", "n/a", "NIL", "Undefined", "  None  "])
def test_parse_response_coerces_null_literals(literal):
    """'none', 'null', 'n/a', 'nil', 'undefined' (any case, with whitespace) → None."""
    raw = f'{{"task_key": "{literal}", "confidence": 0.0, "reasoning": "no match"}}'
    key, _conf, _reason, _dims, err = parse_response(raw, VALID_KEYS)
    assert err is None
    assert key is None


def test_parse_response_rejects_unknown_task_key():
    """A task_key outside the candidate set is an error (defensive against hallucination)."""
    raw = '{"task_key": "ZZZ-99", "confidence": 0.9, "reasoning": "made up"}'
    key, _conf, _reason, _dims, err = parse_response(raw, VALID_KEYS)
    assert err is not None
    assert key is None


def test_routing_for_auto_at_or_above_floor():
    """confidence >= AUTO_FLOOR (0.65) → auto."""
    assert routing_for(0.65, "KAN-86", AUTO_FLOOR, QUEUE_FLOOR) == "auto"
    assert routing_for(0.95, "KAN-86", AUTO_FLOOR, QUEUE_FLOOR) == "auto"


def test_routing_for_queue_between_floors():
    """0.40 ≤ confidence < 0.65 → queue."""
    assert routing_for(0.40, "KAN-86", AUTO_FLOOR, QUEUE_FLOOR) == "queue"
    assert routing_for(0.60, "KAN-86", AUTO_FLOOR, QUEUE_FLOOR) == "queue"


def test_routing_for_skip_when_low_confidence():
    """confidence < 0.40 → skip."""
    assert routing_for(0.39, "KAN-86", AUTO_FLOOR, QUEUE_FLOOR) == "skip"
    assert routing_for(0.0, "KAN-86", AUTO_FLOOR, QUEUE_FLOOR) == "skip"


def test_routing_for_skip_when_task_key_none():
    """A null task_key always routes to skip regardless of confidence."""
    assert routing_for(0.99, None, AUTO_FLOOR, QUEUE_FLOOR) == "skip"


def test_repair_truncated_json_closes_dangling_quote_then_braces():
    """_repair_truncated_json closes any open string then balances braces."""
    partial = '{"task_key": "KAN-86", "reasoning": "I think this is the right'
    repaired = _repair_truncated_json(partial)
    assert repaired.endswith('"}')
    import json
    obj = json.loads(repaired)
    assert obj["task_key"] == "KAN-86"


def test_repair_truncated_json_balances_nested_braces():
    """Nested objects with a missing closer get closed in the right order."""
    partial = '{"task_key": "KAN-86", "meta": {"a": 1'
    repaired = _repair_truncated_json(partial)
    import json
    obj = json.loads(repaired)
    assert obj["task_key"] == "KAN-86"
    assert obj["meta"]["a"] == 1


def test_parse_response_handles_empty_input():
    """Empty/whitespace input is rejected with an error."""
    key, conf, _reason, _dims, err = parse_response("", VALID_KEYS)
    assert key is None
    assert conf == 0.0
    assert err is not None
