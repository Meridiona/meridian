# meridian — normalises screenpipe activity into structured app sessions
"""Tests for agents._parser — parse_response and extract_json."""
from __future__ import annotations

import json

import pytest

from agents._parser import parse_response, extract_json


def test_parse_empty_string():
    """Empty input must return overhead/error tuple without raising."""
    task_key, confidence, reasoning, dimensions, session_type, err = parse_response("", valid_keys={"KAN-1"})
    assert task_key is None
    assert confidence == 0.0
    assert session_type == "overhead"
    assert err is not None
    assert "empty" in err.lower()


def test_parse_none_like_string():
    """Whitespace-only input is treated the same as empty."""
    task_key, confidence, reasoning, dimensions, session_type, err = parse_response("   ", valid_keys=set())
    assert task_key is None
    assert err is not None


def test_parse_valid_json_response():
    """A realistic JSON response from the agent parses all fields correctly."""
    valid_keys = {"KAN-42", "KAN-7"}
    payload = {
        "task_key": "KAN-42",
        "confidence": 0.91,
        "reasoning": "The window titles mention the Jira key KAN-42 explicitly.",
        "session_type": "task",
        "dimensions": {
            "activity": ["coding"],
            "tool": ["vscode"],
        },
    }
    raw = json.dumps(payload)
    task_key, confidence, reasoning, dimensions, session_type, err = parse_response(raw, valid_keys)

    assert err is None
    assert task_key == "KAN-42"
    assert abs(confidence - 0.91) < 1e-6
    assert "KAN-42" in reasoning
    assert session_type == "task"
    assert dimensions.get("activity") == ["coding"]
    assert dimensions.get("tool") == ["vscode"]


def test_parse_valid_json_overhead():
    """Session typed overhead with no task_key is returned cleanly."""
    valid_keys = {"KAN-1"}
    payload = {
        "task_key": None,
        "confidence": 0.0,
        "reasoning": "Slack usage, no ticket visible.",
        "session_type": "overhead",
    }
    raw = json.dumps(payload)
    task_key, confidence, reasoning, dimensions, session_type, err = parse_response(raw, valid_keys)

    assert err is None
    assert task_key is None
    assert session_type == "overhead"


def test_parse_invalid_task_key():
    """A task_key that is not in valid_keys returns None and an error string."""
    valid_keys = {"KAN-1", "KAN-2"}
    payload = {"task_key": "KAN-999", "confidence": 0.8, "session_type": "task"}
    raw = json.dumps(payload)
    task_key, confidence, reasoning, dimensions, session_type, err = parse_response(raw, valid_keys)

    assert task_key is None
    assert err is not None
    assert "KAN-999" in err


def test_parse_garbage_string():
    """Random text that contains no JSON object returns a safe error result."""
    task_key, confidence, reasoning, dimensions, session_type, err = parse_response(
        "Sorry, I cannot help with that.", valid_keys={"KAN-1"}
    )
    assert task_key is None
    assert confidence == 0.0
    assert session_type == "overhead"
    assert err is not None


def test_parse_fenced_json():
    """Markdown-fenced JSON block is extracted and parsed correctly."""
    valid_keys = {"KAN-5"}
    inner = json.dumps({"task_key": "KAN-5", "confidence": 0.75, "session_type": "task"})
    raw = f"```json\n{inner}\n```"
    task_key, confidence, reasoning, dimensions, session_type, err = parse_response(raw, valid_keys)

    assert err is None
    assert task_key == "KAN-5"


def test_parse_confidence_clamped():
    """Confidence values outside [0, 1] are clamped."""
    valid_keys = {"KAN-1"}
    payload = {"task_key": "KAN-1", "confidence": 9.5, "session_type": "task"}
    _, confidence, _, _, _, err = parse_response(json.dumps(payload), valid_keys)
    assert err is None
    assert confidence == 1.0

    payload2 = {"task_key": "KAN-1", "confidence": -3.0, "session_type": "task"}
    _, confidence2, _, _, _, err2 = parse_response(json.dumps(payload2), valid_keys)
    assert err2 is None
    assert confidence2 == 0.0


def test_extract_json_returns_none_on_empty():
    assert extract_json("") is None
    assert extract_json("   ") is None


def test_extract_json_plain_object():
    raw = '{"key": "value"}'
    assert extract_json(raw) == raw


def test_extract_json_embedded_in_text():
    raw = 'Here is my answer: {"task_key": "KAN-1"} done.'
    result = extract_json(raw)
    assert result is not None
    obj = json.loads(result)
    assert obj["task_key"] == "KAN-1"
