"""DB layer for the meridian-agents service.

Reads sessions/active_session/etc. from meridian.db and writes the agent-side
tables (agent_runs, agent_cursor, session_summaries, dispatch_queue,
context_graph_nodes, activity_context). Schema is owned by the Rust ETL
(see src/migrations/005_agents.sql) — Python only does SELECT/INSERT/UPDATE.
"""
from __future__ import annotations

import json
import logging
import sqlite3
from contextlib import contextmanager
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Iterable

from agents.config import MERIDIAN_DB

log = logging.getLogger("agents.db")


# ── Connection helpers ─────────────────────────────────────────────────────────
def _utc_now() -> str:
    return datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def _connect() -> sqlite3.Connection:
    path = Path(MERIDIAN_DB).expanduser()
    if not path.exists():
        raise FileNotFoundError(
            f"meridian.db not found at {path}. Is the Meridian daemon running?"
        )
    conn = sqlite3.connect(str(path), isolation_level=None, timeout=10.0)
    conn.row_factory = sqlite3.Row
    conn.execute("PRAGMA journal_mode=WAL;")
    conn.execute("PRAGMA foreign_keys=ON;")
    return conn


@contextmanager
def connection():
    conn = _connect()
    try:
        yield conn
    finally:
        conn.close()


def _json_or_none(val: Any) -> Any:
    if val in (None, "", "null"):
        return None
    if isinstance(val, (dict, list)):
        return val
    try:
        return json.loads(val)
    except (json.JSONDecodeError, TypeError):
        return None


# ── agent_runs / agent_cursor ──────────────────────────────────────────────────
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


# ── Reads from app_sessions / active_session ──────────────────────────────────
_SESSION_COLS = (
    "id, app_name, started_at, ended_at, duration_s, "
    "window_titles, session_text, audio_snippets, "
    "category, confidence, traceparent"
)


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


def _row_to_session(row: sqlite3.Row) -> dict:
    # `traceparent` is populated by the Rust ETL when it closes a session
    # (migration 010_traceparent.sql). May be NULL for older rows or when
    # the daemon ran without observability — agents treat None as
    # "no parent context, start a fresh root span".
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


# ── pm_tasks (populated by the Rust intelligence/providers/jira.rs job) ───────
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
    """Mirror of the Rust upsert in intelligence/providers/jira.rs (so the
    Python fallback fetcher can populate pm_tasks with the same schema)."""
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


# ── ticket_links (session → task mapping) ─────────────────────────────────────
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


# ── context_graph_nodes ────────────────────────────────────────────────────────
def fetch_context_graph_nodes(conn: sqlite3.Connection, limit: int = 100) -> list[dict]:
    rows = conn.execute(
        """
        SELECT node_id, node_type, label, last_seen, frequency, confidence_avg
          FROM context_graph_nodes
         ORDER BY frequency DESC, last_seen DESC
         LIMIT ?
        """,
        (limit,),
    ).fetchall()
    return [dict(r) for r in rows]


def upsert_context_node(
    conn: sqlite3.Connection,
    node_id: str,
    node_type: str,
    label: str,
) -> None:
    now = _utc_now()
    conn.execute(
        """
        INSERT INTO context_graph_nodes (node_id, node_type, label, last_seen, frequency, confidence_avg)
        VALUES (?, ?, ?, ?, 1, 0.7)
        ON CONFLICT(node_id) DO UPDATE SET
            label     = excluded.label,
            last_seen = excluded.last_seen,
            frequency = context_graph_nodes.frequency + 1
        """,
        (node_id, node_type, label, now),
    )


# ── activity_context ───────────────────────────────────────────────────────────
def fetch_activity_context(conn: sqlite3.Connection) -> dict:
    row = conn.execute(
        """
        SELECT updated_at, active_project, jira_key, inferred_task,
               confidence, trigger_jira_sync, tags, last_synced
          FROM activity_context
         WHERE id = 1
        """
    ).fetchone()
    if not row:
        return {}
    return {
        "updated_at": row["updated_at"],
        "active_project": row["active_project"],
        "jira_key": row["jira_key"],
        "inferred_task": row["inferred_task"],
        "confidence": row["confidence"],
        "trigger_jira_sync": bool(row["trigger_jira_sync"]),
        "tags": _json_or_none(row["tags"]) or [],
        "last_synced": row["last_synced"],
    }


def write_activity_context(
    conn: sqlite3.Connection,
    *,
    inferred_task: str,
    confidence: float,
    trigger_jira_sync: bool,
    active_project: str | None = None,
    jira_key: str | None = None,
    tags: list[str] | None = None,
) -> None:
    conn.execute(
        """
        UPDATE activity_context
           SET updated_at        = ?,
               active_project    = ?,
               jira_key          = ?,
               inferred_task     = ?,
               confidence        = ?,
               trigger_jira_sync = ?,
               tags              = ?
         WHERE id = 1
        """,
        (
            _utc_now(),
            active_project,
            jira_key,
            inferred_task,
            float(confidence),
            1 if trigger_jira_sync else 0,
            json.dumps(tags or []),
        ),
    )


def mark_activity_synced(conn: sqlite3.Connection) -> None:
    conn.execute(
        """
        UPDATE activity_context
           SET trigger_jira_sync = 0,
               last_synced       = ?
         WHERE id = 1
        """,
        (_utc_now(),),
    )


# ── session_summaries ──────────────────────────────────────────────────────────
def write_session_summary(
    conn: sqlite3.Connection,
    *,
    session_id: int,
    agent_run_id: int,
    summary_json: dict | str,
) -> None:
    payload = summary_json if isinstance(summary_json, str) else json.dumps(summary_json)
    conn.execute(
        """
        INSERT INTO session_summaries (session_id, agent_run_id, summary_json, generated_at)
        VALUES (?, ?, ?, ?)
        ON CONFLICT(session_id) DO UPDATE SET
            agent_run_id = excluded.agent_run_id,
            summary_json = excluded.summary_json,
            generated_at = excluded.generated_at
        """,
        (session_id, agent_run_id, payload, _utc_now()),
    )


# ── dispatch_queue ─────────────────────────────────────────────────────────────
def enqueue_dispatch(
    conn: sqlite3.Connection,
    *,
    session_id: int,
    agent_run_id: int,
    task_key: str,
    provider: str,
    payload: dict,
    state: str = "pending",
) -> int:
    cur = conn.execute(
        """
        INSERT INTO dispatch_queue (
            session_id, agent_run_id, task_key, provider, payload_json, state, created_at
        ) VALUES (?, ?, ?, ?, ?, ?, ?)
        """,
        (
            session_id,
            agent_run_id,
            task_key,
            provider,
            json.dumps(payload),
            state,
            _utc_now(),
        ),
    )
    return int(cur.lastrowid)


def fetch_pending_dispatches(
    conn: sqlite3.Connection,
    limit: int = 50,
    *,
    routing: str | None = None,
) -> list[dict]:
    rows = conn.execute(
        """
        SELECT id, session_id, agent_run_id, task_key, provider,
               payload_json, state, attempts, last_error, created_at
          FROM dispatch_queue
         WHERE state = 'pending'
         ORDER BY id ASC
         LIMIT ?
        """,
        (limit,),
    ).fetchall()
    out = []
    for r in rows:
        d = dict(r)
        d["payload"] = _json_or_none(d.pop("payload_json")) or {}
        if routing and d["payload"].get("routing") != routing:
            continue
        out.append(d)
    return out


def mark_dispatch_sent(conn: sqlite3.Connection, dispatch_id: int) -> None:
    conn.execute(
        """
        UPDATE dispatch_queue
           SET state         = 'sent',
               dispatched_at = ?,
               attempts      = attempts + 1,
               last_error    = NULL
         WHERE id = ?
        """,
        (_utc_now(), dispatch_id),
    )


def mark_dispatch_failed(conn: sqlite3.Connection, dispatch_id: int, err: str) -> None:
    conn.execute(
        """
        UPDATE dispatch_queue
           SET state      = 'failed',
               attempts   = attempts + 1,
               last_error = ?
         WHERE id = ?
        """,
        (err[:1000], dispatch_id),
    )


def mark_dispatch_skipped(conn: sqlite3.Connection, dispatch_id: int, reason: str) -> None:
    conn.execute(
        """
        UPDATE dispatch_queue
           SET state         = 'skipped',
               attempts      = attempts + 1,
               last_error    = ?,
               dispatched_at = ?
         WHERE id = ?
        """,
        (reason[:1000], _utc_now(), dispatch_id),
    )


# ── session_dimensions (multi-label tagging) ──────────────────────────────────
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

    On conflict we keep the higher confidence and the newer source. This makes
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


# ── Misc helpers ───────────────────────────────────────────────────────────────
def session_id_max(sessions: Iterable[dict]) -> int:
    """Return the largest id in a session bundle, or 0 if empty."""
    return max((int(s["id"]) for s in sessions), default=0)
