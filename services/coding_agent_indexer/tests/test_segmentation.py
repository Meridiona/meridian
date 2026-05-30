"""Edge-case tests for coding_agent_indexer segmentation + sealing.

These cover the invariants the product depends on:
  * segments split on >1h idle gaps (strictly greater)
  * a non-last segment is always sealed; the last seals on end-or-idle
  * the poll seal-sweep settles open rows without re-parsing
  * a SEALED row is immutable; content after the sealed high-water always
    lands in a NEW segment (the resume-after-SessionEnd case)
  * idempotent re-registration (hook + poll races)
  * active_seconds is gap-capped; transcript carries timestamps

Run:
    cd services
    .venv/bin/python -m pytest coding_agent_indexer/tests/test_segmentation.py -v
"""
from __future__ import annotations

import glob
import json
import sqlite3
from datetime import datetime, timedelta, timezone
from pathlib import Path

import pytest

from coding_agent_indexer import config, db, register
from coding_agent_indexer.jsonl_meta import iso_utc, parse_session_segments

BASE = datetime(2026, 5, 20, 8, 0, 0, tzinfo=timezone.utc)
_MIGRATIONS = Path(__file__).resolve().parents[3] / "src" / "migrations"


def _iso(dt: datetime) -> str:
    """Source 'ms + Z' shape that Claude/Codex JSONLs actually write — used to
    build input records. The parser normalises this to the canonical iso_utc()
    ('µs + +00:00') for storage, which is what output assertions compare against.
    """
    u = dt.astimezone(timezone.utc)
    return u.strftime("%Y-%m-%dT%H:%M:%S.") + f"{u.microsecond // 1000:03d}Z"


def _rec(offset_s: int, role: str, text: str) -> dict:
    """One Claude Code JSONL record `offset_s` seconds after BASE."""
    return {
        "type": role,                       # 'user' | 'assistant'
        "timestamp": _iso(BASE + timedelta(seconds=offset_s)),
        "cwd": "/work/proj",
        "message": {"role": role, "content": text},
    }


def _write_jsonl(path: Path, records: list[dict]) -> None:
    path.write_text("\n".join(json.dumps(r) for r in records) + "\n")


def _make_db(tmp_path: Path) -> Path:
    """Build a real meridian.db by applying every migration in order."""
    dbp = tmp_path / "meridian.db"
    con = sqlite3.connect(dbp)
    for m in sorted(glob.glob(str(_MIGRATIONS / "*.sql"))):
        con.executescript(Path(m).read_text())
    con.commit()
    con.close()
    return dbp


@pytest.fixture
def live_db(tmp_path, monkeypatch):
    """Point the indexer at a fresh migrated DB and stub host detection."""
    dbp = _make_db(tmp_path)
    monkeypatch.setattr(config, "MERIDIAN_DB", dbp)
    monkeypatch.setattr(
        "coding_agent_indexer.host_app.detect_host_app", lambda: "TestHost"
    )
    return dbp


def _claude_jsonl(tmp_path: Path, uuid: str, records: list[dict]) -> Path:
    """Place a JSONL under a .claude/projects layout so agent inference works."""
    d = tmp_path / ".claude" / "projects" / "-work-proj"
    d.mkdir(parents=True, exist_ok=True)
    p = d / f"{uuid}.jsonl"
    _write_jsonl(p, records)
    return p


def _rows(dbp: Path, uuid: str) -> list[sqlite3.Row]:
    con = sqlite3.connect(dbp)
    con.row_factory = sqlite3.Row
    rows = con.execute(
        "SELECT * FROM app_sessions WHERE claude_session_uuid=? ORDER BY segment_started_at",
        (uuid,),
    ).fetchall()
    con.close()
    return rows


# ──────────────────────── Parser-level segmentation ─────────────────────────────


def test_single_continuous_segment(tmp_path):
    p = _claude_jsonl(tmp_path, "u1", [
        _rec(0, "user", "hi"),
        _rec(60, "assistant", "working"),
        _rec(120, "user", "thanks"),
    ])
    _meta, segs = parse_session_segments(p)
    assert len(segs) == 1
    assert segs[0].user_turns == 2 and segs[0].assistant_turns == 1
    assert segs[0].is_last is True
    assert segs[0].segment_started_at == iso_utc(BASE)


def test_split_on_gap_over_threshold(tmp_path):
    # 2h gap (7200s > 3600s) → two segments.
    p = _claude_jsonl(tmp_path, "u2", [
        _rec(0, "user", "morning work"),
        _rec(60, "assistant", "done"),
        _rec(60 + 7200, "user", "afternoon work"),
        _rec(60 + 7200 + 30, "assistant", "done2"),
    ])
    _meta, segs = parse_session_segments(p)
    assert len(segs) == 2
    assert segs[0].is_last is False and segs[1].is_last is True
    assert segs[0].segment_started_at == iso_utc(BASE)
    assert segs[1].segment_started_at == iso_utc(BASE + timedelta(seconds=7260))


def test_no_split_at_exact_threshold(tmp_path):
    # Gap of EXACTLY 3600s must NOT gap-split (strictly greater splits).
    # Disable the time-box here so we test the gap rule in isolation.
    p = _claude_jsonl(tmp_path, "u3", [
        _rec(0, "user", "a"),
        _rec(3600, "assistant", "b"),
    ])
    _meta, segs = parse_session_segments(p, max_segment_seconds=0)
    assert len(segs) == 1


def test_time_box_splits_continuous_session(tmp_path):
    # Records every 10 min for 2.5h — NO >1h idle gap, so only the 1h time-box
    # splits it. Long continuous sessions become predictable hourly chunks.
    recs = [_rec(i * 600, "user" if i % 2 == 0 else "assistant", f"m{i}") for i in range(16)]
    p = _claude_jsonl(tmp_path, "tb", recs)
    _meta, segs = parse_session_segments(p, segment_gap_seconds=3600, max_segment_seconds=3600)
    assert len(segs) == 3                                    # 0–1h, 1–2h, 2–2.5h
    for s in segs:
        span = (datetime.fromisoformat(s.ended_at.replace("Z", "+00:00"))
                - datetime.fromisoformat(s.started_at.replace("Z", "+00:00"))).total_seconds()
        assert span < 3600                                   # no chunk exceeds the box
    assert segs[0].is_last is False and segs[-1].is_last is True


def test_time_box_disabled_keeps_one_segment(tmp_path):
    recs = [_rec(i * 600, "user", f"m{i}") for i in range(16)]
    p = _claude_jsonl(tmp_path, "tb0", recs)
    _meta, segs = parse_session_segments(p, segment_gap_seconds=3600, max_segment_seconds=0)
    assert len(segs) == 1                                    # box off → one continuous segment


def test_active_seconds_capped(tmp_path):
    # One 50-min gap (<1h, same segment) must be capped at 300s, not counted whole.
    p = _claude_jsonl(tmp_path, "u4", [
        _rec(0, "user", "a"),
        _rec(3000, "assistant", "b"),   # 50 min later
    ])
    _meta, segs = parse_session_segments(p)
    assert len(segs) == 1
    assert segs[0].active_seconds == config.ACTIVE_TIME_GAP_CAP_SECONDS  # 300, not 3000


def test_timestamps_in_transcript(tmp_path):
    p = _claude_jsonl(tmp_path, "u5", [_rec(0, "user", "hello world")])
    _meta, segs = parse_session_segments(p)
    assert _iso(BASE) in segs[0].transcript
    assert "[user] hello world" in segs[0].transcript


def test_empty_file_has_no_segments(tmp_path):
    p = _claude_jsonl(tmp_path, "u6", [])
    meta, segs = parse_session_segments(p)
    assert segs == [] and meta.is_valid is False


# ──────────────────────── Codex schema parsing ─────────────────────────────────


def _codex_jsonl(tmp_path, uuid, records):
    d = tmp_path / ".codex" / "sessions" / "2026" / "05" / "20"
    d.mkdir(parents=True, exist_ok=True)
    p = d / f"rollout-{uuid}.jsonl"
    _write_jsonl(p, records)
    return p


def _cx(offset_s, rtype, payload):
    return {"type": rtype, "timestamp": _iso(BASE + timedelta(seconds=offset_s)), "payload": payload}


def test_codex_records_parse_to_turns(tmp_path):
    p = _codex_jsonl(tmp_path, "cx1", [
        _cx(0,  "session_meta", {"id": "cx1", "cwd": "/work/proj"}),
        _cx(1,  "event_msg",    {"type": "user_message", "message": "add a test"}),
        _cx(5,  "response_item", {"type": "reasoning"}),                      # non-turn, still ticks time
        _cx(8,  "event_msg",    {"type": "agent_message", "message": "wrote test_x.py"}),
        _cx(60, "event_msg",    {"type": "user_message", "message": "run it"}),
        _cx(63, "event_msg",    {"type": "agent_message", "message": "3 passed"}),
    ])
    meta, segs = parse_session_segments(p, agent="codex")
    assert meta.user_turns == 2 and meta.assistant_turns == 2
    assert len(segs) == 1 and segs[0].cwd == "/work/proj"
    assert "[codex] wrote test_x.py" in segs[0].transcript
    assert _iso(BASE + timedelta(seconds=1)) in segs[0].transcript     # timestamped


def test_codex_splits_on_gap(tmp_path):
    p = _codex_jsonl(tmp_path, "cx2", [
        _cx(0,        "event_msg", {"type": "user_message", "message": "morning"}),
        _cx(30,       "event_msg", {"type": "agent_message", "message": "done"}),
        _cx(30 + 7200, "event_msg", {"type": "user_message", "message": "afternoon"}),   # >1h gap
        _cx(30 + 7230, "event_msg", {"type": "agent_message", "message": "done2"}),
    ])
    _meta, segs = parse_session_segments(p, agent="codex")
    assert len(segs) == 2


# ──────────────────────── Registration + sealing (DB) ───────────────────────────


def test_last_segment_live_until_idle_or_ended(live_db):
    p = _claude_jsonl(live_db.parent, "s1", [
        _rec(0, "user", "a"), _rec(60, "assistant", "b"),
    ])
    # Poll path, only 5 min elapsed → last segment stays LIVE.
    res = register.register_ended_session(
        p, session_ended=False, now=BASE + timedelta(minutes=5),
    )
    assert res.outcome == register.RegisterOutcome.INSERTED
    rows = _rows(live_db, "s1")
    assert len(rows) == 1
    assert rows[0]["sealed_at"] is None
    assert rows[0]["task_method"] == db.TASK_METHOD_LIVE


def test_session_ended_seals_last(live_db):
    p = _claude_jsonl(live_db.parent, "s2", [
        _rec(0, "user", "a"), _rec(60, "assistant", "b"),
    ])
    res = register.register_ended_session(
        p, session_ended=True, now=BASE + timedelta(minutes=5),
    )
    rows = _rows(live_db, "s2")
    assert len(rows) == 1
    assert rows[0]["sealed_at"] is not None
    assert rows[0]["task_method"] == db.TASK_METHOD_PENDING
    assert res.sealed_ids == [rows[0]["id"]]


def test_last_segment_seals_on_idle(live_db):
    p = _claude_jsonl(live_db.parent, "s3", [
        _rec(0, "user", "a"), _rec(60, "assistant", "b"),
    ])
    # 2h after last message, not explicitly ended → idle seal.
    register.register_ended_session(p, session_ended=False, now=BASE + timedelta(hours=2))
    rows = _rows(live_db, "s3")
    assert rows[0]["sealed_at"] is not None
    assert rows[0]["task_method"] == db.TASK_METHOD_PENDING


def test_seal_sweep_settles_open_row(live_db):
    p = _claude_jsonl(live_db.parent, "s4", [
        _rec(0, "user", "a"), _rec(60, "assistant", "b"),
    ])
    register.register_ended_session(p, session_ended=False, now=BASE + timedelta(minutes=5))
    assert _rows(live_db, "s4")[0]["sealed_at"] is None       # still live
    # Sweep 2h later seals it without re-parsing.
    n = db.seal_stale_open_rows(now_iso=_iso(BASE + timedelta(hours=2)), idle_seconds=3600)
    assert n == 1
    row = _rows(live_db, "s4")[0]
    assert row["sealed_at"] is not None
    assert row["task_method"] == db.TASK_METHOD_PENDING


def test_resume_after_seal_creates_new_segment(live_db):
    """The critical edge: SessionEnd seals, user resumes <1h later → new row,
    sealed row untouched, nothing lost."""
    p = _claude_jsonl(live_db.parent, "s5", [
        _rec(0, "user", "first"), _rec(60, "assistant", "done"),
    ])
    register.register_ended_session(p, session_ended=True, now=BASE + timedelta(minutes=2))
    sealed_row = _rows(live_db, "s5")[0]
    assert sealed_row["sealed_at"] is not None

    # Resume only 29 min after the sealed segment's end (gap < 1h).
    _write_jsonl(p, [
        _rec(0, "user", "first"), _rec(60, "assistant", "done"),
        _rec(60 + 1740, "user", "resumed"), _rec(60 + 1740 + 30, "assistant", "more"),
    ])
    register.register_ended_session(p, session_ended=False, now=BASE + timedelta(minutes=40))

    rows = _rows(live_db, "s5")
    assert len(rows) == 2, "resume must create a NEW segment, not mutate the sealed one"
    # Original sealed row is unchanged (still ends at the original last msg).
    assert rows[0]["sealed_at"] == sealed_row["sealed_at"]
    assert rows[0]["ended_at"] == sealed_row["ended_at"]
    # New row covers only the resumed turns, and is live.
    assert rows[1]["segment_started_at"] == iso_utc(BASE + timedelta(seconds=1800))
    assert rows[1]["sealed_at"] is None
    assert "resumed" in rows[1]["session_text"]
    assert "first" not in rows[1]["session_text"]            # no duplication of sealed content


def test_idempotent_reregister_live(live_db):
    """Two poll re-scans of an unchanged LIVE session → same row, no duplicate."""
    p = _claude_jsonl(live_db.parent, "s6", [
        _rec(0, "user", "a"), _rec(60, "assistant", "b"),
    ])
    r1 = register.register_ended_session(p, session_ended=False, now=BASE + timedelta(minutes=5))
    r2 = register.register_ended_session(p, session_ended=False, now=BASE + timedelta(minutes=6))
    assert r1.row_ids == r2.row_ids == [_rows(live_db, "s6")[0]["id"]]
    assert len(_rows(live_db, "s6")) == 1


def test_reregister_sealed_is_noop(live_db):
    """Re-registering an already-sealed session finds nothing new (high-water
    excludes all sealed content) and creates no duplicate."""
    p = _claude_jsonl(live_db.parent, "s6b", [
        _rec(0, "user", "a"), _rec(60, "assistant", "b"),
    ])
    register.register_ended_session(p, session_ended=True, now=BASE + timedelta(minutes=5))
    r2 = register.register_ended_session(p, session_ended=True, now=BASE + timedelta(minutes=6))
    assert r2.outcome == register.RegisterOutcome.SKIPPED_EMPTY
    assert len(_rows(live_db, "s6b")) == 1


def test_sealed_row_not_mutated_by_later_register(live_db):
    p = _claude_jsonl(live_db.parent, "s7", [
        _rec(0, "user", "a"), _rec(60, "assistant", "b"),
    ])
    register.register_ended_session(p, session_ended=True, now=BASE + timedelta(minutes=5))
    before = _rows(live_db, "s7")[0]
    # A poll re-scan of the unchanged file must not touch the sealed row.
    register.register_ended_session(p, session_ended=False, now=BASE + timedelta(hours=3))
    after = _rows(live_db, "s7")[0]
    assert dict(after) == dict(before)


if __name__ == "__main__":
    raise SystemExit(pytest.main([__file__, "-v"]))
