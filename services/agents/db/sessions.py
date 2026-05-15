"""Session, pm_tasks, ticket_links, and session_dimensions reads/writes."""
from __future__ import annotations

import sqlite3
from typing import Any, Iterable

from .connections import _utc_now, _json_or_none
from .agent_runs import get_cursor

_SESSION_COLS = (
    "id, app_name, started_at, ended_at, duration_s, "
    "window_titles, session_text, audio_snippets, "
    "category, confidence, traceparent"
)


def _row_to_session(row: sqlite3.Row) -> dict:
    return {
        "id": row["id"],
        "app_name": row["app_name"],
        "started_at": row["started_at"],
        "ended_at": row["ended_at"],
        "duration_s": row["duration_s"],
        "window_titles": _json_or_none(row["window_titles"]) or [],
        "session_text": row["session_text"] or "",
        "audio_snippets": _json_or_none(row["audio_snippets"]) or [],
        "category": row["category"],
        "confidence": row["confidence"],
        "traceparent": row["traceparent"] if "traceparent" in row.keys() else None,
    }


def fetch_session(conn: sqlite3.Connection, session_id: int) -> dict | None:
    """Fetch one session by id, regardless of cursor / date filter."""
    row = conn.execute(
        f"SELECT {_SESSION_COLS} FROM app_sessions WHERE id = ?",
        (int(session_id),),
    ).fetchone()
    return _row_to_session(row) if row else None


def fetch_recent_sessions(
    conn: sqlite3.Connection,
    limit: int,
    *,
    since_iso: str | None = None,
) -> list[dict]:
    """Fetch the most recent N sessions (newest first), unrelated to the cursor."""
    sql = f"SELECT {_SESSION_COLS} FROM app_sessions"
    args: list[Any] = []
    if since_iso:
        sql += " WHERE started_at >= ?"
        args.append(since_iso)
    sql += " ORDER BY id DESC LIMIT ?"
    args.append(int(limit))
    rows = conn.execute(sql, args).fetchall()
    return [_row_to_session(r) for r in rows]


def fetch_unprocessed_sessions(
    conn: sqlite3.Connection,
    limit: int,
    *,
    since_iso: str | None = None,
) -> list[dict]:
    """Pull the next `limit` sessions whose id is past the cursor.

    When `since_iso` is set, sessions whose `started_at` is earlier than that
    timestamp are also excluded — used by the synthesizer's ONLY_TODAY mode
    so we don't grind through the full historical backlog on every cycle.
    """
    cursor = get_cursor(conn)
    sql = (
        f"SELECT {_SESSION_COLS} "
        "  FROM app_sessions "
        " WHERE id > ?"
    )
    args: list[Any] = [cursor]
    if since_iso:
        sql += " AND started_at >= ?"
        args.append(since_iso)
    sql += " ORDER BY id ASC LIMIT ?"
    args.append(limit)
    rows = conn.execute(sql, args).fetchall()
    return [_row_to_session(r) for r in rows]


def fetch_active_session(conn: sqlite3.Connection) -> dict | None:
    row = conn.execute(
        """
        SELECT id, app_name, started_at, last_seen_at,
               window_titles, session_text, audio_snippets,
               category, confidence
          FROM active_session
         WHERE id = 1
        """
    ).fetchone()
    if not row:
        return None
    return {
        "id": row["id"],
        "app_name": row["app_name"],
        "started_at": row["started_at"],
        "last_seen_at": row["last_seen_at"],
        "window_titles": _json_or_none(row["window_titles"]) or [],
        "session_text": row["session_text"] or "",
        "audio_snippets": _json_or_none(row["audio_snippets"]) or [],
        "category": row["category"],
        "confidence": row["confidence"],
    }


def fetch_ticket_link(conn: sqlite3.Connection, session_id: int) -> dict | None:
    row = conn.execute(
        """
        SELECT session_id, task_key, provider, method, confidence,
               session_type, routing, created_at
          FROM ticket_links
         WHERE session_id = ?
        """,
        (int(session_id),),
    ).fetchone()
    return dict(row) if row else None


def clear_ticket_link(conn: sqlite3.Connection, session_id: int) -> int:
    cur = conn.execute("DELETE FROM ticket_links WHERE session_id = ?", (int(session_id),))
    return int(cur.rowcount or 0)


def write_ticket_link(
    conn: sqlite3.Connection,
    *,
    session_id: int,
    task_key: str | None,
    confidence: float,
    session_type: str,
    routing: str,
    provider: str | None = "jira",
    method: str = "synthesizer",
) -> None:
    conn.execute(
        """
        INSERT INTO ticket_links (
            session_id, task_key, provider, method,
            confidence, session_type, routing, created_at
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)
        ON CONFLICT(session_id) DO UPDATE SET
            task_key     = excluded.task_key,
            provider     = excluded.provider,
            method       = excluded.method,
            confidence   = excluded.confidence,
            session_type = excluded.session_type,
            routing      = excluded.routing,
            created_at   = excluded.created_at
        """,
        (
            session_id,
            task_key,
            provider if task_key else None,
            method,
            float(confidence),
            session_type,
            routing,
            _utc_now(),
        ),
    )


def fetch_pm_tasks(
    conn: sqlite3.Connection,
    *,
    provider: str | None = "jira",
    only_open: bool = True,
) -> list[dict]:
    where_parts = []
    args: list[Any] = []
    if provider:
        where_parts.append("provider = ?")
        args.append(provider)
    if only_open:
        where_parts.append("LOWER(status_category) <> 'done'")
    where = ("WHERE " + " AND ".join(where_parts)) if where_parts else ""
    rows = conn.execute(
        f"""
        SELECT task_key, provider, title, description_text,
               status, status_category, issue_type, project_key, url,
               updated_at, fetched_at, expires_at
          FROM pm_tasks
          {where}
         ORDER BY updated_at DESC
        """,
        args,
    ).fetchall()
    return [dict(r) for r in rows]


def upsert_pm_task(
    conn: sqlite3.Connection,
    *,
    task_key: str,
    title: str = "",
    description_text: str = "",
    status: str = "",
    status_category: str = "todo",
    issue_type: str = "",
    project_key: str = "",
    url: str = "",
    updated_at: str | None = None,
    provider: str = "jira",
    expires_minutes: int = 30,
) -> None:
    """Mirror of the Rust upsert in intelligence/providers/jira.rs."""
    now = _utc_now()
    upd = updated_at or now
    conn.execute(
        f"""
        INSERT INTO pm_tasks (
            task_key, provider, title, description_text,
            status, status_category, issue_type, project_key, url,
            updated_at, fetched_at, expires_at
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?,
                  strftime('%Y-%m-%dT%H:%M:%SZ', 'now', '+{int(expires_minutes)} minutes'))
        ON CONFLICT(task_key) DO UPDATE SET
            provider         = excluded.provider,
            title            = excluded.title,
            description_text = excluded.description_text,
            status           = excluded.status,
            status_category  = excluded.status_category,
            issue_type       = excluded.issue_type,
            project_key      = excluded.project_key,
            url              = excluded.url,
            updated_at       = excluded.updated_at,
            fetched_at       = excluded.fetched_at,
            expires_at       = excluded.expires_at
        """,
        (
            task_key,
            provider,
            title,
            description_text,
            status,
            status_category,
            issue_type,
            project_key,
            url,
            upd,
            now,
        ),
    )


def upsert_session_dimension(
    conn: sqlite3.Connection,
    *,
    session_id: int,
    dimension: str,
    value: str,
    confidence: float,
    source: str,
) -> None:
    """Insert or refresh one (session, dimension, value) row.

    On conflict, keep the higher confidence and the newer source. This makes
    the writer idempotent across multiple stage runs (regex → embeddings → LLM).
    """
    conn.execute(
        """
        INSERT INTO session_dimensions (session_id, dimension, value, confidence, source, created_at)
        VALUES (?, ?, ?, ?, ?, ?)
        ON CONFLICT(session_id, dimension, value) DO UPDATE SET
            confidence = MAX(session_dimensions.confidence, excluded.confidence),
            source     = excluded.source,
            created_at = excluded.created_at
        """,
        (session_id, dimension, value, float(confidence), source, _utc_now()),
    )


def fetch_session_dimensions(conn: sqlite3.Connection, session_id: int) -> list[dict]:
    rows = conn.execute(
        """
        SELECT dimension, value, confidence, source, created_at
          FROM session_dimensions
         WHERE session_id = ?
         ORDER BY dimension, confidence DESC
        """,
        (session_id,),
    ).fetchall()
    return [dict(r) for r in rows]


def clear_session_dimensions(conn: sqlite3.Connection, session_id: int) -> int:
    cur = conn.execute(
        "DELETE FROM session_dimensions WHERE session_id = ?",
        (session_id,),
    )
    return int(cur.rowcount or 0)


def session_id_max(sessions: Iterable[dict]) -> int:
    """Return the largest id in a session bundle, or 0 if empty."""
    return max((int(s["id"]) for s in sessions), default=0)
