"""context_graph_nodes and activity_context reads/writes."""
from __future__ import annotations

import json
import sqlite3

from .connections import _utc_now, _json_or_none


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
