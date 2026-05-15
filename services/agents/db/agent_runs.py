"""agent_runs table and agent_cursor helpers."""
from __future__ import annotations

import logging
import sqlite3

from .connections import _utc_now

log = logging.getLogger("agents.db")


def start_agent_run(conn: sqlite3.Connection) -> int:
    cur = conn.execute(
        "INSERT INTO agent_runs (started_at, status) VALUES (?, 'running')",
        (_utc_now(),),
    )
    return int(cur.lastrowid)


def complete_agent_run(
    conn: sqlite3.Connection,
    run_id: int,
    status: str,
    *,
    error: str | None = None,
    sessions_processed: int = 0,
    summaries_written: int = 0,
    links_written: int = 0,
    dispatches_queued: int = 0,
    dispatches_sent: int = 0,
) -> None:
    conn.execute(
        """
        UPDATE agent_runs
           SET finished_at        = ?,
               status             = ?,
               error              = ?,
               sessions_processed = ?,
               summaries_written  = ?,
               links_written      = ?,
               dispatches_queued  = ?,
               dispatches_sent    = ?
         WHERE id = ?
        """,
        (
            _utc_now(),
            status,
            error,
            sessions_processed,
            summaries_written,
            links_written,
            dispatches_queued,
            dispatches_sent,
            run_id,
        ),
    )


def get_cursor(conn: sqlite3.Connection) -> int:
    row = conn.execute("SELECT last_session_id FROM agent_cursor WHERE id = 1").fetchone()
    return int(row["last_session_id"]) if row else 0


def advance_cursor(conn: sqlite3.Connection, last_session_id: int) -> None:
    cur = conn.execute(
        """
        UPDATE agent_cursor
           SET last_session_id = ?, updated_at = ?
         WHERE id = 1 AND ? > last_session_id
        """,
        (last_session_id, _utc_now(), last_session_id),
    )
    if cur.rowcount == 0:
        log.warning(
            "advance_cursor: no row updated (id=1 missing or last_session_id=%d not greater than stored value)",
            last_session_id,
        )


def advance_cursor_to_id(conn: sqlite3.Connection, target_id: int) -> None:
    """Force-set the agent cursor (used to skip the historical backlog)."""
    conn.execute(
        """
        UPDATE agent_cursor
           SET last_session_id = ?, updated_at = ?
         WHERE id = 1
        """,
        (int(target_id), _utc_now()),
    )
