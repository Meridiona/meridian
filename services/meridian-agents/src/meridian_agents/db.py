# meridian — normalises screenpipe activity into structured app sessions

"""Async SQLite access layer for meridian-agents.

The Rust daemon owns the schema. This module only does SELECT/INSERT/UPDATE
against tables defined by `src/migrations/00{1..4}_*.sql`. WAL mode is
already enabled by the daemon; we set busy_timeout on each connection so
writes contend gracefully with the daemon's ETL writes.
"""

from __future__ import annotations

import json
from dataclasses import dataclass
from typing import Any

import aiosqlite

REQUIRED_TABLES = (
    "app_sessions",
    "ticket_links",
    "pm_tasks",
    "agent_runs",
    "agent_cursor",
    "dispatch_queue",
)
REQUIRED_APP_SESSIONS_COLUMNS = ("summary_json", "activity_kind")

# Open transactions block writers; default to 5s so the orchestrator backs off
# rather than failing immediately when the Rust daemon is mid-write.
BUSY_TIMEOUT_MS = 5000


class SchemaError(RuntimeError):
    """Raised when the meridian.db schema is missing tables or columns the
    agent service depends on. Almost always means the Rust daemon hasn't
    been run on this DB yet (or hasn't been rebuilt after a migration)."""


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
    activity_kind: str | None
    summary_json: str | None


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
    task_key: str
    provider: str
    payload_json: str
    attempts: int


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
    """Verify the DB has every table + column meridian-agents needs.

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
    cur = await conn.execute("PRAGMA table_info(app_sessions)")
    columns = {row["name"] async for row in cur}
    await cur.close()
    for col in REQUIRED_APP_SESSIONS_COLUMNS:
        if col not in columns:
            raise SchemaError(
                f"app_sessions is missing column {col!r} from migration 004. "
                "Rebuild and re-run the Rust daemon."
            )


# ---------------------------------------------------------------------------
# Cursor + run lifecycle
# ---------------------------------------------------------------------------


async def read_cursor(conn: aiosqlite.Connection) -> int:
    cur = await conn.execute("SELECT last_session_id FROM agent_cursor WHERE id = 1")
    row = await cur.fetchone()
    await cur.close()
    if row is None:
        # 004 seeds the row, but tolerate odd states for tests.
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
    if status not in ("success", "failed", "aborted"):
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
# Reads against existing tables
# ---------------------------------------------------------------------------


def _parse_json(value: Any, fallback: Any) -> Any:
    if value is None:
        return fallback
    try:
        return json.loads(value)
    except (TypeError, ValueError):
        return fallback


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
               s.activity_kind, s.summary_json
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
            activity_kind=row["activity_kind"],
            summary_json=row["summary_json"],
        )
        for row in rows
    ]


async def fetch_pm_tasks(
    conn: aiosqlite.Connection,
    *,
    only_fresh: bool = True,
) -> list[PmTask]:
    sql = """
        SELECT task_key, provider, title, description_text, status,
               status_category, issue_type, project_key, url, updated_at
        FROM pm_tasks
    """
    if only_fresh:
        sql += " WHERE expires_at > strftime('%Y-%m-%dT%H:%M:%SZ', 'now')"
    sql += " ORDER BY updated_at DESC"
    cur = await conn.execute(sql)
    rows = await cur.fetchall()
    await cur.close()
    return [PmTask(**dict(row)) for row in rows]


# ---------------------------------------------------------------------------
# Writes against agent-owned tables
# ---------------------------------------------------------------------------


async def write_summary(
    conn: aiosqlite.Connection, *, session_id: int, summary_json: str
) -> None:
    await conn.execute(
        "UPDATE app_sessions SET summary_json = ? WHERE id = ?",
        (summary_json, session_id),
    )


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

    `method='llm'` is hardcoded to distinguish from the rule-based matcher
    the Rust intelligence module may add later.
    """
    if routing not in ("auto", "queue", "skip"):
        raise ValueError(f"link_ticket: invalid routing {routing!r}")
    if session_type not in ("task", "overhead", "unknown"):
        raise ValueError(f"link_ticket: invalid session_type {session_type!r}")
    await conn.execute(
        """
        INSERT INTO ticket_links
            (session_id, task_key, provider, method, confidence, session_type, routing)
        VALUES (?, ?, ?, 'llm', ?, ?, ?)
        """,
        (session_id, task_key, provider, confidence, session_type, routing),
    )


async def enqueue_dispatch(
    conn: aiosqlite.Connection,
    *,
    session_id: int,
    task_key: str,
    provider: str,
    payload_json: str,
) -> int:
    if provider not in ("jira", "github", "linear", "log"):
        raise ValueError(f"enqueue_dispatch: invalid provider {provider!r}")
    cur = await conn.execute(
        """
        INSERT INTO dispatch_queue (session_id, task_key, provider, payload_json)
        VALUES (?, ?, ?, ?)
        """,
        (session_id, task_key, provider, payload_json),
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
        SELECT id, session_id, task_key, provider, payload_json, attempts
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
    if state not in ("sent", "failed", "skipped"):
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
