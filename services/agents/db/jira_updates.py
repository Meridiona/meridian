"""jira_update_log reads/writes."""
from __future__ import annotations

import logging
import sqlite3

from .connections import _utc_now

log = logging.getLogger("agents.db")


def log_jira_update(
    conn: sqlite3.Connection,
    *,
    task_key: str,
    period_start: str,
    period_end: str,
    session_count: int,
    duration_s: int,
    had_activity: bool,
    comment_body: str,
) -> int:
    """Insert a pending jira_update_log row. Returns the new row id.
    On conflict (same task+slot already exists) returns the existing id."""
    conn.execute(
        """
        INSERT INTO jira_update_log
            (task_key, period_start, period_end, session_count, duration_s,
             had_activity, comment_body, state)
        VALUES (?, ?, ?, ?, ?, ?, ?, 'pending')
        ON CONFLICT(task_key, period_start, period_end) DO UPDATE SET
            comment_body = excluded.comment_body,
            session_count = excluded.session_count,
            duration_s = excluded.duration_s,
            had_activity = excluded.had_activity
        """,
        (task_key, period_start, period_end, session_count, duration_s,
         int(had_activity), comment_body),
    )
    conn.commit()
    return _get_update_id(conn, task_key, period_start, period_end)


def _get_update_id(conn: sqlite3.Connection, task_key: str, period_start: str, period_end: str) -> int:
    row = conn.execute(
        "SELECT id FROM jira_update_log WHERE task_key=? AND period_start=? AND period_end=?",
        (task_key, period_start, period_end),
    ).fetchone()
    return row[0] if row else 0


def get_last_update(
    conn: sqlite3.Connection,
    task_key: str,
    period_start: str,
    period_end: str,
) -> dict | None:
    """Return the log row for (task, slot) if state='sent', else None."""
    row = conn.execute(
        """
        SELECT id, state, comment_id, posted_at
        FROM jira_update_log
        WHERE task_key=? AND period_start=? AND period_end=? AND state='sent'
        """,
        (task_key, period_start, period_end),
    ).fetchone()
    if not row:
        return None
    return {"id": row[0], "state": row[1], "comment_id": row[2], "posted_at": row[3]}


def mark_update_sent(conn: sqlite3.Connection, update_id: int, comment_id: str) -> None:
    """Set state='sent', comment_id, posted_at=now."""
    conn.execute(
        """
        UPDATE jira_update_log
        SET state='sent', comment_id=?, posted_at=strftime('%Y-%m-%dT%H:%M:%SZ','now')
        WHERE id=?
        """,
        (comment_id, update_id),
    )
    conn.commit()
    if conn.execute("SELECT changes()").fetchone()[0] == 0:
        log.warning("mark_update_sent: no row matched id=%d", update_id)


def mark_update_failed(conn: sqlite3.Connection, update_id: int, error: str) -> None:
    """Set state='failed', error (capped at 1000 chars)."""
    conn.execute(
        "UPDATE jira_update_log SET state='failed', error=? WHERE id=?",
        (error[:1000], update_id),
    )
    conn.commit()
    if conn.execute("SELECT changes()").fetchone()[0] == 0:
        log.warning("mark_update_failed: no row matched id=%d", update_id)
