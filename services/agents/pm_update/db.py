"""SQLite read/write layer for the pm_update workflow.

Read paths point at the existing tables owned by the Rust ETL
(`app_sessions`) and the intelligence module (`pm_tasks`). Write paths
own the three pm_update tables defined in `schema.sql`.

Connection model: short-lived `sqlite3.Connection` per call, mirroring
the pattern used by `run_task_linker_mlx`. SQLite WAL mode handles
concurrency between Rust and Python without explicit pooling.
"""
from __future__ import annotations

import json
import logging
import sqlite3
from contextlib import contextmanager
from datetime import datetime, timedelta, timezone
from pathlib import Path
from typing import Iterator, Optional

from agents.pm_update import config
from agents.pm_update.models import (
    BulletWithEvidence,
    JiraUpdate,
    SessionBundle,
    SessionDigest,
    UpdateState,
)

log = logging.getLogger(__name__)

# Cap for the per-session excerpt that travels into the LLM prompt. Full
# session_text is always available on demand via get_session_evidence.
EXCERPT_CAP_BYTES = 2_000


# ──────────────────────── Connection helper ────────────────────────────────────


@contextmanager
def connect(db_path: Path = config.MERIDIAN_DB, *, readonly: bool = False) -> Iterator[sqlite3.Connection]:
    """Open a short-lived connection with row_factory + WAL + busy_timeout.

    Read-only mode uses the URI form so we can never accidentally write
    when the caller only meant to read.
    """
    if readonly:
        # The URI form lets sqlite3 enforce read-only at the driver layer.
        uri = f"file:{db_path}?mode=ro"
        conn = sqlite3.connect(uri, uri=True, timeout=10)
    else:
        conn = sqlite3.connect(db_path, timeout=10)
        conn.execute("PRAGMA journal_mode=WAL")
    conn.row_factory = sqlite3.Row
    conn.execute("PRAGMA busy_timeout=5000")
    try:
        yield conn
    finally:
        conn.close()


# ──────────────────────── Schema bootstrap ─────────────────────────────────────


def init_schema(db_path: Path = config.MERIDIAN_DB) -> None:
    """Apply schema.sql idempotently. Safe to call on every CLI invocation.

    Also performs additive column migrations for databases that were
    created before a column existed. SQLite has no `ADD COLUMN IF NOT
    EXISTS`, so we probe `PRAGMA table_info` and only ALTER when
    missing.
    """
    ddl = (Path(__file__).parent / "schema.sql").read_text()
    with connect(db_path) as con:
        con.executescript(ddl)

        # Backfill column for DBs that pre-date posted_worklog_id. This
        # one stays here because pm_updates is Python-owned (no Rust
        # migration manages it).
        cols = {row["name"] for row in con.execute("PRAGMA table_info(pm_updates)")}
        if "posted_worklog_id" not in cols:
            con.execute("ALTER TABLE pm_updates ADD COLUMN posted_worklog_id TEXT")
            log.info("added posted_worklog_id column to pm_updates")

        # NOTE: `session_summary` on app_sessions is owned by the Rust
        # migration `024_session_summary.sql`. Adding it from Python here
        # would race the migration runner and trip "duplicate column" on
        # daemon startup — don't.

        # Worklog idempotency: at most one POSTED worklog per (task, window).
        # Created after the ALTER above so the column always exists.
        con.execute(
            """
            CREATE UNIQUE INDEX IF NOT EXISTS uq_pm_updates_worklog_window
                ON pm_updates (task_key, window_start, window_end)
                WHERE posted_worklog_id IS NOT NULL
            """
        )

        con.commit()
    log.debug("pm_update schema applied to %s", db_path)


# ──────────────────────── Read paths ───────────────────────────────────────────


def fetch_session_bundle(
    task_key: str,
    window_start: datetime,
    window_end: datetime,
    cycle_index: int = 0,
    *,
    db_path: Path = config.MERIDIAN_DB,
) -> SessionBundle:
    """Build a `SessionBundle` for one (task_key, window).

    Includes only sessions where `task_key` matches and `started_at` falls
    inside the window. Drops sessions whose `session_text` is empty since
    those carry no narrative signal.
    """
    start_iso = _utc_iso(window_start)
    end_iso = _utc_iso(window_end)

    with connect(db_path, readonly=True) as con:
        # Ticket metadata for context-aware prompts. Missing pm_task is
        # acceptable — the PM cache may not have refreshed yet.
        task_row = con.execute(
            """
            SELECT title, status_category, assignee_name
            FROM pm_tasks
            WHERE task_key = ?
            """,
            (task_key,),
        ).fetchone()

        rows = con.execute(
            """
            SELECT id, app_name, started_at, ended_at, duration_s,
                   idle_frame_count, frame_count, window_titles,
                   session_text, session_text_source, category,
                   session_summary,
                   COALESCE(task_session_type, '') AS task_session_type
            FROM app_sessions
            WHERE task_key = ?
              AND started_at >= ?
              AND started_at <  ?
              AND COALESCE(task_session_type, '') = 'task'
            ORDER BY id ASC
            """,
            (task_key, start_iso, end_iso),
        ).fetchall()

        dim_rows = con.execute(
            """
            SELECT session_id, dimension, value
            FROM session_dimensions
            WHERE session_id IN (
                SELECT id FROM app_sessions
                WHERE task_key = ? AND started_at >= ? AND started_at < ?
            )
            """,
            (task_key, start_iso, end_iso),
        ).fetchall()

    # Group dimensions by session_id → {dimension: [values]}
    dims_by_session: dict[int, dict[str, list[str]]] = {}
    for r in dim_rows:
        s = dims_by_session.setdefault(r["session_id"], {})
        s.setdefault(r["dimension"], []).append(r["value"])

    digests: list[SessionDigest] = []
    raw_bytes = 0
    total_s = 0
    real_s = 0
    for r in rows:
        text = r["session_text"] or ""
        summary = (r["session_summary"] or "").strip()

        # Skip rows that have neither a classifier summary nor raw text.
        # The summary is the primary PM-update signal; falling back to the
        # 2KB raw excerpt only matters for legacy rows (pre-migration 024).
        if not summary and not text.strip():
            continue

        # Idle discount per session: scale duration by the non-idle frame
        # ratio. frame_count == 0 shouldn't happen for a real session, but
        # we guard against div-by-zero anyway.
        fc = r["frame_count"] or 0
        ifc = r["idle_frame_count"] or 0
        idle_share = (ifc / fc) if fc > 0 else 0.0
        real_session_s = int(round(r["duration_s"] * (1.0 - idle_share)))

        # Prefer the classifier-written prose summary over the OCR excerpt:
        #   - present:  use the full summary, no truncation (it's already
        #               sized to the PM-update budget by the classifier
        #               schema's max_length=8000).
        #   - missing:  fall back to the legacy 2KB OCR excerpt so old
        #               sessions still work end-to-end.
        digest_excerpt = summary if summary else text[:EXCERPT_CAP_BYTES]

        digests.append(
            SessionDigest(
                id=r["id"],
                app_name=r["app_name"],
                started_at=r["started_at"],
                ended_at=r["ended_at"],
                duration_s=r["duration_s"],
                idle_frame_s=int(round(r["duration_s"] * idle_share)),
                top_titles=_parse_top_titles(r["window_titles"]),
                dimensions=dims_by_session.get(r["id"], {}),
                excerpt=digest_excerpt,
                category=r["category"],
                text_source="summary" if summary else r["session_text_source"],
            )
        )
        # raw_bytes still measures the raw_text footprint (drives is_heavy);
        # the synth prompt size is dominated by `digest_excerpt`.
        raw_bytes += len(text)
        total_s += r["duration_s"]
        real_s += real_session_s

    is_heavy = (
        len(digests) > config.PM_UPDATE_HEAVY_SESSION_COUNT
        or raw_bytes > config.PM_UPDATE_HEAVY_TEXT_BYTES
    )

    return SessionBundle(
        task_key=task_key,
        window_start=start_iso,
        window_end=end_iso,
        cycle_index=cycle_index,
        sessions=digests,
        total_seconds=total_s,
        real_seconds=real_s,
        raw_text_bytes=raw_bytes,
        is_heavy=is_heavy,
        pm_task_status=task_row["status_category"] if task_row else None,
        pm_task_title=task_row["title"] if task_row else None,
        assignee_name=task_row["assignee_name"] if task_row else None,
        earlier_today_summaries=_fetch_earlier_today_summaries(
            task_key, window_start, db_path=db_path
        ),
    )


def fetch_session_text(session_id: int, *, db_path: Path = config.MERIDIAN_DB) -> str:
    """Return full session_text for one session_id, or empty string if absent.

    Used by the `get_session_evidence` agno tool so the LLM can pull the
    raw OCR/audio excerpt when its 2KB digest preview isn't enough.
    """
    with connect(db_path, readonly=True) as con:
        row = con.execute(
            "SELECT session_text FROM app_sessions WHERE id = ?", (session_id,),
        ).fetchone()
    if row is None:
        return ""
    return row["session_text"] or ""


def fetch_pm_task(task_key: str, *, db_path: Path = config.MERIDIAN_DB) -> Optional[dict]:
    """Current cached state of a Jira ticket from `pm_tasks`."""
    with connect(db_path, readonly=True) as con:
        row = con.execute(
            """
            SELECT task_key, provider, title, description_text, status_category,
                   issue_type, project_key, url, parent_key, epic_title,
                   sprint_name, assignee_name, fetched_at
            FROM pm_tasks WHERE task_key = ?
            """,
            (task_key,),
        ).fetchone()
    return dict(row) if row else None


def _fetch_earlier_today_summaries(
    task_key: str,
    window_start: datetime,
    *,
    db_path: Path,
) -> list[str]:
    """Headlines of earlier-today cycles, oldest → newest.

    Used by the Synth prompt to avoid repeating what was already posted.
    """
    day = window_start.astimezone(timezone.utc).strftime("%Y-%m-%d")
    with connect(db_path, readonly=True) as con:
        rows = con.execute(
            """
            SELECT payload_json
            FROM pm_updates
            WHERE task_key = ? AND day_utc = ? AND state = ?
            ORDER BY cycle_index ASC
            """,
            (task_key, day, UpdateState.POSTED.value),
        ).fetchall()
    summaries: list[str] = []
    for r in rows:
        try:
            payload = json.loads(r["payload_json"])
            if summary := payload.get("summary"):
                summaries.append(summary)
        except (json.JSONDecodeError, KeyError):
            continue
    return summaries


# ──────────────────────── Write paths ──────────────────────────────────────────


def upsert_pm_update(
    update: JiraUpdate,
    *,
    state: UpdateState,
    coverage: float,
    workflow_run_id: Optional[str] = None,
    session_id_min: Optional[int] = None,
    session_id_max: Optional[int] = None,
    db_path: Path = config.MERIDIAN_DB,
) -> int:
    """Insert or update the pm_updates row for this (task, day, cycle).

    Returns the row's `id`. Idempotent on `(task_key, day_utc, cycle_index)`.
    """
    day = _day_utc(update.window_start)
    payload = update.model_dump_json()

    with connect(db_path) as con:
        cur = con.execute(
            """
            INSERT INTO pm_updates (
                task_key, day_utc, cycle_index, window_start, window_end,
                state, confidence, coverage, time_spent_seconds,
                payload_json, session_id_min, session_id_max, workflow_run_id
            )
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT (task_key, day_utc, cycle_index)
            DO UPDATE SET
                state              = excluded.state,
                confidence         = excluded.confidence,
                coverage           = excluded.coverage,
                time_spent_seconds = excluded.time_spent_seconds,
                payload_json       = excluded.payload_json,
                workflow_run_id    = excluded.workflow_run_id
            RETURNING id
            """,
            (
                update.task_key,
                day,
                update.cycle_index,
                update.window_start,
                update.window_end,
                state.value,
                update.confidence,
                coverage,
                update.time_spent_seconds,
                payload,
                session_id_min,
                session_id_max,
                workflow_run_id,
            ),
        )
        row = cur.fetchone()
        pm_update_id = row["id"] if row else cur.lastrowid

        # Refresh evidence table — drop old refs, insert fresh.
        con.execute("DELETE FROM pm_update_evidence WHERE pm_update_id = ?", (pm_update_id,))
        for kind, bullets in (
            ("shipped",     update.what_shipped),
            ("in_progress", update.in_progress),
            ("blocker",     update.blockers),
            ("decision",    update.decisions),
        ):
            for idx, b in enumerate(bullets):
                _insert_evidence(con, pm_update_id, kind, idx, b)
        con.commit()
    return pm_update_id


def mark_posted(
    pm_update_id: int,
    posted_comment_id: str,
    *,
    db_path: Path = config.MERIDIAN_DB,
) -> None:
    """Stamp the row with a Jira comment id once the post lands."""
    with connect(db_path) as con:
        con.execute(
            """
            UPDATE pm_updates
            SET state = ?, posted_comment_id = ?, posted_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now')
            WHERE id = ?
            """,
            (UpdateState.POSTED.value, posted_comment_id, pm_update_id),
        )
        con.commit()


def mark_worklog_posted(
    pm_update_id: int,
    posted_worklog_id: str,
    *,
    db_path: Path = config.MERIDIAN_DB,
) -> None:
    """Stamp the row with the Jira worklog id once the post lands.

    Separate from `mark_posted` because worklog and comment are
    independent phases — phase 1 ships only worklog. Setting either
    one's id is enough to flip `state` to POSTED.
    """
    with connect(db_path) as con:
        con.execute(
            """
            UPDATE pm_updates
            SET state = ?,
                posted_worklog_id = ?,
                posted_at = COALESCE(posted_at, strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
            WHERE id = ?
            """,
            (UpdateState.POSTED.value, posted_worklog_id, pm_update_id),
        )
        con.commit()


def find_existing_worklog(
    task_key: str,
    window_start: str,
    window_end: str,
    *,
    db_path: Path = config.MERIDIAN_DB,
) -> Optional[tuple[int, str]]:
    """Look up any prior worklog post for this exact (task, window).

    Returns (pm_update_id, posted_worklog_id) or None. Used by the
    Route step to short-circuit duplicate posts after daemon restarts
    or backfill replays.
    """
    with connect(db_path, readonly=True) as con:
        row = con.execute(
            """
            SELECT id, posted_worklog_id FROM pm_updates
            WHERE task_key = ?
              AND window_start = ?
              AND window_end   = ?
              AND posted_worklog_id IS NOT NULL
            LIMIT 1
            """,
            (task_key, window_start, window_end),
        ).fetchone()
    if row is None:
        return None
    return row["id"], row["posted_worklog_id"]


def record_feedback(
    pm_update_id: int,
    *,
    feedback_kind: str,
    original_text: Optional[str] = None,
    edited_text: Optional[str] = None,
    note: Optional[str] = None,
    db_path: Path = config.MERIDIAN_DB,
) -> int:
    """Record an admin edit / rejection / approval. Fuels self-learning."""
    if feedback_kind not in ("edit", "reject", "approve"):
        raise ValueError(f"unknown feedback_kind: {feedback_kind}")
    with connect(db_path) as con:
        cur = con.execute(
            """
            INSERT INTO pm_update_feedback
                (pm_update_id, feedback_kind, original_text, edited_text, note)
            VALUES (?, ?, ?, ?, ?)
            """,
            (pm_update_id, feedback_kind, original_text, edited_text, note),
        )
        con.commit()
        return cur.lastrowid


def fetch_recent_feedback(
    task_key: str,
    *,
    limit: int = 5,
    db_path: Path = config.MERIDIAN_DB,
) -> list[dict]:
    """Recent feedback rows for one ticket, newest first.

    Synth pre_hook injects these as few-shot guidance — that's the
    closing loop on self-improvement.
    """
    with connect(db_path, readonly=True) as con:
        rows = con.execute(
            """
            SELECT f.id, f.feedback_kind, f.original_text, f.edited_text,
                   f.note, f.created_at
            FROM pm_update_feedback f
            JOIN pm_updates u ON u.id = f.pm_update_id
            WHERE u.task_key = ?
            ORDER BY f.created_at DESC
            LIMIT ?
            """,
            (task_key, limit),
        ).fetchall()
    return [dict(r) for r in rows]


# ──────────────────────── Helpers ──────────────────────────────────────────────


def _utc_iso(dt: datetime) -> str:
    """ISO-8601 UTC with 'Z' suffix, matching the rest of meridian.db."""
    if dt.tzinfo is None:
        dt = dt.replace(tzinfo=timezone.utc)
    return dt.astimezone(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def _day_utc(iso_ts: str) -> str:
    """Strip an ISO timestamp to YYYY-MM-DD in UTC."""
    # Tolerant of both '...Z' and '+00:00' suffixes.
    cleaned = iso_ts.replace("Z", "+00:00")
    return datetime.fromisoformat(cleaned).astimezone(timezone.utc).strftime("%Y-%m-%d")


def _parse_top_titles(raw: Optional[str], *, n: int = 3) -> list[str]:
    """Extract the n most common window titles from the JSON column."""
    if not raw:
        return []
    try:
        parsed = json.loads(raw)
    except json.JSONDecodeError:
        return []
    if not isinstance(parsed, list):
        return []
    # Each element is {"title": str, "count": int}; sort desc by count.
    titles = sorted(
        (p for p in parsed if isinstance(p, dict) and p.get("title")),
        key=lambda p: p.get("count", 0),
        reverse=True,
    )
    return [p["title"] for p in titles[:n]]


def _insert_evidence(
    con: sqlite3.Connection,
    pm_update_id: int,
    bullet_kind: str,
    bullet_index: int,
    bullet: BulletWithEvidence,
) -> None:
    for session_id in bullet.evidence_refs:
        con.execute(
            """
            INSERT OR IGNORE INTO pm_update_evidence
                (pm_update_id, bullet_kind, bullet_index, session_id, excerpt)
            VALUES (?, ?, ?, ?, ?)
            """,
            (pm_update_id, bullet_kind, bullet_index, session_id, bullet.text[:400]),
        )


def last_posted_window_end(
    task_key: str,
    *,
    db_path: Path = config.MERIDIAN_DB,
) -> Optional[datetime]:
    """When was the most recent successful post for this ticket?

    Used by the daemon (later) to pick the next window's start. Returns
    None if no post has ever landed.
    """
    with connect(db_path, readonly=True) as con:
        row = con.execute(
            """
            SELECT window_end FROM pm_updates
            WHERE task_key = ? AND state = ?
            ORDER BY window_end DESC LIMIT 1
            """,
            (task_key, UpdateState.POSTED.value),
        ).fetchone()
    if row is None:
        return None
    cleaned = row["window_end"].replace("Z", "+00:00")
    return datetime.fromisoformat(cleaned)


def has_recent_classified_work(
    task_key: str,
    since: datetime,
    *,
    db_path: Path = config.MERIDIAN_DB,
) -> bool:
    """True if any classified session for `task_key` was written after `since`.

    Lets the daemon skip ticks where nothing has happened.
    """
    since_iso = _utc_iso(since)
    with connect(db_path, readonly=True) as con:
        row = con.execute(
            """
            SELECT 1 FROM app_sessions
            WHERE task_key = ? AND ended_at > ?
              AND COALESCE(task_session_type, '') = 'task'
            LIMIT 1
            """,
            (task_key, since_iso),
        ).fetchone()
    return row is not None
