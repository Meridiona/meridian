"""SQLite read/write layer for coding_agent_indexer.

Owns inserts to `app_sessions` for Claude / Codex sessions (rows carry a
non-NULL `claude_session_uuid`). Per-day chunking: a session spanning
multiple local days produces one row per day, keyed on
`(claude_session_uuid, day_utc)`.

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
from coding_agent_indexer.jsonl_meta import DaySlice

log = logging.getLogger(__name__)

# Sentinel for rows we own — not produced by any ETL run.
_INDEXER_ETL_RUN_ID = 0

# Static defaults for columns that exist for screen-frame rows but are
# meaningless for a Claude / Codex session.
_EMPTY_JSON_LIST = "[]"
_CATEGORY        = "coding"                # we are sure: this IS coding work
_CATEGORY_METHOD = "coding_agent_indexer"

# `task_method` is set non-NULL on insert so the Rust MLX classifier
# skips these rows — its query is `WHERE task_method IS NULL`. The
# summariser (phase 2, Claude API) picks them up by querying
# `WHERE task_method = 'pending_summariser'`.
_TASK_METHOD_PENDING = "pending_summariser"

# Map agent flavour → app_name. We use the agent's product name, not
# the host terminal/IDE: a Claude Code session is `Claude Code` whether
# you ran it inside VS Code, iTerm2, or any other terminal.
_APP_NAME_BY_AGENT = {
    "claude_code": "Claude Code",
    "codex":       "Codex",
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


def upsert_session_day_slice(
    slice_: DaySlice,
    *,
    db_path: Optional[Path] = None,
) -> Optional[int]:
    """INSERT or UPDATE one (uuid, day) row for a coding-agent session.

    Per-day rows: a long-running session that spans multiple calendar
    days produces one row per local day, so PM-update window queries
    (`WHERE started_at BETWEEN ?...`) correctly attribute time to the
    day the work actually happened.

    Live-tracking semantics: each tick can re-call this with a fresh
    slice; mutable fields (started_at, ended_at, duration_s,
    frame_count, session_text) get refreshed; classifier-owned fields
    (task_method, task_key, session_summary, …) are deliberately NOT
    touched on UPDATE so the summariser's work isn't clobbered.

    Idempotent under hook + poller races via the partial unique index
    on `(claude_session_uuid, day_utc)`.

    Args:
        slice_: parsed DaySlice (one day's slice of a session).
        db_path: target meridian.db; defaults to config.MERIDIAN_DB.

    Returns:
        The row's id (whether INSERTed or UPDATEd), or None if the
        slice was rejected as invalid (e.g. no turns).
    """
    if not slice_.is_valid:
        log.info(
            "skip upsert: slice invalid (uuid=%s day=%s user=%d asst=%d started=%r ended=%r)",
            slice_.session_uuid, slice_.day_utc, slice_.user_turns,
            slice_.assistant_turns, slice_.started_at, slice_.ended_at,
        )
        return None

    app_name     = _APP_NAME_BY_AGENT.get(slice_.agent, "Claude Code")
    transcript   = slice_.transcript
    session_text = transcript if transcript else None
    text_source  = "claude_jsonl" if transcript else None
    frame_count  = slice_.user_turns + slice_.assistant_turns

    with connect(db_path) as con:
        con.execute(
            """
            INSERT INTO app_sessions (
                app_name, started_at, ended_at, duration_s, day_utc,
                window_titles, min_frame_id, max_frame_id, frame_count,
                etl_run_id, idle_frame_count,
                category, confidence, category_method,
                session_text, session_text_source,
                task_method,
                claude_session_uuid
            )
            VALUES (?, ?, ?, ?, ?,  ?, ?, ?, ?,  ?, ?,  ?, ?, ?,  ?, ?,  ?,  ?)
            ON CONFLICT (claude_session_uuid, day_utc)
            WHERE claude_session_uuid IS NOT NULL
            DO UPDATE SET
                started_at          = excluded.started_at,
                ended_at            = excluded.ended_at,
                duration_s          = excluded.duration_s,
                frame_count         = excluded.frame_count,
                session_text        = excluded.session_text,
                session_text_source = excluded.session_text_source
            """,
            (
                app_name,
                slice_.started_at,
                slice_.ended_at,
                slice_.active_seconds,
                slice_.day_utc,
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
                _TASK_METHOD_PENDING,
                slice_.session_uuid,
            ),
        )
        row = con.execute(
            "SELECT id FROM app_sessions "
            "WHERE claude_session_uuid = ? AND day_utc = ?",
            (slice_.session_uuid, slice_.day_utc),
        ).fetchone()
        con.commit()
        return row["id"] if row else None


def delete_claude_session_rows(*, db_path: Optional[Path] = None) -> int:
    """Delete every Claude/Codex-owned app_sessions row.

    Used by the CLI `--reseed` flag to wipe stale per-(uuid, started_at)
    rows from a pre-migration-026 install so the next `--scan-once`
    re-registers them under the new per-day scheme. Only touches rows
    the indexer owns (`claude_session_uuid IS NOT NULL`); never touches
    screen-frame rows.

    Returns the number of rows deleted.
    """
    with connect(db_path) as con:
        cur = con.execute(
            "DELETE FROM app_sessions WHERE claude_session_uuid IS NOT NULL"
        )
        con.commit()
        return cur.rowcount or 0


def fetch_session_endpoints(*, db_path: Optional[Path] = None) -> dict[str, str]:
    """Return {claude_session_uuid: latest_ended_at_iso} for every registered row.

    Used by the daemon's change-detection (skip parsing a JSONL whose
    mtime hasn't moved past the stored `ended_at`). Cheap — uses the
    `idx_app_sessions_claude_uuid` partial index.
    """
    with connect(db_path, readonly=True) as con:
        rows = con.execute(
            "SELECT claude_session_uuid, MAX(ended_at) AS ended_at "
            "FROM   app_sessions "
            "WHERE  claude_session_uuid IS NOT NULL "
            "GROUP  BY claude_session_uuid"
        ).fetchall()
    return {r["claude_session_uuid"]: r["ended_at"] for r in rows if r["claude_session_uuid"]}
