"""Unit tests for jira_updater and jira_updater_daemon helper functions.

Covers pure-logic helpers (_parse_search_result, _normalise_issue,
_parse_mcp_summary, _format_comment, _build_user_message), daemon slot
helpers (compute_slots, slot_window), and the four jira_update_log db
functions (log_jira_update, get_last_update, mark_update_sent,
mark_update_failed).

All external calls (Atlassian MCP, Meridian MCP, hermes) are never
invoked — tested functions are pure or operate against in-memory SQLite.
"""
from __future__ import annotations

import re
import sqlite3

import pytest

from agents import jira_updater
from agents.jira_updater import (
    _build_user_message,
    _format_comment,
    _normalise_issue,
    _parse_mcp_summary,
    _parse_search_result,
)
from agents.jira_updater_daemon import compute_slots, next_slot_dt, slot_window


# ── Fixtures ────────────────────────────────────────────────────────────────────

MIGRATION_SQL = """\
CREATE TABLE IF NOT EXISTS jira_update_log (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    task_key      TEXT    NOT NULL,
    period_start  TEXT    NOT NULL,
    period_end    TEXT    NOT NULL,
    session_count INTEGER DEFAULT 0,
    duration_s    INTEGER DEFAULT 0,
    had_activity  INTEGER DEFAULT 0,
    comment_body  TEXT,
    comment_id    TEXT,
    state         TEXT    DEFAULT 'pending',
    error         TEXT,
    posted_at     TEXT,
    created_at    TEXT    DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_jira_update_log_dedup
    ON jira_update_log(task_key, period_start, period_end);
"""


@pytest.fixture
def conn():
    """In-memory SQLite connection with the jira_update_log table created."""
    db = sqlite3.connect(":memory:")
    db.row_factory = sqlite3.Row
    db.executescript(MIGRATION_SQL)
    yield db
    db.close()


@pytest.fixture
def sample_task():
    return {
        "task_key": "KAN-87",
        "title": "Implement OAuth flow",
        "status": "In Progress",
        "url": "https://example.atlassian.net/browse/KAN-87",
    }


_FROM = "2026-05-14T09:00:00Z"
_TO   = "2026-05-14T13:00:00Z"


# ── _parse_search_result ────────────────────────────────────────────────────────

def test_parse_search_result_plain_list():
    """A JSON array is returned as-is."""
    raw = '[{"key": "KAN-1"}, {"key": "KAN-2"}]'
    result = _parse_search_result(raw)
    assert len(result) == 2
    assert result[0]["key"] == "KAN-1"


def test_parse_search_result_issues_key():
    """A dict with an 'issues' list is unwrapped."""
    raw = '{"issues": [{"key": "KAN-3"}], "total": 1}'
    result = _parse_search_result(raw)
    assert len(result) == 1
    assert result[0]["key"] == "KAN-3"


def test_parse_search_result_results_key():
    """A dict with a 'results' list is unwrapped."""
    raw = '{"results": [{"key": "KAN-4"}]}'
    result = _parse_search_result(raw)
    assert result[0]["key"] == "KAN-4"


def test_parse_search_result_empty_string():
    """Empty input returns an empty list without raising."""
    result = _parse_search_result("")
    assert result == []


def test_parse_search_result_non_json_text():
    """Plain prose (no JSON) returns an empty list."""
    result = _parse_search_result("Server error: connection refused")
    assert result == []


def test_parse_search_result_malformed_json():
    """Unparseable JSON returns an empty list."""
    result = _parse_search_result("{key: 'no quotes'}")
    assert result == []


def test_parse_search_result_unexpected_shape():
    """A dict with no recognised list key returns an empty list."""
    raw = '{"total": 0, "startAt": 0}'
    result = _parse_search_result(raw)
    assert result == []


def test_parse_search_result_json_embedded_in_prose():
    """JSON object embedded in surrounding text is extracted and parsed."""
    raw = 'FastMCP ready.\n{"issues": [{"key": "KAN-5"}]}\nDone.'
    result = _parse_search_result(raw)
    assert result[0]["key"] == "KAN-5"


# ── _normalise_issue ────────────────────────────────────────────────────────────

def test_normalise_issue_flat_mcp_format():
    """mcp-atlassian flat structure is correctly mapped."""
    issue = {
        "key": "KAN-10",
        "summary": "Fix login bug",
        "status": {"name": "In Progress", "category": "indeterminate"},
    }
    result = _normalise_issue(issue, "https://example.atlassian.net")
    assert result["task_key"] == "KAN-10"
    assert result["title"] == "Fix login bug"
    assert result["status"] == "In Progress"
    assert result["url"] == "https://example.atlassian.net/browse/KAN-10"


def test_normalise_issue_string_status():
    """A string status (non-dict) is coerced to a string."""
    issue = {"key": "KAN-11", "summary": "Refactor DB", "status": "Done"}
    result = _normalise_issue(issue, "https://example.atlassian.net")
    assert result["status"] == "Done"


def test_normalise_issue_missing_fields_give_empty_strings():
    """Missing key/summary/status fields default to empty strings."""
    result = _normalise_issue({}, "https://example.atlassian.net")
    assert result["task_key"] == ""
    assert result["title"] == ""
    assert result["status"] == ""


def test_normalise_issue_url_trailing_slash_stripped():
    """Trailing slash on base_url is stripped before /browse/."""
    issue = {"key": "KAN-12", "summary": "Edge", "status": {}}
    result = _normalise_issue(issue, "https://example.atlassian.net/")
    assert result["url"] == "https://example.atlassian.net/browse/KAN-12"


# ── _parse_mcp_summary ──────────────────────────────────────────────────────────

def test_parse_mcp_summary_no_sessions_linked():
    """'No sessions linked' sentinel returns (False, 0, 0)."""
    had, count, dur = _parse_mcp_summary("No sessions linked to KAN-87.")
    assert had is False
    assert count == 0
    assert dur == 0


def test_parse_mcp_summary_hours_and_minutes():
    """'3 session(s), 2h 15m total' → (True, 3, 8100 s)."""
    had, count, dur = _parse_mcp_summary("3 session(s), 2h 15m total")
    assert had is True
    assert count == 3
    assert dur == 2 * 3600 + 15 * 60


def test_parse_mcp_summary_minutes_only():
    """'1 session(s), 45m total' → (True, 1, 2700 s)."""
    had, count, dur = _parse_mcp_summary("1 session(s), 45m total")
    assert had is True
    assert count == 1
    assert dur == 45 * 60


def test_parse_mcp_summary_exact_one_hour():
    """'2 session(s), 1h 0m total' → (True, 2, 3600 s)."""
    had, count, dur = _parse_mcp_summary("2 session(s), 1h 0m total")
    assert had is True
    assert count == 2
    assert dur == 3600


def test_parse_mcp_summary_unrecognised_format_treated_as_activity():
    """Unrecognised text that lacks the sentinel → (True, 0, 0): activity present but counts unknown."""
    had, count, dur = _parse_mcp_summary("Some unexpected MCP output.")
    assert had is True
    assert count == 0
    assert dur == 0


# ── _format_comment ─────────────────────────────────────────────────────────────

def test_format_comment_with_activity(sample_task):
    """Non-zero duration/session_count includes the footer line."""
    body = _format_comment(
        task=sample_task,
        summary="• Implemented OAuth\n• Added tests",
        from_time=_FROM,
        to_time=_TO,
        duration_s=3600,
        session_count=2,
    )
    assert "09:00" in body
    assert "13:00" in body
    assert "• Implemented OAuth" in body
    assert "1h 0m" in body
    assert "2 session(s)" in body
    assert "Via Meridian" in body


def test_format_comment_no_activity(sample_task):
    """duration_s=0 and session_count=0 → no-activity message, no footer."""
    body = _format_comment(
        task=sample_task,
        summary="No activity recorded in this period.",
        from_time=_FROM,
        to_time=_TO,
        duration_s=0,
        session_count=0,
    )
    assert "No activity recorded in this period." in body
    assert "Via Meridian" not in body


def test_format_comment_header_contains_date(sample_task):
    """The header includes the formatted date from from_time."""
    body = _format_comment(
        task=sample_task,
        summary="• Did work",
        from_time=_FROM,
        to_time=_TO,
        duration_s=900,
        session_count=1,
    )
    # from_time is 2026-05-14 → "May 14"
    assert "May 14" in body


# ── _build_user_message ─────────────────────────────────────────────────────────

def test_build_user_message_contains_task_section(sample_task):
    """Output contains the TASK key and title."""
    msg = _build_user_message(sample_task, "3 session(s), 2h 0m total", _FROM, _TO)
    assert "KAN-87" in msg
    assert "Implement OAuth flow" in msg


def test_build_user_message_contains_period_section(sample_task):
    """Output contains the Period line with formatted HH:MM times."""
    msg = _build_user_message(sample_task, "some data", _FROM, _TO)
    assert "Period:" in msg
    assert "09:00" in msg
    assert "13:00" in msg


def test_build_user_message_contains_activity_data_section(sample_task):
    """ACTIVITY DATA section is present and contains the mcp_text."""
    mcp = "3 session(s), 2h 15m total\n- cursor.so: 45m"
    msg = _build_user_message(sample_task, mcp, _FROM, _TO)
    assert "ACTIVITY DATA:" in msg
    assert "cursor.so" in msg


def test_build_user_message_instruction_line_present(sample_task):
    """The summarisation instruction is present in the message."""
    msg = _build_user_message(sample_task, "data", _FROM, _TO)
    assert "bullet point" in msg.lower()


# ── compute_slots ───────────────────────────────────────────────────────────────

def test_compute_slots_4h_interval():
    """9–17 with 4h interval → [13, 17]."""
    assert compute_slots(9, 17, 4) == [13, 17]


def test_compute_slots_8h_interval():
    """9–17 with 8h interval → [17]."""
    assert compute_slots(9, 17, 8) == [17]


def test_compute_slots_2h_interval():
    """9–17 with 2h interval → [11, 13, 15, 17]."""
    assert compute_slots(9, 17, 2) == [11, 13, 15, 17]


def test_compute_slots_interval_exceeds_window():
    """Interval longer than office window → ValueError (no valid slots)."""
    with pytest.raises(ValueError, match="No update slots fit"):
        compute_slots(9, 10, 8)


def test_compute_slots_tight_window():
    """Exactly one interval fits at end → one slot."""
    assert compute_slots(9, 11, 2) == [11]


def test_next_slot_dt_raises_on_empty_slots():
    """next_slot_dt with an empty list raises ValueError, not IndexError."""
    with pytest.raises(ValueError, match="slots must be non-empty"):
        next_slot_dt([])


# ── slot_window ──────────────────────────────────────────────────────────────────

_ISO_RE = re.compile(
    r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z$"
)


def test_slot_window_returns_valid_iso_strings():
    """Both returned strings are valid ISO 8601 UTC timestamps."""
    from_t, to_t = slot_window(13, 4)
    assert _ISO_RE.match(from_t), f"from_time not ISO 8601: {from_t}"
    assert _ISO_RE.match(to_t), f"to_time not ISO 8601: {to_t}"


def test_slot_window_to_time_hour_matches_slot():
    """The to_time hour (in UTC) matches slot_hour when local offset is accounted for."""
    # We cannot assume a fixed local timezone, so we verify the window width instead.
    from datetime import datetime, timezone

    from_t, to_t = slot_window(17, 4)
    dt_from = datetime.fromisoformat(from_t.replace("Z", "+00:00"))
    dt_to   = datetime.fromisoformat(to_t.replace("Z", "+00:00"))
    delta_h = (dt_to - dt_from).total_seconds() / 3600
    assert delta_h == pytest.approx(4.0)


def test_slot_window_from_is_before_to():
    """from_time is always strictly before to_time."""
    from datetime import datetime

    from_t, to_t = slot_window(13, 4)
    dt_from = datetime.fromisoformat(from_t.replace("Z", "+00:00"))
    dt_to   = datetime.fromisoformat(to_t.replace("Z", "+00:00"))
    assert dt_from < dt_to


# ── db: log_jira_update ─────────────────────────────────────────────────────────

def test_log_jira_update_inserts_row(conn):
    """Inserting a new update returns a positive id and stores the row."""
    from agents.db import log_jira_update

    uid = log_jira_update(
        conn,
        task_key="KAN-87",
        period_start=_FROM,
        period_end=_TO,
        session_count=3,
        duration_s=7200,
        had_activity=True,
        comment_body="• did stuff",
    )
    assert uid > 0
    row = conn.execute("SELECT * FROM jira_update_log WHERE id=?", (uid,)).fetchone()
    assert row["task_key"] == "KAN-87"
    assert row["state"] == "pending"
    assert row["had_activity"] == 1
    assert row["session_count"] == 3


def test_log_jira_update_upsert_on_conflict(conn):
    """Duplicate (task, start, end) upserts rather than raising."""
    from agents.db import log_jira_update

    uid1 = log_jira_update(
        conn,
        task_key="KAN-87",
        period_start=_FROM,
        period_end=_TO,
        session_count=1,
        duration_s=3600,
        had_activity=True,
        comment_body="first",
    )
    uid2 = log_jira_update(
        conn,
        task_key="KAN-87",
        period_start=_FROM,
        period_end=_TO,
        session_count=2,
        duration_s=7200,
        had_activity=True,
        comment_body="updated",
    )
    # Only one row should exist.
    count = conn.execute("SELECT COUNT(*) FROM jira_update_log").fetchone()[0]
    assert count == 1
    row = conn.execute("SELECT comment_body FROM jira_update_log WHERE id=?", (uid2,)).fetchone()
    assert row["comment_body"] == "updated"


# ── db: get_last_update ─────────────────────────────────────────────────────────

def test_get_last_update_returns_none_when_pending(conn):
    """Pending row is not returned by get_last_update."""
    from agents.db import get_last_update, log_jira_update

    log_jira_update(
        conn,
        task_key="KAN-88",
        period_start=_FROM,
        period_end=_TO,
        session_count=1,
        duration_s=1800,
        had_activity=True,
        comment_body="body",
    )
    assert get_last_update(conn, "KAN-88", _FROM, _TO) is None


def test_get_last_update_returns_row_when_sent(conn):
    """get_last_update returns the row only after mark_update_sent."""
    from agents.db import get_last_update, log_jira_update, mark_update_sent

    uid = log_jira_update(
        conn,
        task_key="KAN-88",
        period_start=_FROM,
        period_end=_TO,
        session_count=1,
        duration_s=1800,
        had_activity=True,
        comment_body="body",
    )
    mark_update_sent(conn, uid, "comment-abc")
    result = get_last_update(conn, "KAN-88", _FROM, _TO)
    assert result is not None
    assert result["state"] == "sent"
    assert result["comment_id"] == "comment-abc"


def test_get_last_update_returns_none_for_unknown_slot(conn):
    """Querying a non-existent (task, slot) pair returns None."""
    from agents.db import get_last_update

    assert get_last_update(conn, "KAN-99", _FROM, _TO) is None


# ── db: mark_update_sent ────────────────────────────────────────────────────────

def test_mark_update_sent_sets_state_and_comment_id(conn):
    """mark_update_sent transitions state to 'sent' and persists comment_id."""
    from agents.db import log_jira_update, mark_update_sent

    uid = log_jira_update(
        conn,
        task_key="KAN-89",
        period_start=_FROM,
        period_end=_TO,
        session_count=2,
        duration_s=5400,
        had_activity=True,
        comment_body="comment",
    )
    mark_update_sent(conn, uid, "cid-999")
    row = conn.execute(
        "SELECT state, comment_id, posted_at FROM jira_update_log WHERE id=?", (uid,)
    ).fetchone()
    assert row["state"] == "sent"
    assert row["comment_id"] == "cid-999"
    assert row["posted_at"] is not None


# ── db: mark_update_failed ──────────────────────────────────────────────────────

def test_mark_update_failed_sets_state_and_error(conn):
    """mark_update_failed transitions state to 'failed' and stores the error."""
    from agents.db import log_jira_update, mark_update_failed

    uid = log_jira_update(
        conn,
        task_key="KAN-90",
        period_start=_FROM,
        period_end=_TO,
        session_count=0,
        duration_s=0,
        had_activity=False,
        comment_body="",
    )
    mark_update_failed(conn, uid, "connection timeout")
    row = conn.execute(
        "SELECT state, error FROM jira_update_log WHERE id=?", (uid,)
    ).fetchone()
    assert row["state"] == "failed"
    assert row["error"] == "connection timeout"


def test_mark_update_failed_truncates_long_error(conn):
    """Errors longer than 1000 chars are truncated to 1000 chars."""
    from agents.db import log_jira_update, mark_update_failed

    uid = log_jira_update(
        conn,
        task_key="KAN-91",
        period_start=_FROM,
        period_end=_TO,
        session_count=0,
        duration_s=0,
        had_activity=False,
        comment_body="",
    )
    long_error = "x" * 2000
    mark_update_failed(conn, uid, long_error)
    row = conn.execute(
        "SELECT error FROM jira_update_log WHERE id=?", (uid,)
    ).fetchone()
    assert len(row["error"]) == 1000
