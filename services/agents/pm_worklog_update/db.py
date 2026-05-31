# meridian — normalises screenpipe activity into structured app sessions
"""SQLite read/write layer for the pm_worklog_update workflow."""
from __future__ import annotations

import json
import logging
import sqlite3
from contextlib import contextmanager
from datetime import datetime, timedelta, timezone
from pathlib import Path
from typing import Iterator, Optional

from agents.pm_worklog_update import config
from agents.pm_worklog_update.models import (
    BulletWithEvidence,
    JiraUpdate,
    SessionBundle,
    SessionDigest,
    UpdateState,
)

log = logging.getLogger(__name__)

EXCERPT_CAP_BYTES = 2_000


@contextmanager
def connect(db_path: Path = config.MERIDIAN_DB, *, readonly: bool = False) -> Iterator[sqlite3.Connection]:
    if readonly:
        conn = sqlite3.connect(f"file:{db_path}?mode=ro", uri=True, timeout=10)
    else:
        conn = sqlite3.connect(db_path, timeout=10)
        conn.execute("PRAGMA journal_mode=WAL")
    conn.row_factory = sqlite3.Row
    conn.execute("PRAGMA busy_timeout=5000")
    try:
        yield conn
    finally:
        conn.close()


def init_schema(db_path: Path = config.MERIDIAN_DB) -> None:
    """Apply schema.sql idempotently."""
    ddl = (Path(__file__).parent / "schema.sql").read_text()
    with connect(db_path) as con:
        # Rename tables from old pm_update naming if they still exist.
        existing = {r[0] for r in con.execute("SELECT name FROM sqlite_master WHERE type='table'")}
        for old, new in [
            ("pm_updates",         "pm_worklogs"),
            ("pm_update_evidence", "pm_worklog_evidence"),
            ("pm_update_feedback", "pm_worklog_feedback"),
        ]:
            if old in existing and new not in existing:
                con.execute(f"ALTER TABLE {old} RENAME TO {new}")
                log.info("renamed table %s → %s", old, new)
        con.commit()

        con.executescript(ddl)

        cols = {r["name"] for r in con.execute("PRAGMA table_info(pm_worklogs)")}
        if "posted_worklog_id" not in cols:
            con.execute("ALTER TABLE pm_worklogs ADD COLUMN posted_worklog_id TEXT")
            log.info("added posted_worklog_id column to pm_worklogs")

        # Rename pm_update_id → pm_worklog_id in evidence/feedback tables if needed.
        # SQLite has no RENAME COLUMN before 3.25; use a table rebuild.
        for tbl in ("pm_worklog_evidence", "pm_worklog_feedback"):
            tcols = {r["name"] for r in con.execute(f"PRAGMA table_info({tbl})")}
            if "pm_update_id" in tcols and "pm_worklog_id" not in tcols:
                con.executescript(f"""
                    ALTER TABLE {tbl} RENAME TO _{tbl}_old;
                """)
                log.info("dropped old %s (will recreate with pm_worklog_id)", tbl)
        con.commit()

        # Re-run DDL to recreate any tables that were just dropped.
        con.executescript(ddl)

        con.execute(
            """
            CREATE UNIQUE INDEX IF NOT EXISTS uq_pm_worklogs_worklog_window
                ON pm_worklogs (task_key, window_start, window_end)
                WHERE posted_worklog_id IS NOT NULL
            """
        )
        con.commit()
    log.debug("pm_worklog schema applied to %s", db_path)


def fetch_session_bundle(
    task_key: str,
    window_start: datetime,
    window_end: datetime,
    cycle_index: int = 0,
    *,
    db_path: Path = config.MERIDIAN_DB,
) -> SessionBundle:
    start_iso = _utc_iso(window_start)
    end_iso   = _utc_iso(window_end)

    with connect(db_path, readonly=True) as con:
        task_row = con.execute(
            """
            SELECT title, status_category, assignee_name,
                   COALESCE(description_text, '') AS description_text
            FROM pm_tasks WHERE task_key = ?
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

    dims_by_session: dict[int, dict[str, list[str]]] = {}
    for r in dim_rows:
        dims_by_session.setdefault(r["session_id"], {}).setdefault(r["dimension"], []).append(r["value"])

    digests: list[SessionDigest] = []
    raw_bytes = total_s = real_s = 0

    for r in rows:
        text    = r["session_text"] or ""
        summary = (r["session_summary"] or "").strip()
        if not summary and not text.strip():
            continue

        fc  = r["frame_count"] or 0
        ifc = r["idle_frame_count"] or 0
        idle_share      = (ifc / fc) if fc > 0 else 0.0
        real_session_s  = int(round(r["duration_s"] * (1.0 - idle_share)))

        excerpt    = summary if summary else text[:EXCERPT_CAP_BYTES]
        text_src   = "summary" if summary else r["session_text_source"]

        digests.append(SessionDigest(
            id=r["id"],
            app_name=r["app_name"],
            started_at=r["started_at"],
            ended_at=r["ended_at"],
            duration_s=r["duration_s"],
            idle_frame_s=int(round(r["duration_s"] * idle_share)),
            top_titles=_parse_top_titles(r["window_titles"]),
            dimensions=dims_by_session.get(r["id"], {}),
            excerpt=excerpt,
            category=r["category"],
            text_source=text_src,
        ))
        raw_bytes += len(text)
        total_s   += r["duration_s"]
        real_s    += real_session_s

    is_heavy = (
        len(digests) > config.PM_WORKLOG_HEAVY_SESSION_COUNT
        or raw_bytes > config.PM_WORKLOG_HEAVY_TEXT_BYTES
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
        pm_task_description=task_row["description_text"][:1000] if task_row else None,
        assignee_name=task_row["assignee_name"] if task_row else None,
        earlier_today_summaries=_fetch_earlier_today_summaries(task_key, window_start, db_path=db_path),
    )


def fetch_session_text(session_id: int, *, db_path: Path = config.MERIDIAN_DB) -> str:
    with connect(db_path, readonly=True) as con:
        row = con.execute("SELECT session_text FROM app_sessions WHERE id = ?", (session_id,)).fetchone()
    return (row["session_text"] or "") if row else ""


def fetch_pm_task(task_key: str, *, db_path: Path = config.MERIDIAN_DB) -> Optional[dict]:
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
    task_key: str, window_start: datetime, *, db_path: Path
) -> list[str]:
    day = window_start.astimezone(timezone.utc).strftime("%Y-%m-%d")
    with connect(db_path, readonly=True) as con:
        rows = con.execute(
            """
            SELECT payload_json FROM pm_worklogs
            WHERE task_key = ? AND day_utc = ? AND state = ?
            ORDER BY cycle_index ASC
            """,
            (task_key, day, UpdateState.POSTED.value),
        ).fetchall()
    summaries = []
    for r in rows:
        try:
            payload = json.loads(r["payload_json"])
            if s := payload.get("summary"):
                summaries.append(s)
        except (json.JSONDecodeError, KeyError):
            continue
    return summaries


def upsert_pm_worklog(
    update: JiraUpdate,
    *,
    state: UpdateState,
    coverage: float,
    workflow_run_id: Optional[str] = None,
    session_id_min: Optional[int] = None,
    session_id_max: Optional[int] = None,
    db_path: Path = config.MERIDIAN_DB,
) -> int:
    day     = _day_utc(update.window_start)
    payload = update.model_dump_json()

    with connect(db_path) as con:
        cur = con.execute(
            """
            INSERT INTO pm_worklogs (
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
                update.task_key, day, update.cycle_index,
                update.window_start, update.window_end,
                state.value, update.confidence, coverage,
                update.time_spent_seconds, payload,
                session_id_min, session_id_max, workflow_run_id,
            ),
        )
        row = cur.fetchone()
        pm_worklog_id = row["id"] if row else cur.lastrowid

        con.execute("DELETE FROM pm_worklog_evidence WHERE pm_worklog_id = ?", (pm_worklog_id,))
        for kind, bullets in (
            ("shipped",     update.what_shipped),
            ("in_progress", update.in_progress),
            ("blocker",     update.blockers),
            ("decision",    update.decisions),
        ):
            for idx, b in enumerate(bullets):
                _insert_evidence(con, pm_worklog_id, kind, idx, b)
        con.commit()
    return pm_worklog_id


def mark_worklog_posted(
    pm_worklog_id: int, posted_worklog_id: str, *, db_path: Path = config.MERIDIAN_DB
) -> None:
    with connect(db_path) as con:
        con.execute(
            """
            UPDATE pm_worklogs
            SET state = ?, posted_worklog_id = ?,
                posted_at = COALESCE(posted_at, strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
            WHERE id = ?
            """,
            (UpdateState.POSTED.value, posted_worklog_id, pm_worklog_id),
        )
        con.commit()


def find_existing_worklog(
    task_key: str, window_start: str, window_end: str, *, db_path: Path = config.MERIDIAN_DB
) -> Optional[tuple[int, str]]:
    with connect(db_path, readonly=True) as con:
        row = con.execute(
            """
            SELECT id, posted_worklog_id FROM pm_worklogs
            WHERE task_key = ? AND window_start = ? AND window_end = ?
              AND posted_worklog_id IS NOT NULL
            LIMIT 1
            """,
            (task_key, window_start, window_end),
        ).fetchone()
    return (row["id"], row["posted_worklog_id"]) if row else None


def record_feedback(
    pm_worklog_id: int, *, feedback_kind: str,
    original_text: Optional[str] = None, edited_text: Optional[str] = None,
    note: Optional[str] = None, db_path: Path = config.MERIDIAN_DB,
) -> int:
    if feedback_kind not in ("edit", "reject", "approve"):
        raise ValueError(f"unknown feedback_kind: {feedback_kind}")
    with connect(db_path) as con:
        cur = con.execute(
            """
            INSERT INTO pm_worklog_feedback
                (pm_worklog_id, feedback_kind, original_text, edited_text, note)
            VALUES (?, ?, ?, ?, ?)
            """,
            (pm_worklog_id, feedback_kind, original_text, edited_text, note),
        )
        con.commit()
        return cur.lastrowid


def fetch_recent_feedback(
    task_key: str, *, limit: int = 5, db_path: Path = config.MERIDIAN_DB
) -> list[dict]:
    with connect(db_path, readonly=True) as con:
        rows = con.execute(
            """
            SELECT f.id, f.feedback_kind, f.original_text, f.edited_text, f.note, f.created_at
            FROM pm_worklog_feedback f
            JOIN pm_worklogs u ON u.id = f.pm_worklog_id
            WHERE u.task_key = ?
            ORDER BY f.created_at DESC LIMIT ?
            """,
            (task_key, limit),
        ).fetchall()
    return [dict(r) for r in rows]


def last_posted_window_end(
    task_key: str, *, db_path: Path = config.MERIDIAN_DB
) -> Optional[datetime]:
    with connect(db_path, readonly=True) as con:
        row = con.execute(
            """
            SELECT window_end FROM pm_worklogs
            WHERE task_key = ? AND state = ?
            ORDER BY window_end DESC LIMIT 1
            """,
            (task_key, UpdateState.POSTED.value),
        ).fetchone()
    if row is None:
        return None
    return datetime.fromisoformat(row["window_end"].replace("Z", "+00:00"))


def has_recent_classified_work(
    task_key: str, since: datetime, *, db_path: Path = config.MERIDIAN_DB
) -> bool:
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


# ── helpers ────────────────────────────────────────────────────────────────────

def _utc_iso(dt: datetime) -> str:
    if dt.tzinfo is None:
        dt = dt.replace(tzinfo=timezone.utc)
    return dt.astimezone(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def _day_utc(iso_ts: str) -> str:
    return datetime.fromisoformat(iso_ts.replace("Z", "+00:00")).astimezone(timezone.utc).strftime("%Y-%m-%d")


def _parse_top_titles(raw: Optional[str], *, n: int = 3) -> list[str]:
    if not raw:
        return []
    try:
        parsed = json.loads(raw)
    except json.JSONDecodeError:
        return []
    if not isinstance(parsed, list):
        return []
    titles = sorted(
        (p for p in parsed if isinstance(p, dict) and p.get("title")),
        key=lambda p: p.get("count", 0), reverse=True,
    )
    return [p["title"] for p in titles[:n]]


def _insert_evidence(
    con: sqlite3.Connection, pm_worklog_id: int,
    bullet_kind: str, bullet_index: int, bullet: BulletWithEvidence,
) -> None:
    for session_id in bullet.evidence_refs:
        con.execute(
            """
            INSERT OR IGNORE INTO pm_worklog_evidence
                (pm_worklog_id, bullet_kind, bullet_index, session_id, excerpt)
            VALUES (?, ?, ?, ?, ?)
            """,
            (pm_worklog_id, bullet_kind, bullet_index, session_id, bullet.text[:400]),
        )
