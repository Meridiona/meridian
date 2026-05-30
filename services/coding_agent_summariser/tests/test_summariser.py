"""Edge-case tests for the summariser. Claude + MLX are mocked — no live calls.

Covers the contract the daemon relies on:
  * success writes the prose + flips task_method to 'summarised'
  * idempotent / crash-safe write (WHERE session_summary IS NULL)
  * rate-limit → MLX fallback (and the rate_limited flag for daemon backoff)
  * non-rate Claude failure also falls back to MLX
  * both engines failing leaves the row NULL (retried later)
  * prior-burst summary is fed as context (chaining)
  * transcript is capped; queue filters to sealed/unsummarised/non-empty
  * --dry-run never writes

Run:
    cd services
    .venv/bin/python -m pytest coding_agent_summariser/tests/test_summariser.py -v
"""
from __future__ import annotations

import glob
import sqlite3
from pathlib import Path

import pytest

from coding_agent_summariser import claude_runner, codex_runner, config, db, mlx_fallback, summariser
from coding_agent_summariser.claude_runner import RateLimited, SummariserError

_MIGRATIONS = Path(__file__).resolve().parents[3] / "src" / "migrations"


def _make_db(tmp_path: Path) -> Path:
    dbp = tmp_path / "meridian.db"
    con = sqlite3.connect(dbp)
    for m in sorted(glob.glob(str(_MIGRATIONS / "*.sql"))):
        con.executescript(Path(m).read_text())
    con.commit(); con.close()
    return dbp


def _insert(dbp: Path, *, uuid: str, seg_start: str, ended: str, text: str,
            sealed: bool = True, summary=None, task_method="pending_summariser",
            app: str = "Claude Code", day: str = "2026-05-20", frames: int = 5) -> int:
    con = sqlite3.connect(dbp)
    cur = con.execute(
        """
        INSERT INTO app_sessions (
            app_name, started_at, ended_at, duration_s,
            window_titles, min_frame_id, max_frame_id, frame_count, etl_run_id,
            idle_frame_count, category, confidence, category_method,
            session_text, session_text_source, task_method, session_summary,
            claude_session_uuid, segment_started_at, sealed_at
        ) VALUES (?,?,?,?, ?,?,?,?,?, ?,?,?,?, ?,?,?,?, ?,?,?)
        """,
        (app, seg_start, ended, 100,
         "[]", 0, 0, frames, 0,
         0, "coding", 1.0, "coding_agent_indexer",
         text, "claude_jsonl", task_method, summary,
         uuid, seg_start, (ended if sealed else None)),
    )
    con.commit(); rid = cur.lastrowid; con.close()
    return rid


@pytest.fixture
def dbp(tmp_path, monkeypatch):
    p = _make_db(tmp_path)
    monkeypatch.setattr(config, "MERIDIAN_DB", p)
    db.ensure_schema(db_path=p)          # add summary_source (not in the Rust migrations)
    # Neutralise the noise filter for routing/idempotency tests (they use tiny
    # text on purpose); the threshold itself is exercised by its own test.
    monkeypatch.setattr(config, "MIN_TURNS", 0)
    monkeypatch.setattr(config, "MIN_TEXT_BYTES", 0)
    return p


def _only_row(dbp, rid):
    con = sqlite3.connect(dbp); con.row_factory = sqlite3.Row
    r = con.execute("SELECT session_summary, task_method, summary_source FROM app_sessions WHERE id=?", (rid,)).fetchone()
    con.close(); return r


# ──────────────────────── DB queue / write ─────────────────────────────────────


def test_queue_filters(dbp):
    good = _insert(dbp, uuid="a", seg_start="2026-05-20T08:00:00Z", ended="2026-05-20T08:10:00Z", text="work")
    _insert(dbp, uuid="b", seg_start="2026-05-20T09:00:00Z", ended="2026-05-20T09:05:00Z", text="x", sealed=False)        # unsealed
    _insert(dbp, uuid="c", seg_start="2026-05-20T10:00:00Z", ended="2026-05-20T10:05:00Z", text="x", summary="done")       # already summarised
    _insert(dbp, uuid="d", seg_start="2026-05-20T11:00:00Z", ended="2026-05-20T11:05:00Z", text="")                        # empty text
    rows = db.fetch_pending(50, db_path=dbp)
    assert [r.id for r in rows] == [good]


def test_day_scopes_the_queue(dbp):
    a = _insert(dbp, uuid="a", seg_start="2026-05-20T08:00:00Z", ended="2026-05-20T08:10:00Z",
                text="day20", day="2026-05-20")
    _insert(dbp, uuid="b", seg_start="2026-05-21T08:00:00Z", ended="2026-05-21T08:10:00Z",
            text="day21", day="2026-05-21")
    assert [r.id for r in db.fetch_pending(50, day="2026-05-20", db_path=dbp)] == [a]
    assert len(db.fetch_pending(50, day="2026-05-21", db_path=dbp)) == 1
    assert len(db.fetch_pending(50, db_path=dbp)) == 2          # no day → all (CLI/daemon always pass one)


def test_skips_trivial_sessions(dbp, monkeypatch):
    monkeypatch.setattr(config, "MIN_TURNS", 2)
    monkeypatch.setattr(config, "MIN_TEXT_BYTES", 800)
    real = _insert(dbp, uuid="big", seg_start="2026-05-20T08:00:00Z", ended="2026-05-20T08:30:00Z",
                   text="x" * 1000, frames=8)
    _insert(dbp, uuid="tiny", seg_start="2026-05-20T09:00:00Z", ended="2026-05-20T09:01:00Z",
            text="You've hit your limit", frames=2)             # < 800 bytes → noise
    _insert(dbp, uuid="fewturns", seg_start="2026-05-20T10:00:00Z", ended="2026-05-20T10:05:00Z",
            text="x" * 1000, frames=1)                           # < 2 turns → noise
    assert [r.id for r in db.fetch_pending(50, db_path=dbp)] == [real]


def test_write_is_idempotent(dbp):
    rid = _insert(dbp, uuid="a", seg_start="2026-05-20T08:00:00Z", ended="2026-05-20T08:10:00Z", text="w")
    assert db.write_summary(rid, "first", db_path=dbp) is True
    assert db.write_summary(rid, "second", db_path=dbp) is False     # already set → noop
    assert _only_row(dbp, rid)["session_summary"] == "first"


# ──────────────────────── summarise_one engine routing ─────────────────────────


def test_success_writes_and_flips_method(dbp, monkeypatch):
    rid = _insert(dbp, uuid="a", seg_start="2026-05-20T08:00:00Z", ended="2026-05-20T08:10:00Z", text="fixed a bug")
    monkeypatch.setattr(claude_runner, "run_claude", lambda *_a, **_k: {"summary": "Fixed the bug.", "blockers": []})
    out = summariser.summarise_one(db.fetch_pending(1, db_path=dbp)[0], db_path=dbp)
    assert out.written and out.source.value == "claude"
    row = _only_row(dbp, rid)
    assert row["session_summary"] == "Fixed the bug."
    assert row["task_method"] == config.TASK_METHOD_SUMMARISED
    assert row["summary_source"] == "claude"        # engine recorded in DB


def test_rate_limit_falls_back_to_mlx(dbp, monkeypatch):
    rid = _insert(dbp, uuid="a", seg_start="2026-05-20T08:00:00Z", ended="2026-05-20T08:10:00Z", text="w")
    def boom(*_a, **_k): raise RateLimited("usage limit")
    monkeypatch.setattr(claude_runner, "run_claude", boom)
    monkeypatch.setattr(mlx_fallback, "summarise", lambda *_a, **_k: "MLX summary.")
    out = summariser.summarise_one(db.fetch_pending(1, db_path=dbp)[0], db_path=dbp)
    assert out.written and out.source.value == "mlx" and out.rate_limited is True
    row = _only_row(dbp, rid)
    assert row["session_summary"] == "MLX summary." and row["summary_source"] == "mlx"


def test_claude_error_falls_back_to_mlx(dbp, monkeypatch):
    _insert(dbp, uuid="a", seg_start="2026-05-20T08:00:00Z", ended="2026-05-20T08:10:00Z", text="w")
    def boom(*_a, **_k): raise SummariserError("timed out")
    monkeypatch.setattr(claude_runner, "run_claude", boom)
    monkeypatch.setattr(mlx_fallback, "summarise", lambda *_a, **_k: "MLX summary.")
    out = summariser.summarise_one(db.fetch_pending(1, db_path=dbp)[0], db_path=dbp)
    assert out.written and out.source.value == "mlx" and out.rate_limited is False


def test_both_fail_leaves_null(dbp, monkeypatch):
    rid = _insert(dbp, uuid="a", seg_start="2026-05-20T08:00:00Z", ended="2026-05-20T08:10:00Z", text="w")
    monkeypatch.setattr(claude_runner, "run_claude", lambda *_a, **_k: (_ for _ in ()).throw(RateLimited("limit")))
    monkeypatch.setattr(mlx_fallback, "summarise", lambda *_a, **_k: (_ for _ in ()).throw(SummariserError("mlx down")))
    out = summariser.summarise_one(db.fetch_pending(1, db_path=dbp)[0], db_path=dbp)
    assert out.written is False and out.rate_limited is True and out.error
    assert _only_row(dbp, rid)["session_summary"] is None        # retried next tick


def test_dry_run_does_not_write(dbp, monkeypatch):
    rid = _insert(dbp, uuid="a", seg_start="2026-05-20T08:00:00Z", ended="2026-05-20T08:10:00Z", text="w")
    monkeypatch.setattr(claude_runner, "run_claude", lambda *_a, **_k: {"summary": "S", "blockers": []})
    out = summariser.summarise_one(db.fetch_pending(1, db_path=dbp)[0], write=False, db_path=dbp)
    assert out.written is False and out.summary == "S"
    assert _only_row(dbp, rid)["session_summary"] is None


# ──────────────────────── chaining + capping ───────────────────────────────────


def test_prior_summary_is_passed_as_context(dbp, monkeypatch):
    _insert(dbp, uuid="a", seg_start="2026-05-20T08:00:00Z", ended="2026-05-20T08:10:00Z",
            text="burst1", summary="Earlier burst summary.")          # already summarised, earlier
    later = _insert(dbp, uuid="a", seg_start="2026-05-20T12:00:00Z", ended="2026-05-20T12:10:00Z", text="burst2")
    captured = {}
    def spy(stdin_text, **_k):
        captured["stdin"] = stdin_text
        return {"summary": "Second burst.", "blockers": []}
    monkeypatch.setattr(claude_runner, "run_claude", spy)
    row = next(r for r in db.fetch_pending(10, db_path=dbp) if r.id == later)
    summariser.summarise_one(row, db_path=dbp)
    assert "EARLIER IN THIS SESSION" in captured["stdin"]
    assert "Earlier burst summary." in captured["stdin"]


def test_codex_session_routes_to_codex(dbp, monkeypatch):
    rid = _insert(dbp, uuid="a", seg_start="2026-05-20T08:00:00Z", ended="2026-05-20T08:10:00Z",
                  text="codex work", app="Codex")
    # claude must NOT be called for a Codex session
    monkeypatch.setattr(claude_runner, "run_claude",
                        lambda *_a, **_k: (_ for _ in ()).throw(AssertionError("claude used for codex")))
    monkeypatch.setattr(codex_runner, "run_codex", lambda *_a, **_k: {"summary": "Codex did it.", "blockers": []})
    out = summariser.summarise_one(db.fetch_pending(1, db_path=dbp)[0], db_path=dbp)
    assert out.written and out.source.value == "codex"
    row = _only_row(dbp, rid)
    assert row["session_summary"] == "Codex did it." and row["summary_source"] == "codex"


def test_codex_rate_limited_falls_back_to_mlx(dbp, monkeypatch):
    _insert(dbp, uuid="a", seg_start="2026-05-20T08:00:00Z", ended="2026-05-20T08:10:00Z",
            text="codex work", app="Codex")
    monkeypatch.setattr(codex_runner, "run_codex",
                        lambda *_a, **_k: (_ for _ in ()).throw(RateLimited("usage limit")))
    monkeypatch.setattr(mlx_fallback, "summarise", lambda *_a, **_k: "MLX covered codex.")
    out = summariser.summarise_one(db.fetch_pending(1, db_path=dbp)[0], db_path=dbp)
    assert out.written and out.source.value == "mlx" and out.rate_limited is True


def test_mlx_tail_caps_from_the_bottom(monkeypatch):
    monkeypatch.setattr(config, "MLX_INPUT_MAX_TOKENS", 10)
    monkeypatch.setattr(config, "MLX_CHARS_PER_TOKEN", 4)        # 40-char cap
    assert mlx_fallback._tail_cap("short") == "short"            # under cap → unchanged
    long = "TOP_HEAD" + "x" * 200 + "BOTTOM_END"
    out = mlx_fallback._tail_cap(long)
    assert out.endswith("BOTTOM_END")                            # kept the bottom
    assert "TOP_HEAD" not in out                                 # dropped the top
    assert "truncated" in out


def test_transcript_is_capped():
    big = "x" * (config.TRANSCRIPT_CAP_CHARS + 50_000)
    out = summariser._cap_transcript(big)
    assert len(out) < len(big)
    assert "elided" in out


if __name__ == "__main__":
    raise SystemExit(pytest.main([__file__, "-v"]))
