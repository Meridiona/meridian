"""SQLite layer for the summariser — read the queue, write the summary.

Read paths use URI read-only mode. The single write path is idempotent:
`UPDATE ... WHERE session_summary IS NULL`, so concurrent runs, restarts, and
retries can never double-write or clobber a summary that already landed.

Connection model: short-lived `sqlite3.Connection` per call (mirrors the
indexer). We never hold a transcript longer than the call that uses it.
"""
from __future__ import annotations

import logging
import sqlite3
from contextlib import contextmanager
from dataclasses import dataclass
from pathlib import Path
from typing import Iterator, List, Optional

from coding_agent_summariser import config

log = logging.getLogger(__name__)

_BUSY_TIMEOUT_MS = 5000


@dataclass(frozen=True)
class PendingRow:
    """A sealed coding-agent segment awaiting a summary (metadata only).

    `session_text` is fetched separately, one at a time, to keep memory flat.
    """
    id:                 int
    session_uuid:       str
    agent:              str
    segment_started_at: str
    started_at:         str
    ended_at:           str
    duration_s:         int
    text_bytes:         int


@contextmanager
def connect(
    db_path: Optional[Path] = None, *, readonly: bool = False,
) -> Iterator[sqlite3.Connection]:
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


def ensure_schema(*, db_path: Optional[Path] = None) -> None:
    """Idempotently add the `summary_source` column to app_sessions.

    app_sessions is Rust-owned, but `summary_source` is a summariser-only field
    not managed by any Rust migration — so we add it from Python (the same
    pattern pm_update uses for its own additive columns). Rust ignores the extra
    column; no Rust migration / recompile needed. Safe to call on every startup.
    """
    with connect(db_path) as con:
        cols = {r["name"] for r in con.execute("PRAGMA table_info(app_sessions)")}
        if "summary_source" not in cols:
            con.execute("ALTER TABLE app_sessions ADD COLUMN summary_source TEXT")
            con.commit()
            log.info("added summary_source column to app_sessions")


def _to_row(r: sqlite3.Row) -> PendingRow:
    return PendingRow(
        id=r["id"],
        session_uuid=r["claude_session_uuid"],
        agent=r["app_name"],
        segment_started_at=r["segment_started_at"],
        started_at=r["started_at"],
        ended_at=r["ended_at"],
        duration_s=r["duration_s"],
        text_bytes=r["text_bytes"] or 0,
    )


_ROW_COLS = """
    id, claude_session_uuid, app_name, segment_started_at,
    started_at, ended_at, duration_s,
    length(session_text) AS text_bytes
"""


def fetch_pending(
    limit: int,
    *,
    day: Optional[str] = None,
    session_uuid: Optional[str] = None,
    db_path: Optional[Path] = None,
) -> List[PendingRow]:
    """Sealed coding segments needing a summary, oldest-ended first.

    `day` (a `YYYY-MM-DD` day_utc) scopes the queue to a single calendar day —
    the daemon always passes today, so it never drains all history at once; the
    CLI passes a chosen day to backfill just that day. `session_uuid` restricts
    to one session. Oldest-first ordering means a session's earlier bursts are
    summarised before its later ones (prior-burst context — `fetch_prior_summary`).
    """
    # Noise filter: trivial/empty-work segments are never worth a summary.
    clauses = ""
    params: tuple = (config.TASK_METHOD_PENDING, config.MIN_TURNS, config.MIN_TEXT_BYTES)
    if day:
        clauses += " AND substr(started_at, 1, 10) = ?"
        params += (day,)
    if session_uuid:
        clauses += " AND claude_session_uuid = ?"
        params += (session_uuid,)
    params += (limit,)
    with connect(db_path, readonly=True) as con:
        rows = con.execute(
            f"""
            SELECT {_ROW_COLS}
            FROM   app_sessions
            WHERE  claude_session_uuid IS NOT NULL
              AND  sealed_at IS NOT NULL
              AND  task_method = ?
              AND  session_summary IS NULL
              AND  session_text IS NOT NULL
              AND  session_text <> ''
              AND  frame_count >= ?
              AND  length(session_text) >= ?
              {clauses}
            ORDER BY ended_at ASC
            LIMIT ?
            """,
            params,
        ).fetchall()
    return [_to_row(r) for r in rows]


def fetch_by_id(row_id: int, *, db_path: Optional[Path] = None) -> Optional[PendingRow]:
    """One row by id, regardless of summary/seal state — for CLI --row dry-runs."""
    with connect(db_path, readonly=True) as con:
        r = con.execute(
            f"SELECT {_ROW_COLS} FROM app_sessions WHERE id = ? AND claude_session_uuid IS NOT NULL",
            (row_id,),
        ).fetchone()
    return _to_row(r) if r else None


def fetch_transcript(row_id: int, *, db_path: Optional[Path] = None) -> str:
    """Full `session_text` for one row. Fetched per-row to bound memory."""
    with connect(db_path, readonly=True) as con:
        row = con.execute(
            "SELECT session_text FROM app_sessions WHERE id = ?", (row_id,),
        ).fetchone()
    return (row["session_text"] if row else "") or ""


def fetch_prior_summary(
    session_uuid: str,
    segment_started_at: str,
    *,
    db_path: Optional[Path] = None,
) -> Optional[str]:
    """The summary of this session's most recent EARLIER burst, if any.

    Passed to the model as continuation context so a resumed session reads as
    one coherent story instead of repeating itself.
    """
    with connect(db_path, readonly=True) as con:
        row = con.execute(
            """
            SELECT session_summary
            FROM   app_sessions
            WHERE  claude_session_uuid = ?
              AND  segment_started_at < ?
              AND  session_summary IS NOT NULL
            ORDER BY segment_started_at DESC
            LIMIT 1
            """,
            (session_uuid, segment_started_at),
        ).fetchone()
    return row["session_summary"] if row and row["session_summary"] else None


def write_summary(
    row_id: int, summary: str, *, source: Optional[str] = None, db_path: Optional[Path] = None,
) -> bool:
    """Persist the summary + engine source + flip task_method. Idempotent.

    `source` is the engine that produced it ('claude' | 'codex' | 'mlx'),
    stored in `summary_source`. Returns True if this call wrote the row, False
    if it was already summarised (another worker won the race / retry).
    """
    with connect(db_path) as con:
        cur = con.execute(
            """
            UPDATE app_sessions
            SET    session_summary = ?, task_method = ?, summary_source = ?
            WHERE  id = ? AND session_summary IS NULL
            """,
            (summary, config.TASK_METHOD_SUMMARISED, source, row_id),
        )
        con.commit()
        return (cur.rowcount or 0) > 0
