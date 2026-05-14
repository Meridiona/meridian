"""session_summaries and dispatch_queue reads/writes."""
from __future__ import annotations

import json
import sqlite3

from .connections import _utc_now, _json_or_none


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
