# meridian — normalises screenpipe activity into structured app sessions
"""Focused tests for coding_agent_indexer.jsonl_meta."""
from __future__ import annotations

import json
from datetime import timezone

from coding_agent_indexer.jsonl_meta import parse_session_slices


def test_parse_codex_session_slices_normalizes_rollout_schema(tmp_path):
    """Codex rollout JSONLs should parse into the shared day-slice model."""
    path = tmp_path / "rollout-2026-05-29T11-40-53-abc123.jsonl"
    records = [
        {
            "timestamp": "2026-05-29T06:12:17.503Z",
            "type": "session_meta",
            "payload": {
                "id": "abc123",
                "cwd": "/repo/meridian",
                "originator": "codex-tui",
                "source": "cli",
                "thread_source": "user",
            },
        },
        {
            "timestamp": "2026-05-29T06:12:20.000Z",
            "type": "event_msg",
            "payload": {"type": "user_message", "message": "inspect the parser"},
        },
        {
            "timestamp": "2026-05-29T06:12:28.000Z",
            "type": "event_msg",
            "payload": {
                "type": "agent_message",
                "message": [
                    {"text": "Opened the file."},
                    {"text": "Found the bug."},
                ],
            },
        },
        {
            "timestamp": "2026-05-29T06:12:28.000Z",
            "type": "response_item",
            "payload": {
                "type": "message",
                "role": "assistant",
                "content": [{"type": "output_text", "text": "duplicate transport wrapper"}],
            },
        },
    ]
    path.write_text("".join(json.dumps(r) + "\n" for r in records))

    meta, slices = parse_session_slices(path, agent="codex", local_tz=timezone.utc)

    assert meta.agent == "codex"
    assert meta.cwd == "/repo/meridian"
    assert meta.user_turns == 1
    assert meta.assistant_turns == 1
    assert meta.started_at == "2026-05-29T06:12:17.503Z"
    assert meta.ended_at == "2026-05-29T06:12:28.000Z"

    assert len(slices) == 1
    assert slices[0].day_utc == "2026-05-29"
    assert slices[0].user_turns == 1
    assert slices[0].assistant_turns == 1
    assert slices[0].active_seconds == 10
    assert "[user] inspect the parser" in slices[0].transcript
    assert "[codex] Opened the file.\nFound the bug." in slices[0].transcript
    assert "duplicate transport wrapper" not in slices[0].transcript


def test_parse_codex_session_slices_split_by_day(tmp_path):
    """Codex sessions spanning midnight should produce one row per day."""
    path = tmp_path / "rollout-2026-05-29T11-40-53-def456.jsonl"
    records = [
        {
            "timestamp": "2026-05-29T23:59:55Z",
            "type": "session_meta",
            "payload": {"id": "def456", "cwd": "/repo/meridian"},
        },
        {
            "timestamp": "2026-05-29T23:59:58Z",
            "type": "event_msg",
            "payload": {"type": "user_message", "message": "before midnight"},
        },
        {
            "timestamp": "2026-05-30T00:00:07Z",
            "type": "event_msg",
            "payload": {"type": "agent_message", "message": "after midnight"},
        },
    ]
    path.write_text("".join(json.dumps(r) + "\n" for r in records))

    meta, slices = parse_session_slices(path, agent="codex", local_tz=timezone.utc)

    assert meta.user_turns == 1
    assert meta.assistant_turns == 1
    assert [s.day_utc for s in slices] == ["2026-05-29", "2026-05-30"]
    assert slices[0].transcript == "[user] before midnight"
    assert slices[1].transcript == "[codex] after midnight"
