# meridian — normalises screenpipe activity into structured app sessions
"""End-to-end smoke test for the meridian tagger (Stages 1 + 2).

Strategy
--------
Build a temporary `meridian.db` from the canonical SQL migrations, seed it
with realistic `pm_tasks` (KAN-86/83/87/88) and a hand-crafted set of
`app_sessions` that probe specific tagger behaviours, then drive
`tagger.run_once(stages={1, 2})` against the temp DB and assert per-session
outcomes.

Stage 2 normally loads the BAAI/bge-small embedding model from
sentence-transformers. We monkeypatch `agents.embeddings.encode_batch` (and
`encode`) with a deterministic hash-based fake encoder so tests are:

  1. Portable — no model download / disk cache required in CI.
  2. Deterministic — same text always yields the same vector, so cosine
     similarities (and therefore Stage-2 routing) are reproducible.
  3. Fast — sub-second instead of sub-minute.

The fake encoder is intentionally crude: it tokenises text on word
boundaries and L2-normalises a histogram in 384-d. Tokens shared between a
session and a task description push their cosine toward 1.0; uncorrelated
text trends toward 0. That's enough to exercise Stage-2 ranking without
pretending to validate the actual bge-small model.

Stage 3 is skipped — its LLM call requires HERMES_BASE_URL + a live model
endpoint and is non-deterministic. A separate `pytest.mark.skip` stub
documents that.
"""
from __future__ import annotations

import hashlib
import importlib
import json
import os
import re
import sqlite3
from pathlib import Path

import numpy as np
import pytest


# ───────────────────────────── paths ──────────────────────────────────────────
_REPO_ROOT     = Path(__file__).resolve().parents[3]
_MIGRATIONS_DIR = _REPO_ROOT / "src" / "migrations"


# ───────────────────────────── fake embedder ──────────────────────────────────
_FAKE_DIM = 384


def _fake_encode_one(text: str) -> np.ndarray:
    """Hash-based bag-of-words encoder.

    Each token contributes to a few buckets via blake2b. The vector is then
    L2-normalised so dot products give cosines in [-1, 1] (in practice ≥ 0
    because all bucket weights are non-negative).
    """
    vec = np.zeros(_FAKE_DIM, dtype=np.float32)
    tokens = re.findall(r"[A-Za-z0-9_\-\.]+", (text or "").lower())
    if not tokens:
        # Distinct-but-empty: a single bucket so empty strings don't all
        # collapse to the same zero vector (which would max-cosine to
        # everything via the (cos+1)/2 rescale).
        vec[0] = 1.0
        return vec
    for tok in tokens:
        # Two-byte windows from a 6-byte blake2b digest → 3 buckets per token.
        # Spreading tokens across multiple buckets keeps single-overlap
        # cosines meaningful without letting any one token dominate.
        digest = hashlib.blake2b(tok.encode("utf-8"), digest_size=6).digest()
        for j in range(3):
            idx = int.from_bytes(digest[j * 2:j * 2 + 2], "little") % _FAKE_DIM
            vec[idx] += 1.0
    norm = float(np.linalg.norm(vec))
    if norm > 0.0:
        vec /= norm
    return vec


def _fake_encode_batch(texts, *, batch_size: int = 32) -> np.ndarray:
    return np.stack([_fake_encode_one(t) for t in texts], axis=0).astype(np.float32)


def _fake_encode(text: str) -> np.ndarray:
    return _fake_encode_one(text)


# ───────────────────────────── DB fixture ─────────────────────────────────────
def _apply_migrations(db_path: Path) -> None:
    """Run all SQL files in src/migrations/ in numerical order."""
    files = sorted(
        _MIGRATIONS_DIR.glob("*.sql"),
        key=lambda p: int(p.name.split("_", 1)[0]),
    )
    assert files, f"no migrations found under {_MIGRATIONS_DIR}"
    conn = sqlite3.connect(str(db_path))
    try:
        conn.execute("PRAGMA foreign_keys=ON;")
        for f in files:
            conn.executescript(f.read_text())
        conn.commit()
    finally:
        conn.close()


@pytest.fixture
def meridian_db(tmp_path, monkeypatch):
    """Build a fresh meridian.db from migrations and reload `agents.config`.

    `agents.config` reads `MERIDIAN_DB` at import time and caches it. We set
    the env var first, then `importlib.reload` on the modules that captured
    it, so subsequent calls into `agents.db` open the temp file.
    """
    db_path = tmp_path / "meridian.db"
    _apply_migrations(db_path)
    monkeypatch.setenv("MERIDIAN_DB", str(db_path))
    monkeypatch.setenv("MERIDIAN_HOME", str(tmp_path))
    monkeypatch.setenv("ONLY_TODAY", "0")
    monkeypatch.setenv("STAGE3_ENABLED", "0")
    # Make sure log dir exists for tagger._configure_logging()
    (tmp_path / "logs").mkdir(parents=True, exist_ok=True)

    # Reload modules that captured MERIDIAN_DB at import.
    import agents.config as _cfg
    importlib.reload(_cfg)
    import agents.db as _db
    importlib.reload(_db)
    # Tagger imports config + db at top level — reload it too so it picks up
    # the freshly-reloaded MERIDIAN_DB constant.
    import agents.tagger as _tagger
    importlib.reload(_tagger)

    return db_path


@pytest.fixture
def fake_embedder(monkeypatch):
    """Patch `agents.embeddings` to use the deterministic fake encoder.

    We patch both `encode` and `encode_batch` so neither code path tries to
    load sentence-transformers. The patch is module-level so any later
    `from agents.embeddings import encode_batch` re-import still resolves to
    the original module attribute (we mutate that attribute, not a local).
    """
    import agents.embeddings as emb
    monkeypatch.setattr(emb, "encode_batch", _fake_encode_batch)
    monkeypatch.setattr(emb, "encode", _fake_encode)
    # Make sure get_model is never called (would try to download weights).
    def _no_model():  # pragma: no cover
        raise AssertionError("get_model() called inside test — encoder patch missed")
    monkeypatch.setattr(emb, "get_model", _no_model)
    return emb


# ───────────────────────────── seed helpers ───────────────────────────────────
_NOW = "2026-05-10T12:00:00Z"


def _insert_pm_task(conn, *, task_key, title, description, issue_type="task", status="In Progress"):
    conn.execute(
        """
        INSERT INTO pm_tasks (task_key, provider, title, description_text,
                              status, status_category, issue_type, project_key, url,
                              updated_at, fetched_at, expires_at)
        VALUES (?, 'jira', ?, ?, ?, 'in_progress', ?, 'KAN', '',
                ?, ?, '2099-01-01T00:00:00Z')
        """,
        (task_key, title, description, status, issue_type, _NOW, _NOW),
    )


def _insert_session(
    conn,
    *,
    sid,
    app_name,
    duration_s,
    titles=None,
    ocr=None,
    audio=None,
    category="coding",
    confidence=0.8,
):
    titles_json = json.dumps([{"window_name": t, "count": 1} for t in (titles or [])])
    ocr_json    = json.dumps([{"text": t} for t in (ocr or [])])
    audio_json  = json.dumps([{"text": t} for t in (audio or [])])
    started = "2026-05-10T11:00:00Z"
    ended   = "2026-05-10T11:00:30Z"
    conn.execute(
        """
        INSERT INTO app_sessions (
            id, app_name, started_at, ended_at, duration_s,
            window_titles, ocr_samples, elements_samples, audio_snippets, signals,
            min_frame_id, max_frame_id, frame_count, etl_run_id,
            idle_frame_count, category, confidence, category_method
        ) VALUES (?, ?, ?, ?, ?,
                  ?, ?, '[]', ?, '[]',
                  1, 2, 1, 1,
                  0, ?, ?, 'rule_based')
        """,
        (sid, app_name, started, ended, duration_s,
         titles_json, ocr_json, audio_json,
         category, confidence),
    )


# Session id constants — line them up with the docstring scenarios.
SID_EMPTY            = 1   # A. duration=0, no content
SID_SUBTHIRTY        = 2   # B. 15s with content (prefilter overhead)
SID_CODE_DEFER       = 3   # C. 31s code session, no ticket → defer to Stage 2
SID_VERBATIM_KAN86   = 4   # D. KAN-86 in title — Stage 1 auto
SID_UTF8_FALSEPOS    = 5   # E. UTF-8 in OCR — denylisted
SID_MULTI_TICKET     = 6   # F. KAN-86 AND KAN-87 in titles
SID_TICKET_UNKNOWN   = 7   # G. ABC-1 — ticket-shaped, no pm_task → skip
SID_CURSOR_AI        = 8   # H. Cursor + chat panel, 500s
SID_MEETING          = 9   # I. Zoom meeting, 1200s
SID_TAILSCALE_SEM    = 10  # J. Tailscale browser, no verbatim KAN-NN


@pytest.fixture
def seeded_db(meridian_db):
    """Open the temp DB, insert pm_tasks + app_sessions fixtures, return path."""
    conn = sqlite3.connect(str(meridian_db))
    try:
        # ── pm_tasks (mirror of the user's real Jira board) ──
        _insert_pm_task(
            conn, task_key="KAN-86",
            title="Migrate active-intelligence to meridian",
            description=(
                "Migrate the active-intelligence service from hermes into the "
                "meridian repository. Move the python tagger code, port the "
                "rules-based stage 1 and embeddings stage 2, and wire them "
                "to meridian.db."
            ),
            issue_type="story",
        )
        _insert_pm_task(
            conn, task_key="KAN-83",
            title="Setup tailscale on mac studio and macbook air",
            description=(
                "Configure tailscale across the mac studio and macbook air so "
                "remote access works without a VPN. Install tailscale, log in, "
                "and verify ssh works between hosts."
            ),
            issue_type="task",
        )
        _insert_pm_task(
            conn, task_key="KAN-87",
            title="Add logging and observability",
            description=(
                "Add structured logging across the agents service and "
                "instrument key spans so we can trace tagger behaviour in "
                "production. Wire into prometheus or opentelemetry."
            ),
            issue_type="task",
        )
        _insert_pm_task(
            conn, task_key="KAN-88",
            title="Gpay expenses sheet automation",
            description=(
                "Automate the monthly google pay expenses spreadsheet — "
                "scrape the gpay activity, normalise into categories, and "
                "push to a google sheet."
            ),
            issue_type="task",
        )

        # ── app_sessions ──
        # A. Empty session — duration=0, no content. Must hit prefilter.
        _insert_session(
            conn, sid=SID_EMPTY, app_name="Finder", duration_s=0,
            titles=[], ocr=[], audio=[],
            category="idle_personal", confidence=0.4,
        )

        # B. Sub-30s with content. Prefilter overhead/skip via MIN_LLM_DURATION_S.
        _insert_session(
            conn, sid=SID_SUBTHIRTY, app_name="Code", duration_s=15,
            titles=["main.rs — meridian"],
            ocr=["fn main() { println!(\"hello\"); }"],
        )

        # C. 31s Code session with .py file → Stage 1 activity=coding, no
        #    ticket → defer to Stage 2 (which then matches against pm_tasks).
        _insert_session(
            conn, sid=SID_CODE_DEFER, app_name="Code", duration_s=31,
            titles=["tagger.py — agents"],
            ocr=[
                "def run_once(stages): pass",
                "from agents import db",
                "python pytest mypy ruff",
            ],
        )

        # D. Verbatim KAN-86 in title — Stage 1 regex auto.
        _insert_session(
            conn, sid=SID_VERBATIM_KAN86, app_name="Code", duration_s=400,
            titles=["KAN-86 migrate-active-intelligence — meridian"],
            ocr=["working on KAN-86 today"],
        )

        # E. UTF-8 false-positive — looks like a ticket key but is in the
        #    denylist (_TICKET_FALSE_POSITIVES). Stage 1 must NOT match it.
        _insert_session(
            conn, sid=SID_UTF8_FALSEPOS, app_name="Code", duration_s=120,
            titles=["main.rs — utf8 encoding test"],
            ocr=[
                "encoding: UTF-8 detected at offset 0",
                "RFC-2616 says HTTP-200 means OK",
            ],
        )

        # F. Both KAN-86 AND KAN-87 in titles — first-seen wins (extract_tickets
        #    de-dupes preserving insertion order).
        _insert_session(
            conn, sid=SID_MULTI_TICKET, app_name="Code", duration_s=200,
            titles=[
                "KAN-87 add-logging — meridian",
                "KAN-86 migrate-active-intelligence — meridian",
            ],
            ocr=["touching both tickets"],
        )

        # G. Ticket-shaped key NOT in pm_tasks (ABC-1) → ticket_links should
        #    be written with task_key=NULL, session_type='task', routing='skip'.
        _insert_session(
            conn, sid=SID_TICKET_UNKNOWN, app_name="Code", duration_s=200,
            titles=["editor"],
            ocr=["see ABC-1 for context"],
        )

        # H. Cursor + chat panel content for 500s. activity=ai_pair_programming,
        #    collaboration=ai_assisted.
        _insert_session(
            conn, sid=SID_CURSOR_AI, app_name="Cursor", duration_s=500,
            titles=["chat — Cursor"],
            ocr=[
                "You are an expert assistant. tab to accept",
                "model: claude-sonnet-4",
                "compose a refactor for this file",
            ],
        )

        # I. Long Zoom meeting — activity=meeting, collaboration=team_review.
        _insert_session(
            conn, sid=SID_MEETING, app_name="zoom.us", duration_s=1200,
            titles=["Zoom Meeting"],
            ocr=["Zoom meeting in progress — share screen", "participants: 5"],
            audio=["yeah let's discuss the migration"],
        )

        # J. Tailscale-related browser session, no verbatim KAN-NN. Should
        #    rely on Stage 2 cosine to find KAN-83.
        _insert_session(
            conn, sid=SID_TAILSCALE_SEM, app_name="Google Chrome", duration_s=300,
            titles=["tailscale admin console — Chrome"],
            ocr=[
                "Configure tailscale on macbook air and mac studio for remote ssh",
                "tailscale up --accept-routes --advertise-exit-node",
                "ssh between hosts via tailscale",
            ],
        )
        conn.commit()
    finally:
        conn.close()
    return meridian_db


# ───────────────────────────── run the tagger ─────────────────────────────────
@pytest.fixture
def run_summary(seeded_db, fake_embedder):
    """Run `tagger.run_once(stages={1, 2})` and return the summary dict.

    Imported lazily so the meridian_db fixture's reload happens BEFORE
    `tagger` captures `MERIDIAN_DB`. The fake_embedder fixture must be
    pulled in before the run so encode_batch is patched.
    """
    import agents.tagger as tagger
    return tagger.run_once(stages={1, 2}, since_iso=None)


# Lookup helpers used by the assertion tests.
def _ticket_link(db_path: Path, sid: int) -> dict | None:
    conn = sqlite3.connect(str(db_path))
    conn.row_factory = sqlite3.Row
    try:
        row = conn.execute(
            "SELECT * FROM ticket_links WHERE session_id = ?", (sid,)
        ).fetchone()
        return dict(row) if row else None
    finally:
        conn.close()


def _dims(db_path: Path, sid: int) -> dict[str, set[str]]:
    conn = sqlite3.connect(str(db_path))
    try:
        rows = conn.execute(
            "SELECT dimension, value FROM session_dimensions WHERE session_id = ?", (sid,)
        ).fetchall()
    finally:
        conn.close()
    out: dict[str, set[str]] = {}
    for d, v in rows:
        out.setdefault(d, set()).add(v)
    return out


def _agent_runs(db_path: Path) -> list[dict]:
    conn = sqlite3.connect(str(db_path))
    conn.row_factory = sqlite3.Row
    try:
        return [dict(r) for r in conn.execute("SELECT * FROM agent_runs").fetchall()]
    finally:
        conn.close()


def _dispatches(db_path: Path) -> list[dict]:
    conn = sqlite3.connect(str(db_path))
    conn.row_factory = sqlite3.Row
    try:
        return [dict(r) for r in conn.execute("SELECT * FROM dispatch_queue").fetchall()]
    finally:
        conn.close()


def _cursor(db_path: Path) -> int:
    conn = sqlite3.connect(str(db_path))
    try:
        row = conn.execute("SELECT last_session_id FROM agent_cursor WHERE id=1").fetchone()
        return int(row[0]) if row else 0
    finally:
        conn.close()


# ───────────────────────────── per-session assertions ─────────────────────────
class TestPerSession:
    def test_a_empty_session_is_overhead_skip(self, seeded_db, run_summary):
        link = _ticket_link(seeded_db, SID_EMPTY)
        assert link is not None, "empty session must still get a ticket_links row"
        assert link["session_type"] == "overhead"
        assert link["routing"] == "skip"
        assert link["task_key"] is None
        assert link["method"] == "rule_prefilter"

    def test_b_sub30_with_content_is_overhead_skip(self, seeded_db, run_summary):
        link = _ticket_link(seeded_db, SID_SUBTHIRTY)
        assert link is not None
        # Prefilter triggers on duration < MIN_LLM_DURATION_S (30s).
        assert link["session_type"] == "overhead"
        assert link["routing"] == "skip"
        assert link["method"] == "rule_prefilter"

    def test_c_code_defer_runs_stage2(self, seeded_db, run_summary):
        # 31s Code session with .py file → Stage 1 should add activity=coding
        # but no verbatim ticket → Stage 2 takes over.
        dims = _dims(seeded_db, SID_CODE_DEFER)
        assert "coding" in dims.get("activity", set()), \
            f"expected activity=coding from ide_with_code_files rule, got {dims.get('activity')}"
        link = _ticket_link(seeded_db, SID_CODE_DEFER)
        assert link is not None
        # Stage 1 deferred — Stage 2 wrote the row with method=stage2_embed.
        assert link["method"] == "semantic_embed", \
            f"expected semantic matcher to handle this session, got method={link['method']}"
        assert link["session_type"] == "task"

    def test_d_verbatim_kan86_auto_dispatches(self, seeded_db, run_summary):
        link = _ticket_link(seeded_db, SID_VERBATIM_KAN86)
        assert link is not None
        assert link["task_key"] == "KAN-86"
        assert link["session_type"] == "task"
        assert link["routing"] == "auto"
        assert link["method"] == "rule_regex"
        assert link["confidence"] >= 0.9

        # Dispatch row queued under jira provider.
        rows = [d for d in _dispatches(seeded_db) if d["session_id"] == SID_VERBATIM_KAN86]
        assert len(rows) == 1
        assert rows[0]["task_key"] == "KAN-86"
        assert rows[0]["provider"] == "jira"

    def test_e_utf8_falsepositive_is_filtered(self, seeded_db, run_summary):
        # UTF-8 / RFC-2616 / HTTP-200 are in _TICKET_FALSE_POSITIVES. Stage 1
        # must therefore NOT see any candidate keys, defer to Stage 2, and
        # Stage 2 should write a stage2_embed row (or nothing if it can't
        # match) — but never tag UTF-8 as a ticket.
        link = _ticket_link(seeded_db, SID_UTF8_FALSEPOS)
        assert link is not None, "session should still get a stage 2 row"
        assert link["task_key"] != "UTF-8"
        assert link["task_key"] != "RFC-2616"
        assert link["method"] != "rule_regex", \
            "rule classifier must not have matched a denylisted token"

    def test_f_multi_ticket_first_in_text_wins(self, seeded_db, run_summary):
        # First-seen ticket key wins — extract_tickets de-dupes preserving
        # insertion order, and titles are joined in order. KAN-87 is first
        # in window_titles.
        link = _ticket_link(seeded_db, SID_MULTI_TICKET)
        assert link is not None
        assert link["task_key"] == "KAN-87", \
            f"first-seen ticket should win; got {link['task_key']}"
        assert link["routing"] == "auto"
        assert link["method"] == "rule_regex"

    def test_g_unknown_ticket_records_task_skip(self, seeded_db, run_summary):
        # ABC-1 is ticket-shaped but not in pm_tasks → write task/skip with
        # task_key=NULL.
        link = _ticket_link(seeded_db, SID_TICKET_UNKNOWN)
        assert link is not None
        assert link["task_key"] is None
        assert link["session_type"] == "task"
        assert link["routing"] == "skip"
        assert link["method"] == "rule_regex"

    def test_h_cursor_chat_is_ai_pair_programming(self, seeded_db, run_summary):
        dims = _dims(seeded_db, SID_CURSOR_AI)
        assert "ai_pair_programming" in dims.get("activity", set()), \
            f"expected ai_pair_programming, got {dims.get('activity')}"
        assert "ai_assisted" in dims.get("collaboration", set())
        # 500s → engagement bucket is "focused" (per duration_engagement rule).
        assert dims.get("engagement", set()) <= {"focused", "deep_work"}

    def test_i_zoom_meeting_is_meeting_team_review(self, seeded_db, run_summary):
        dims = _dims(seeded_db, SID_MEETING)
        assert "meeting" in dims.get("activity", set())
        # team_review is one of the meeting rule's secondary hits.
        assert "team_review" in dims.get("collaboration", set())
        # 1200s > 600s → deep_work
        assert "deep_work" in dims.get("engagement", set())

    def test_j_tailscale_routes_to_stage2(self, seeded_db, run_summary):
        # No verbatim KAN-NN → Stage 1 defers. Stage 2 must produce a row.
        link = _ticket_link(seeded_db, SID_TAILSCALE_SEM)
        assert link is not None
        assert link["method"] == "semantic_embed", \
            f"expected semantic matcher to handle tailscale session, got {link['method']}"
        # With our fake encoder, KAN-83 shares many tokens (tailscale, mac
        # studio, macbook air, ssh, remote) with the session — so we
        # expect KAN-83 to be the chosen task IF stage 2 routes anywhere
        # other than skip. We don't pin the routing because the fake
        # encoder's exact threshold behaviour isn't a guarantee — but if
        # a task_key is chosen, it MUST be KAN-83.
        if link["task_key"] is not None:
            assert link["task_key"] == "KAN-83", (
                "tailscale session should map to KAN-83 (Setup tailscale...) "
                f"but mapped to {link['task_key']}"
            )


# ───────────────────────────── global invariants ──────────────────────────────
class TestGlobalInvariants:
    def test_cursor_advanced_to_max_session_id(self, seeded_db, run_summary):
        cur = _cursor(seeded_db)
        assert cur == SID_TAILSCALE_SEM, \
            f"cursor should advance to MAX(session.id)={SID_TAILSCALE_SEM}; got {cur}"

    def test_each_session_has_exactly_one_ticket_link(self, seeded_db, run_summary):
        conn = sqlite3.connect(str(seeded_db))
        try:
            rows = conn.execute(
                "SELECT session_id, COUNT(*) FROM ticket_links GROUP BY session_id"
            ).fetchall()
        finally:
            conn.close()
        for sid, n in rows:
            assert n == 1, f"session {sid} has {n} ticket_links rows (expected 1)"
        # And every fixture session got a row.
        seen_ids = {sid for sid, _ in rows}
        expected = {
            SID_EMPTY, SID_SUBTHIRTY, SID_CODE_DEFER, SID_VERBATIM_KAN86,
            SID_UTF8_FALSEPOS, SID_MULTI_TICKET, SID_TICKET_UNKNOWN,
            SID_CURSOR_AI, SID_MEETING, SID_TAILSCALE_SEM,
        }
        assert seen_ids == expected, f"missing sessions: {expected - seen_ids}"

    def test_dispatch_queue_only_has_auto_or_queue(self, seeded_db, run_summary):
        rows = _dispatches(seeded_db)
        for d in rows:
            payload = json.loads(d["payload_json"])
            assert payload.get("routing") in ("auto", "queue"), \
                f"dispatch row {d['id']} has routing={payload.get('routing')!r}"
            assert d["task_key"], \
                f"dispatch row {d['id']} has empty task_key — must not be queued"

    def test_dispatch_queue_includes_kan86_verbatim(self, seeded_db, run_summary):
        # Stage 1 auto match queues a dispatch.
        rows = [d for d in _dispatches(seeded_db) if d["task_key"] == "KAN-86"
                and d["session_id"] == SID_VERBATIM_KAN86]
        assert len(rows) == 1, \
            f"expected exactly 1 KAN-86 dispatch for session {SID_VERBATIM_KAN86}; got {len(rows)}"

    def test_session_dimensions_present_for_non_prefiltered(self, seeded_db, run_summary):
        # Pre-filtered sessions get exactly one dim row (engagement=idle from
        # the prefilter). Non-prefiltered get many.
        non_prefiltered = {
            SID_CODE_DEFER, SID_VERBATIM_KAN86, SID_UTF8_FALSEPOS,
            SID_MULTI_TICKET, SID_TICKET_UNKNOWN, SID_CURSOR_AI,
            SID_MEETING, SID_TAILSCALE_SEM,
        }
        for sid in non_prefiltered:
            dims = _dims(seeded_db, sid)
            assert dims, f"session {sid} should have session_dimensions rows but has none"
            # Every non-prefiltered session also gets the solo_default
            # collaboration fallback unless overridden.
            assert "collaboration" in dims, \
                f"session {sid} missing collaboration dimension; got {sorted(dims)}"

    def test_prefiltered_sessions_have_idle_engagement(self, seeded_db, run_summary):
        for sid in (SID_EMPTY, SID_SUBTHIRTY):
            dims = _dims(seeded_db, sid)
            assert dims.get("engagement") == {"idle"}, \
                f"prefiltered session {sid} should have engagement=idle; got {dims}"

    def test_agent_run_completed_success(self, seeded_db, run_summary):
        runs = _agent_runs(seeded_db)
        assert len(runs) == 1, f"expected exactly 1 agent_run, got {len(runs)}"
        r = runs[0]
        assert r["status"] == "success", \
            f"agent_run status should be success; got {r['status']!r}, error={r.get('error')!r}"
        assert r["finished_at"] is not None, "finished_at must be set on completion"
        assert r["sessions_processed"] == 10

    def test_no_dangling_running_runs(self, seeded_db, run_summary):
        runs = _agent_runs(seeded_db)
        running = [r for r in runs if r["status"] == "running"]
        assert running == [], f"found dangling 'running' agent_runs: {running}"

    def test_summary_counters_match_db(self, seeded_db, run_summary):
        # The summary dict must agree with what we count in the DB.
        assert run_summary["sessions"] == 10
        assert run_summary["skipped"] == 2  # A + B
        assert run_summary["kept"] == 8


# ───────────────────────────── Stage 3 placeholder ────────────────────────────
@pytest.mark.skip(reason="requires HERMES_BASE_URL + live LLM endpoint; "
                         "Stage 3 is non-deterministic and provider-dependent")
def test_stage3_smoke():  # pragma: no cover
    """Placeholder for the Stage 3 smoke pass.

    Stage 3 only fires when Stage 2 returns routing='queue'. To exercise it
    end-to-end we'd need:
      * a configured HERMES_BASE_URL pointing at a real model endpoint
      * an OLLAMA_API_KEY (or equivalent)
      * tolerance for non-determinism in the LLM's chosen task / confidence
    Mocking the model output here would just be Stage 2 in disguise.
    """
    pass


# ───────────────────────── cursor-advance-on-failure ─────────────────────────
def test_cursor_advances_per_session_on_mid_batch_failure(
    seeded_db, fake_embedder, monkeypatch
):
    """Simulate a SIGTERM mid-batch and assert earlier cursors persisted.

    We patch `agents.tagger._tag_session` so the 3rd kept session raises.
    Sessions before it must still leave the cursor advanced to their ids,
    even though later sessions never ran. This locks in the durability
    invariant called out in tagger.run_once's comment block.

    Side effect we *don't* assert (and therefore record as a bug below):
    the agent_runs row is left in 'running' status because the exception
    propagates past the `with db.connection()` block before
    `complete_agent_run` is reached. A future zombie-sweep step should
    mark it 'aborted'.
    """
    import agents.tagger as tagger

    real_tag_session = tagger._tag_session
    call_count = {"n": 0}

    def boom(*args, **kwargs):
        call_count["n"] += 1
        # Sessions 1 + 2 are pre-filtered so they don't call _tag_session.
        # Sessions 3 (SID_CODE_DEFER), 4 (SID_VERBATIM_KAN86) run normally;
        # the 3rd kept session (SID 5, UTF-8) blows up.
        if call_count["n"] == 3:
            raise RuntimeError("simulated SIGTERM mid-batch")
        return real_tag_session(*args, **kwargs)

    monkeypatch.setattr(tagger, "_tag_session", boom)

    with pytest.raises(RuntimeError):
        tagger.run_once(stages={1, 2}, since_iso=None)

    # Cursor must reflect at least the last successfully completed session.
    cur = _cursor(seeded_db)
    assert cur >= SID_VERBATIM_KAN86, (
        f"cursor should have advanced through the 4 sessions that finished "
        f"before the failure; got {cur}"
    )
    assert cur < SID_UTF8_FALSEPOS, (
        f"cursor should NOT have advanced past the failing session; got {cur}"
    )
