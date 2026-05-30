"""SQLite read/write layer for coding_agent_indexer.

Owns inserts to `app_sessions` for Claude / Codex sessions (rows carry a
non-NULL `claude_session_uuid`). Segment chunking: a session is sliced
into work bursts split on >1h idle gaps, one row per burst, keyed on
`(claude_session_uuid, segment_started_at)`.

Lifecycle of a coding-agent row:
  * LIVE   — `sealed_at IS NULL`, `task_method = 'coding_agent_live'`.
             Re-UPSERTed each poll while the burst is still growing.
  * SEALED — `sealed_at` set, `task_method = 'pending_summariser'`.
             Immutable: the UPSERT carries `WHERE sealed_at IS NULL`, so a
             sealed row is never mutated again. This is the downstream
             contract — summariser/classifier only read sealed rows.

Connection model: short-lived `sqlite3.Connection` per call. WAL mode is
set once at DB creation by the Rust ETL — we only set busy_timeout per
connection. Read paths use URI read-only mode for extra safety.
"""
from __future__ import annotations

import logging
import sqlite3
from contextlib import contextmanager
from pathlib import Path
from typing import Iterator, Optional

from coding_agent_indexer import config
from coding_agent_indexer.jsonl_meta import Segment

log = logging.getLogger(__name__)

# Sentinel for rows we own — not produced by any ETL run.
_INDEXER_ETL_RUN_ID = 0

# Static defaults for columns that exist for screen-frame rows but are
# meaningless for a Claude / Codex session.
_EMPTY_JSON_LIST = "[]"
_CATEGORY        = "coding"                # we are sure: this IS coding work
_CATEGORY_METHOD = "coding_agent_indexer"

# `task_method` is set non-NULL in BOTH states so the Rust MLX classifier
# (which selects `WHERE task_method IS NULL`) skips these rows whether live
# or sealed. The summariser's queue is `WHERE task_method = 'pending_summariser'`
# — which only sealed rows carry.
TASK_METHOD_LIVE    = "coding_agent_live"
TASK_METHOD_PENDING = "pending_summariser"

# Map agent flavour → app_name / session_text_source. We use the agent's
# product name, not the host terminal/IDE: a Claude Code session is
# `Claude Code` whether run inside VS Code, iTerm2, or any other terminal.
_APP_NAME_BY_AGENT = {
    "claude_code": "Claude Code",
    "codex":       "Codex",
}
_TEXT_SOURCE_BY_AGENT = {
    "claude_code": "claude_jsonl",
    "codex":       "codex_jsonl",
}

_BUSY_TIMEOUT_MS = 5000


# ──────────────────────── Connection helper ────────────────────────────────────


@contextmanager
def connect(
    db_path: Optional[Path] = None, *, readonly: bool = False,
) -> Iterator[sqlite3.Connection]:
    """Short-lived sqlite3 connection. WAL was set by the Rust ETL at
    DB creation; we only set busy_timeout per connection.

    `db_path=None` resolves to `config.MERIDIAN_DB` at call time so
    tests / repl users that monkeypatch the config see the override.
    """
    target = Path(db_path) if db_path is not None else config.MERIDIAN_DB
    if readonly:
        conn = sqlite3.connect(f"file:{target}?mode=ro", uri=True, timeout=10)
    else:
        conn = sqlite3.connect(target, timeout=10)
    conn.row_factory = sqlite3.Row
    conn.execute(f"PRAGMA busy_timeout={_BUSY_TIMEOUT_MS}")
    try:
        yield conn
    finally:
        conn.close()


# ──────────────────────── Write paths ──────────────────────────────────────────


def upsert_segment(
    segment: Segment,
    *,
    sealed: bool,
    sealed_at: Optional[str] = None,
    db_path: Optional[Path] = None,
) -> Optional[int]:
    """INSERT or UPDATE one (uuid, segment_started_at) row.

    `sealed=False` writes/refreshes a LIVE row (mutable, re-UPSERTed each
    poll). `sealed=True` writes/seals the row — `task_method` flips to
    `pending_summariser` and `sealed_at` is stamped.

    The UPDATE branch carries `WHERE sealed_at IS NULL`, so once a row is
    sealed it is immutable: subsequent UPSERTs for the same key (e.g. a
    redundant re-scan) are no-ops. Combined with the parser's sealed
    high-water boundary, this guarantees a sealed row never changes and
    new content always lands in a fresh segment.

    Returns the row id, or None if the segment was rejected as invalid.
    """
    if not segment.is_valid:
        log.info(
            "skip upsert: segment invalid (uuid=%s seg_start=%s user=%d asst=%d ended=%r)",
            segment.session_uuid, segment.segment_started_at, segment.user_turns,
            segment.assistant_turns, segment.ended_at,
        )
        return None

    app_name     = _APP_NAME_BY_AGENT.get(segment.agent, "Claude Code")
    transcript   = segment.transcript
    session_text = transcript if transcript else None
    text_source  = _TEXT_SOURCE_BY_AGENT.get(segment.agent) if transcript else None
    frame_count  = segment.user_turns + segment.assistant_turns
    task_method  = TASK_METHOD_PENDING if sealed else TASK_METHOD_LIVE
    sealed_stamp = sealed_at if sealed else None

    with connect(db_path) as con:
        con.execute(
            """
            INSERT INTO app_sessions (
                app_name, started_at, ended_at, duration_s,
                window_titles, min_frame_id, max_frame_id, frame_count,
                etl_run_id, idle_frame_count,
                category, confidence, category_method,
                session_text, session_text_source,
                task_method,
                claude_session_uuid, segment_started_at, sealed_at
            )
            VALUES (?, ?, ?, ?,  ?, ?, ?, ?,  ?, ?,  ?, ?, ?,  ?, ?,  ?,  ?, ?, ?)
            ON CONFLICT (claude_session_uuid, segment_started_at)
            WHERE claude_session_uuid IS NOT NULL
            DO UPDATE SET
                started_at          = excluded.started_at,
                ended_at            = excluded.ended_at,
                duration_s          = excluded.duration_s,
                frame_count         = excluded.frame_count,
                session_text        = excluded.session_text,
                session_text_source = excluded.session_text_source,
                task_method         = excluded.task_method,
                sealed_at           = excluded.sealed_at
            WHERE app_sessions.sealed_at IS NULL
            """,
            (
                app_name,
                segment.started_at,
                segment.ended_at,
                segment.active_seconds,
                _EMPTY_JSON_LIST,
                0,                                         # min_frame_id (sentinel)
                0,                                         # max_frame_id (sentinel)
                frame_count,
                _INDEXER_ETL_RUN_ID,
                0,                                         # idle_frame_count
                _CATEGORY,
                1.0,                                       # confidence
                _CATEGORY_METHOD,
                session_text,
                text_source,
                task_method,
                segment.session_uuid,
                segment.segment_started_at,
                sealed_stamp,
            ),
        )
        row = con.execute(
            "SELECT id FROM app_sessions "
            "WHERE claude_session_uuid = ? AND segment_started_at = ?",
            (segment.session_uuid, segment.segment_started_at),
        ).fetchone()
        con.commit()
        return row["id"] if row else None


def seal_stale_open_rows(
    *,
    now_iso: str,
    idle_seconds: int,
    db_path: Optional[Path] = None,
) -> int:
    """Seal every LIVE coding-agent row whose last activity is > idle_seconds old.

    This is the robust backstop that does NOT require re-parsing the JSONL:
    it seals rows left open by crashes, force-quits, macOS sleep, or a file
    that was deleted after the session ended. Idempotent — already-sealed
    rows are excluded by `sealed_at IS NULL`.

    Returns the number of rows sealed.
    """
    cutoff = _shift_iso(now_iso, -idle_seconds)
    with connect(db_path) as con:
        cur = con.execute(
            """
            UPDATE app_sessions
            SET    sealed_at = ?, task_method = ?
            WHERE  claude_session_uuid IS NOT NULL
              AND  sealed_at IS NULL
              AND  ended_at < ?
            """,
            (now_iso, TASK_METHOD_PENDING, cutoff),
        )
        con.commit()
        return cur.rowcount or 0


def delete_claude_session_rows(*, db_path: Optional[Path] = None) -> int:
    """Delete every Claude/Codex-owned app_sessions row.

    Used by the CLI `--reseed` flag to wipe legacy per-(uuid, day) rows so
    the next scan re-registers them under the per-segment scheme. Only
    touches rows the indexer owns (`claude_session_uuid IS NOT NULL`);
    never touches screen-frame rows. Returns rows deleted.
    """
    with connect(db_path) as con:
        cur = con.execute(
            "DELETE FROM app_sessions WHERE claude_session_uuid IS NOT NULL"
        )
        con.commit()
        return cur.rowcount or 0


# ──────────────────────── Read paths ────────────────────────────────────────────


def fetch_session_endpoints(*, db_path: Optional[Path] = None) -> dict[str, str]:
    """Return {claude_session_uuid: latest_ended_at_iso} across all its segments.

    Used by the daemon's change-detection: skip parsing a JSONL whose mtime
    hasn't moved past the latest stored `ended_at`.
    """
    with connect(db_path, readonly=True) as con:
        rows = con.execute(
            "SELECT claude_session_uuid, MAX(ended_at) AS ended_at "
            "FROM   app_sessions "
            "WHERE  claude_session_uuid IS NOT NULL "
            "GROUP  BY claude_session_uuid"
        ).fetchall()
    return {r["claude_session_uuid"]: r["ended_at"] for r in rows if r["claude_session_uuid"]}


def sealed_high_water(uuid: str, *, db_path: Optional[Path] = None) -> Optional[str]:
    """Latest `ended_at` among this session's SEALED segments, or None.

    Passed to the parser as `start_after_ts` so already-sealed content is
    excluded and any newer record opens a fresh segment — the invariant that
    makes a post-SessionEnd resume safe.
    """
    with connect(db_path, readonly=True) as con:
        row = con.execute(
            "SELECT MAX(ended_at) AS hwm FROM app_sessions "
            "WHERE claude_session_uuid = ? AND sealed_at IS NOT NULL",
            (uuid,),
        ).fetchone()
    return row["hwm"] if row and row["hwm"] else None


# ──────────────────────── Helpers ──────────────────────────────────────────────


def _shift_iso(iso: str, delta_seconds: int) -> str:
    """Return `iso` shifted by delta_seconds, in the canonical µs+'+00:00' UTC format.

    Used to compute the seal cutoff. Lexicographic comparison of two
    same-format UTC ISO strings is a valid chronological comparison, so the
    sweep's `ended_at < cutoff` works as a plain string compare.
    """
    from datetime import datetime, timedelta, timezone
    dt = datetime.fromisoformat(iso.replace("Z", "+00:00"))
    if dt.tzinfo is None:
        dt = dt.replace(tzinfo=timezone.utc)
    shifted = dt.astimezone(timezone.utc) + timedelta(seconds=delta_seconds)
    # Canonical µs+'+00:00' shape (see jsonl_meta.iso_utc): the same fixed-width
    # UTC form every row stores, so the sweep's string compare stays chronological.
    return shifted.strftime("%Y-%m-%dT%H:%M:%S.%f+00:00")
