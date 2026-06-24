"""Unit tests for the 30-min per-ticket continuity context.

Covers the two halves of the rewritten recent-work block:
  * _fetch_recent_ticket_activity — windowing, confidence floor, candidate-gating,
    per-ticket aggregation, recency ordering.
  * _format_continuity            — rendering (none / single / multiple / recency).

Run: services/.venv/bin/pytest services/tests/test_continuity_context.py -v
(Also runnable without pytest: services/.venv/bin/python services/tests/test_continuity_context.py)
"""
from __future__ import annotations

import datetime as _dt
import sqlite3
import sys
from pathlib import Path

# Make `from agents import ...` resolve (mirror tests/evals/eval_classifier.py).
_SERVICES_DIR = Path(__file__).resolve().parent.parent
if str(_SERVICES_DIR) not in sys.path:
    sys.path.insert(0, str(_SERVICES_DIR))

from agents import mlx_classifier as rtl  # noqa: E402
from agents import _prompts  # noqa: E402

ANCHOR = "2026-06-17T10:00:00+00:00"


def _at(minutes_before: int) -> str:
    base = _dt.datetime.fromisoformat(ANCHOR)
    return (base - _dt.timedelta(minutes=minutes_before)).isoformat()


def _make_con(rows: list[dict]) -> sqlite3.Connection:
    """In-memory meridian DB with the columns _fetch_recent_ticket_activity reads."""
    con = sqlite3.connect(":memory:")
    con.row_factory = sqlite3.Row
    con.execute(
        "CREATE TABLE app_sessions ("
        " id INTEGER PRIMARY KEY AUTOINCREMENT,"
        " task_key TEXT, started_at TEXT, ended_at TEXT, duration_s REAL,"
        " task_confidence REAL, task_session_type TEXT)"
    )
    for r in rows:
        con.execute(
            "INSERT INTO app_sessions"
            " (task_key, started_at, ended_at, duration_s, task_confidence, task_session_type)"
            " VALUES (?, ?, ?, ?, ?, ?)",
            (
                r.get("task_key"),
                r["started_at"],
                r.get("ended_at"),
                r.get("duration_s", 0.0),
                r.get("task_confidence", 0.9),
                r.get("task_session_type", "task"),
            ),
        )
    con.commit()
    return con


def _keys(activity):
    return [a["task_key"] for a in activity]


# ── _fetch_recent_ticket_activity ────────────────────────────────────────────

def test_aggregates_per_ticket_and_orders_by_recency():
    con = _make_con([
        # KAN-1: two sessions, most-recent ends 2 min before anchor
        {"task_key": "KAN-1", "started_at": _at(10), "ended_at": _at(9),  "duration_s": 300},
        {"task_key": "KAN-1", "started_at": _at(3),  "ended_at": _at(2),  "duration_s": 120},
        # KAN-2: one session, ends 20 min before anchor
        {"task_key": "KAN-2", "started_at": _at(21), "ended_at": _at(20), "duration_s": 600},
    ])
    out = rtl._fetch_recent_ticket_activity(con, ANCHOR, ["KAN-1", "KAN-2"])
    assert _keys(out) == ["KAN-1", "KAN-2"]          # most-recently-active first
    k1 = out[0]
    assert k1["sessions"] == 2
    assert k1["total_s"] == 420.0                    # 300 + 120 summed
    assert abs(k1["ago_s"] - 120) < 1                # last active ~2 min ago
    assert abs(out[1]["ago_s"] - 1200) < 1           # KAN-2 ~20 min ago


def test_excludes_sessions_outside_the_window():
    con = _make_con([
        {"task_key": "KAN-1", "started_at": _at(5),  "ended_at": _at(4),  "duration_s": 60},
        {"task_key": "KAN-9", "started_at": _at(40), "ended_at": _at(39), "duration_s": 60},  # >30 min
    ])
    out = rtl._fetch_recent_ticket_activity(con, ANCHOR, ["KAN-1", "KAN-9"])
    assert _keys(out) == ["KAN-1"]


def test_excludes_below_confidence_floor():
    con = _make_con([
        {"task_key": "KAN-1", "started_at": _at(5), "ended_at": _at(4), "duration_s": 60, "task_confidence": 0.9},
        {"task_key": "KAN-2", "started_at": _at(5), "ended_at": _at(4), "duration_s": 60, "task_confidence": 0.5},
    ])
    out = rtl._fetch_recent_ticket_activity(con, ANCHOR, ["KAN-1", "KAN-2"])
    assert _keys(out) == ["KAN-1"]


def test_candidate_gating_drops_non_candidate_tickets():
    con = _make_con([
        {"task_key": "KAN-1", "started_at": _at(5), "ended_at": _at(4), "duration_s": 60},
        {"task_key": "KAN-7", "started_at": _at(5), "ended_at": _at(4), "duration_s": 60},  # not a candidate
    ])
    out = rtl._fetch_recent_ticket_activity(con, ANCHOR, ["KAN-1"])
    assert _keys(out) == ["KAN-1"]


def test_excludes_untracked_and_null_task():
    con = _make_con([
        {"task_key": None,    "started_at": _at(5), "ended_at": _at(4), "duration_s": 60, "task_session_type": "untracked"},
        {"task_key": "KAN-1", "started_at": _at(5), "ended_at": _at(4), "duration_s": 60, "task_session_type": "task"},
    ])
    out = rtl._fetch_recent_ticket_activity(con, ANCHOR, ["KAN-1"])
    assert _keys(out) == ["KAN-1"]


def test_no_candidates_returns_empty():
    con = _make_con([{"task_key": "KAN-1", "started_at": _at(5), "ended_at": _at(4), "duration_s": 60}])
    assert rtl._fetch_recent_ticket_activity(con, ANCHOR, []) == []


def test_no_anchor_returns_empty():
    con = _make_con([{"task_key": "KAN-1", "started_at": _at(5), "ended_at": _at(4), "duration_s": 60}])
    assert rtl._fetch_recent_ticket_activity(con, "", ["KAN-1"]) == []


# ── _format_continuity ───────────────────────────────────────────────────────

def test_format_empty_is_explicit_no_work_line():
    out = _prompts._format_continuity([])
    assert out.strip() == "(no tracked work in this window)"
    assert out != ""  # explicit, never silent


def test_format_single_ticket_recent():
    out = _prompts._format_continuity(
        [{"task_key": "KAN-1", "total_s": 420, "sessions": 2, "ago_s": 30}]
    )
    assert "KAN-1" in out
    assert "~7 min" in out
    assert "2 sessions" in out
    assert "just before this session" in out  # ago < 60s


def test_format_recency_minutes():
    out = _prompts._format_continuity(
        [{"task_key": "KAN-2", "total_s": 600, "sessions": 1, "ago_s": 1200}]
    )
    assert "1 session" in out
    assert "~20 min before this session" in out


def test_format_multiple_tickets_one_bullet_each():
    out = _prompts._format_continuity([
        {"task_key": "KAN-1", "total_s": 420, "sessions": 2, "ago_s": 30},
        {"task_key": "KAN-2", "total_s": 600, "sessions": 1, "ago_s": 1200},
    ])
    assert out.count("•") == 2
    assert "KAN-1" in out and "KAN-2" in out


def test_build_user_message_includes_block_when_activity_present():
    msg = _prompts.build_user_message(
        {"app_name": "Code", "session_text": "x"},
        [{"task_key": "KAN-1", "title": "t", "description_text": "d"}],
        recent_activity=[{"task_key": "KAN-1", "total_s": 60, "sessions": 1, "ago_s": 30}],
        now_iso=ANCHOR,
    )
    assert "RECENT WORK CONTEXT" in msg
    assert "WEAK" in msg          # framed as a weak prior
    assert "KAN-1" in msg


def test_build_user_message_shows_explicit_block_when_no_activity():
    msg = _prompts.build_user_message(
        {"app_name": "Code", "session_text": "x"},
        [{"task_key": "KAN-1", "title": "t", "description_text": "d"}],
        recent_activity=[],
        now_iso=ANCHOR,
    )
    assert "RECENT WORK CONTEXT" in msg            # always present now
    assert "no tracked work in this window" in msg  # explicit empty state


# ── plain-python runner (no pytest needed) ───────────────────────────────────

if __name__ == "__main__":
    fns = [v for k, v in sorted(globals().items()) if k.startswith("test_") and callable(v)]
    passed = 0
    for fn in fns:
        try:
            fn()
            print(f"  PASS  {fn.__name__}")
            passed += 1
        except Exception as exc:  # noqa: BLE001
            print(f"  FAIL  {fn.__name__}: {exc!r}")
    print(f"\n{passed}/{len(fns)} passed")
    raise SystemExit(0 if passed == len(fns) else 1)
