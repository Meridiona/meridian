# meridian — normalises screenpipe activity into structured app sessions

"""Async SQLite access layer for meridian-agents.

The Rust daemon owns the schema. This module only does SELECT/INSERT/UPDATE
against tables defined by `src/migrations/00*.sql`. WAL mode is already
enabled by the daemon; we set busy_timeout on each connection so writes
contend gracefully with the daemon's ETL writes.
"""

from __future__ import annotations

import json
from dataclasses import dataclass
from typing import Any

import aiosqlite

REQUIRED_TABLES = (
    # Rust-owned (Python reads only)
    "app_sessions",
    "active_session",
    "pm_tasks",
    # Cross-domain (existed in 003, agent-owned writer)
    "ticket_links",
    # Agent-owned (introduced in 005)
    "agent_runs",
    "agent_cursor",
    "dispatch_queue",
    "session_summaries",
    "context_graph_nodes",
    "activity_context",
)

# Open transactions block writers; default to 5s so the orchestrator backs off
# rather than failing immediately when the Rust daemon is mid-write.
BUSY_TIMEOUT_MS = 5000

VALID_ROUTINGS = ("auto", "queue", "skip")
VALID_SESSION_TYPES = ("task", "overhead", "unknown")
VALID_DISPATCH_PROVIDERS = ("jira", "github", "linear", "log")
VALID_DISPATCH_STATES = ("sent", "failed", "skipped")
VALID_RUN_STATUSES = ("success", "failed", "aborted")
VALID_NODE_TYPES = ("project", "task", "tool", "pattern", "ticket")


class SchemaError(RuntimeError):
    """Raised when the meridian.db schema is missing tables the agent
    service depends on. Almost always means the Rust daemon hasn't been
    run on this DB yet (or hasn't been rebuilt after a migration)."""


# ---------------------------------------------------------------------------
# Row shapes
# ---------------------------------------------------------------------------


@dataclass(frozen=True)
class Session:
    id: int
    app_name: str
    started_at: str
    ended_at: str
    duration_s: int
    window_titles: list[Any]
    ocr_samples: list[Any]
    audio_snippets: list[Any]
    # category + confidence are populated by the Rust ETL inline (004_categories.sql).
    # category is one of the 10 ActivityKind values (lowercase enum name).
    category: str
    confidence: float


@dataclass(frozen=True)
class PmTask:
    task_key: str
    provider: str
    title: str
    description_text: str
    status: str
    status_category: str
    issue_type: str
    project_key: str
    url: str
    updated_at: str


@dataclass(frozen=True)
class DispatchItem:
    id: int
    session_id: int
    agent_run_id: int
    task_key: str
    provider: str
    payload_json: str
    attempts: int


@dataclass(frozen=True)
class ContextNode:
    node_id: str
    node_type: str
    label: str
    last_seen: str
    frequency: int
    confidence_avg: float


@dataclass(frozen=True)
class ActivityContext:
    updated_at: str
    active_project: str | None
    jira_key: str | None
    inferred_task: str
    confidence: float
    trigger_jira_sync: bool
    tags: list[Any]
    last_synced: str | None


# ---------------------------------------------------------------------------
# Connection helpers
# ---------------------------------------------------------------------------


async def open_ro(meridian_db_path: str) -> aiosqlite.Connection:
    """Open the meridian DB in read-only mode (mode=ro)."""
    uri = f"file:{meridian_db_path}?mode=ro"
    conn = await aiosqlite.connect(uri, uri=True)
    await conn.execute(f"PRAGMA busy_timeout = {BUSY_TIMEOUT_MS}")
    conn.row_factory = aiosqlite.Row
    return conn


async def open_rw(meridian_db_path: str) -> aiosqlite.Connection:
    """Open the meridian DB in read-write mode without creating it.

    `mode=rw` (not `rwc`) — we never create the file ourselves; the Rust
    daemon does that and runs migrations. Refusing to create avoids a
    silent fork where Python writes to an empty file the Rust daemon
    doesn't know about.
    """
    uri = f"file:{meridian_db_path}?mode=rw"
    conn = await aiosqlite.connect(uri, uri=True)
    await conn.execute(f"PRAGMA busy_timeout = {BUSY_TIMEOUT_MS}")
    await conn.execute("PRAGMA foreign_keys = ON")
    conn.row_factory = aiosqlite.Row
    return conn


async def schema_check(conn: aiosqlite.Connection) -> None:
    """Verify the DB has every table meridian-agents needs.

    Raises `SchemaError` with an actionable message — almost always the
    fix is "run the Rust daemon at least once on this DB so migrations
    apply".
    """
    for table in REQUIRED_TABLES:
        cur = await conn.execute(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name = ?",
            (table,),
        )
        row = await cur.fetchone()
        await cur.close()
        if row is None:
            raise SchemaError(
                f"meridian.db is missing required table {table!r}. "
                "Run the Rust daemon (`./target/release/meridian`) on this "
                "MERIDIAN_DB at least once so migrations apply."
            )


# ---------------------------------------------------------------------------
# Cursor + run lifecycle
# ---------------------------------------------------------------------------


async def read_cursor(conn: aiosqlite.Connection) -> int:
    cur = await conn.execute("SELECT last_session_id FROM agent_cursor WHERE id = 1")
    row = await cur.fetchone()
    await cur.close()
    if row is None:
        # 005 seeds the row, but tolerate odd states for tests.
        return 0
    return int(row["last_session_id"])


async def advance_cursor(conn: aiosqlite.Connection, last_session_id: int) -> None:
    await conn.execute(
        """
        UPDATE agent_cursor
        SET last_session_id = ?,
            updated_at      = strftime('%Y-%m-%dT%H:%M:%SZ', 'now')
        WHERE id = 1
        """,
        (last_session_id,),
    )


async def start_run(conn: aiosqlite.Connection) -> int:
    cur = await conn.execute("INSERT INTO agent_runs DEFAULT VALUES")
    run_id = cur.lastrowid
    await cur.close()
    assert run_id is not None
    return run_id


async def finish_run(
    conn: aiosqlite.Connection,
    run_id: int,
    *,
    status: str,
    error: str | None = None,
    sessions_processed: int = 0,
    summaries_written: int = 0,
    links_written: int = 0,
    dispatches_queued: int = 0,
    dispatches_sent: int = 0,
) -> None:
    if status not in VALID_RUN_STATUSES:
        raise ValueError(f"finish_run: invalid status {status!r}")
    await conn.execute(
        """
        UPDATE agent_runs
        SET status              = ?,
            error               = ?,
            finished_at         = strftime('%Y-%m-%dT%H:%M:%SZ', 'now'),
            sessions_processed  = ?,
            summaries_written   = ?,
            links_written       = ?,
            dispatches_queued   = ?,
            dispatches_sent     = ?
        WHERE id = ?
        """,
        (
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


# ---------------------------------------------------------------------------
# Reads
# ---------------------------------------------------------------------------


def _parse_json(value: Any, fallback: Any) -> Any:
    if value is None:
        return fallback
    try:
        return json.loads(value)
    except (TypeError, ValueError):
        return fallback


async def count_new_sessions(conn: aiosqlite.Connection, *, after_id: int) -> int:
    """Cheap watchdog query — no JOIN, no JSON parsing."""
    cur = await conn.execute(
        "SELECT COUNT(*) AS n FROM app_sessions WHERE id > ?",
        (after_id,),
    )
    row = await cur.fetchone()
    await cur.close()
    return int(row["n"]) if row else 0


async def fetch_unanalysed_sessions(
    conn: aiosqlite.Connection,
    *,
    after_id: int,
    limit: int = 50,
) -> list[Session]:
    """Closed app_sessions newer than the cursor that have no ticket_links row.

    The Rust ETL only inserts a row into app_sessions when the block is
    closed (app switch or gap), so every row this returns is final and
    safe to analyse.
    """
    cur = await conn.execute(
        """
        SELECT s.id, s.app_name, s.started_at, s.ended_at, s.duration_s,
               s.window_titles, s.ocr_samples, s.audio_snippets,
               s.category, s.confidence
        FROM app_sessions s
        LEFT JOIN ticket_links t ON t.session_id = s.id
        WHERE s.id > ?
          AND t.session_id IS NULL
        ORDER BY s.id ASC
        LIMIT ?
        """,
        (after_id, limit),
    )
    rows = await cur.fetchall()
    await cur.close()
    return [
        Session(
            id=row["id"],
            app_name=row["app_name"],
            started_at=row["started_at"],
            ended_at=row["ended_at"],
            duration_s=row["duration_s"],
            window_titles=_parse_json(row["window_titles"], []),
            ocr_samples=_parse_json(row["ocr_samples"], []),
            audio_snippets=_parse_json(row["audio_snippets"], []),
            category=row["category"],
            confidence=float(row["confidence"]),
        )
        for row in rows
    ]


async def fetch_pm_tasks(
    conn: aiosqlite.Connection,
    *,
    only_fresh: bool = True,
    provider: str | None = None,
    exclude_done: bool = True,
) -> list[PmTask]:
    """Cached PM tasks the Rust intelligence layer has fetched.

    `only_fresh=True` — drops rows past `expires_at`.
    `provider='jira'` — narrows to a single provider (defaults to all).
    `exclude_done=True` — drops `status_category='done'` rows so we only
    propose open tickets to the synthesizer.
    """
    sql = [
        "SELECT task_key, provider, title, description_text, status,",
        "       status_category, issue_type, project_key, url, updated_at",
        "FROM pm_tasks",
    ]
    where: list[str] = []
    params: list[Any] = []
    if only_fresh:
        where.append("expires_at > strftime('%Y-%m-%dT%H:%M:%SZ', 'now')")
    if provider is not None:
        where.append("provider = ?")
        params.append(provider)
    if exclude_done:
        where.append("status_category != 'done'")
    if where:
        sql.append("WHERE " + " AND ".join(where))
    sql.append("ORDER BY updated_at DESC")
    cur = await conn.execute(" ".join(sql), tuple(params))
    rows = await cur.fetchall()
    await cur.close()
    return [PmTask(**dict(row)) for row in rows]


async def read_activity_context(
    conn: aiosqlite.Connection,
) -> ActivityContext | None:
    cur = await conn.execute(
        """
        SELECT updated_at, active_project, jira_key, inferred_task,
               confidence, trigger_jira_sync, tags, last_synced
        FROM activity_context
        WHERE id = 1
        """
    )
    row = await cur.fetchone()
    await cur.close()
    if row is None:
        return None
    return ActivityContext(
        updated_at=row["updated_at"],
        active_project=row["active_project"],
        jira_key=row["jira_key"],
        inferred_task=row["inferred_task"],
        confidence=float(row["confidence"]),
        trigger_jira_sync=bool(row["trigger_jira_sync"]),
        tags=_parse_json(row["tags"], []),
        last_synced=row["last_synced"],
    )


async def fetch_context_nodes(
    conn: aiosqlite.Connection,
    *,
    node_type: str | None = None,
    limit: int = 200,
) -> list[ContextNode]:
    sql = (
        "SELECT node_id, node_type, label, last_seen, frequency, confidence_avg "
        "FROM context_graph_nodes"
    )
    params: tuple[Any, ...] = ()
    if node_type is not None:
        if node_type not in VALID_NODE_TYPES:
            raise ValueError(f"fetch_context_nodes: invalid node_type {node_type!r}")
        sql += " WHERE node_type = ?"
        params = (node_type,)
    sql += " ORDER BY last_seen DESC LIMIT ?"
    params = (*params, limit)
    cur = await conn.execute(sql, params)
    rows = await cur.fetchall()
    await cur.close()
    return [
        ContextNode(
            node_id=row["node_id"],
            node_type=row["node_type"],
            label=row["label"],
            last_seen=row["last_seen"],
            frequency=int(row["frequency"]),
            confidence_avg=float(row["confidence_avg"]),
        )
        for row in rows
    ]


# ---------------------------------------------------------------------------
# Writes
# ---------------------------------------------------------------------------


async def write_session_summary(
    conn: aiosqlite.Connection,
    *,
    session_id: int,
    agent_run_id: int,
    summary_json: str,
) -> int:
    """Insert one LLM-derived summary for a session.

    Schema enforces UNIQUE on session_id, so a second call for the same
    session raises `aiosqlite.IntegrityError` (caller decides whether to
    treat that as idempotent).
    """
    cur = await conn.execute(
        """
        INSERT INTO session_summaries (session_id, agent_run_id, summary_json)
        VALUES (?, ?, ?)
        """,
        (session_id, agent_run_id, summary_json),
    )
    summary_id = cur.lastrowid
    await cur.close()
    assert summary_id is not None
    return summary_id


async def link_ticket(
    conn: aiosqlite.Connection,
    *,
    session_id: int,
    task_key: str | None,
    provider: str | None,
    confidence: float,
    routing: str,
    session_type: str = "task",
) -> None:
    """Insert a ticket_links row produced by an LLM matcher.

    `method='llm'` is hardcoded. Every closed session gets exactly one
    ticket_links row (UNIQUE on session_id) — see the contract in the
    project plan for the four valid (session_type, task_key, routing)
    combinations.
    """
    if routing not in VALID_ROUTINGS:
        raise ValueError(f"link_ticket: invalid routing {routing!r}")
    if session_type not in VALID_SESSION_TYPES:
        raise ValueError(f"link_ticket: invalid session_type {session_type!r}")
    await conn.execute(
        """
        INSERT INTO ticket_links
            (session_id, task_key, provider, method, confidence, session_type, routing)
        VALUES (?, ?, ?, 'llm', ?, ?, ?)
        """,
        (session_id, task_key, provider, confidence, session_type, routing),
    )


async def upsert_context_node(
    conn: aiosqlite.Connection,
    *,
    node_id: str,
    node_type: str,
    label: str,
    last_seen: str | None = None,
) -> None:
    """Upsert a knowledge-graph node.

    On conflict (`node_id` UNIQUE), label and last_seen are refreshed and
    frequency is incremented. confidence_avg stays at its previous value
    — refining the smoothing algorithm is a v2 concern.
    """
    if node_type not in VALID_NODE_TYPES:
        raise ValueError(f"upsert_context_node: invalid node_type {node_type!r}")
    await conn.execute(
        """
        INSERT INTO context_graph_nodes (node_id, node_type, label, last_seen)
        VALUES (?, ?, ?, COALESCE(?, strftime('%Y-%m-%dT%H:%M:%SZ', 'now')))
        ON CONFLICT (node_id) DO UPDATE SET
            label     = excluded.label,
            last_seen = excluded.last_seen,
            frequency = frequency + 1
        """,
        (node_id, node_type, label, last_seen),
    )


async def write_activity_context(
    conn: aiosqlite.Connection,
    *,
    inferred_task: str,
    confidence: float,
    trigger_jira_sync: bool = False,
    active_project: str | None = None,
    jira_key: str | None = None,
    tags: list[Any] | None = None,
) -> None:
    """Update the single-row activity_context.

    Synthesizer overwrites this every tick; jira-keeper updates only
    `last_synced` + `trigger_jira_sync` via `mark_activity_context_synced`.
    """
    tags_json = json.dumps(tags) if tags is not None else None
    await conn.execute(
        """
        UPDATE activity_context
        SET updated_at        = strftime('%Y-%m-%dT%H:%M:%SZ', 'now'),
            active_project    = ?,
            jira_key          = ?,
            inferred_task     = ?,
            confidence        = ?,
            trigger_jira_sync = ?,
            tags              = ?
        WHERE id = 1
        """,
        (
            active_project,
            jira_key,
            inferred_task,
            confidence,
            1 if trigger_jira_sync else 0,
            tags_json,
        ),
    )


async def mark_activity_context_synced(
    conn: aiosqlite.Connection,
    *,
    last_synced: str | None = None,
) -> None:
    """Called by jira-keeper after a successful dispatch.

    Clears `trigger_jira_sync` and stamps `last_synced`. Defaults to NOW
    when `last_synced` is omitted.
    """
    await conn.execute(
        """
        UPDATE activity_context
        SET trigger_jira_sync = 0,
            last_synced       = COALESCE(?, strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
        WHERE id = 1
        """,
        (last_synced,),
    )


async def enqueue_dispatch(
    conn: aiosqlite.Connection,
    *,
    session_id: int,
    agent_run_id: int,
    task_key: str,
    provider: str,
    payload_json: str,
) -> int:
    if provider not in VALID_DISPATCH_PROVIDERS:
        raise ValueError(f"enqueue_dispatch: invalid provider {provider!r}")
    cur = await conn.execute(
        """
        INSERT INTO dispatch_queue
            (session_id, agent_run_id, task_key, provider, payload_json)
        VALUES (?, ?, ?, ?, ?)
        """,
        (session_id, agent_run_id, task_key, provider, payload_json),
    )
    dispatch_id = cur.lastrowid
    await cur.close()
    assert dispatch_id is not None
    return dispatch_id


async def claim_dispatch_pending(
    conn: aiosqlite.Connection, *, limit: int = 25
) -> list[DispatchItem]:
    cur = await conn.execute(
        """
        SELECT id, session_id, agent_run_id, task_key, provider, payload_json, attempts
        FROM dispatch_queue
        WHERE state = 'pending'
        ORDER BY id ASC
        LIMIT ?
        """,
        (limit,),
    )
    rows = await cur.fetchall()
    await cur.close()
    return [DispatchItem(**dict(row)) for row in rows]


async def mark_dispatched(
    conn: aiosqlite.Connection,
    *,
    dispatch_id: int,
    state: str,
    error: str | None = None,
) -> None:
    """Move a dispatch row to a terminal state.

    `attempts` is incremented atomically. `dispatched_at` is stamped only
    when state == 'sent'; failures leave it NULL so retries are visible.
    """
    if state not in VALID_DISPATCH_STATES:
        raise ValueError(f"mark_dispatched: invalid state {state!r}")
    await conn.execute(
        """
        UPDATE dispatch_queue
        SET state         = ?,
            last_error    = ?,
            attempts      = attempts + 1,
            dispatched_at = CASE WHEN ? = 'sent'
                                 THEN strftime('%Y-%m-%dT%H:%M:%SZ', 'now')
                                 ELSE dispatched_at
                            END
        WHERE id = ?
        """,
        (state, error, state, dispatch_id),
    )
