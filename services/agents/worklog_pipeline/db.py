"""DB layer for the worklog pipeline — candidate reads + draft persistence.

Reads the confirmed daily plan and open PM tasks (candidates), and writes
worklog drafts to pm_worklogs (the existing UI/approval surface) and proposed
tasks to pm_proposed_tasks. All writes are idempotent and never clobber a row a
human has already approved/posted — mirroring the daemon's draft-immutability rule.
"""
from __future__ import annotations

import json
import logging
import sqlite3
from pathlib import Path

from agents.time_utils import local_hour_utc_bounds

log = logging.getLogger("meridian.worklog.db")

# Worklog states a re-run may overwrite. Anything else (approved/posted/queued)
# is human-owned and left untouched.
_OVERWRITABLE = ("drafted", "skipped", "failed")


def open_db(db_path: str | Path) -> sqlite3.Connection:
    conn = sqlite3.connect(str(Path(db_path).expanduser()))
    conn.row_factory = sqlite3.Row
    conn.execute("PRAGMA foreign_keys = ON")
    return conn


def fetch_confirmed_plan(conn: sqlite3.Connection, plan_date: str) -> list[str]:
    """Return the confirmed daily-plan task keys for ``plan_date`` (local day),
    ordered by position. Empty if the plan isn't confirmed or was skipped — the
    matcher then falls back to scanning the backlog (tier 2). Mirrors
    src/daily_plan.rs's confirmed/skipped gate.
    """
    try:
        meta = conn.execute(
            "SELECT confirmed_at, skipped FROM daily_plan_meta WHERE plan_date = ?",
            (plan_date,),
        ).fetchone()
        if not meta or meta["confirmed_at"] is None or meta["skipped"]:
            return []
        rows = conn.execute(
            "SELECT task_key FROM daily_plan WHERE plan_date = ? ORDER BY position",
            (plan_date,),
        ).fetchall()
        return [r["task_key"] for r in rows]
    except sqlite3.OperationalError:
        return []  # pre-migration-041 DB


def fetch_open_tasks(conn: sqlite3.Connection) -> list[dict]:
    """Open (non-terminal) PM tasks, excluding curation-excluded ones. Returns
    dicts with task_key/title/description_text/issue_type/epic_title.
    """
    try:
        rows = conn.execute(
            """
            SELECT pm_tasks.task_key, title, description_text, issue_type, epic_title
            FROM pm_tasks
            LEFT JOIN pm_task_curation c ON c.task_key = pm_tasks.task_key
            WHERE COALESCE(pm_tasks.is_terminal, 0) = 0
              AND (c.decision IS NULL OR c.decision != 'excluded')
            """
        ).fetchall()
    except sqlite3.OperationalError:
        rows = conn.execute(
            "SELECT task_key, title, description_text, issue_type, epic_title "
            "FROM pm_tasks WHERE COALESCE(is_terminal, 0) = 0"
        ).fetchall()
    return [dict(r) for r in rows]


def fetch_coding_summaries(conn: sqlite3.Connection, hour: str) -> list[dict]:
    """Coding-agent sessions for ``hour`` ('YYYY-MM-DDTHH') that already carry a
    summary, in time order. Returns dicts: started_at, app_name, task_key,
    session_summary.

    These are the clean, agent-written summaries the summariser produced. The
    worklog pipeline folds them into the activity summary VERBATIM (never
    re-compressed or rewritten), so the coding work the dev actually did reaches
    the matcher/worklog with its file names, ticket keys, and per-task detail
    intact. The readiness gate (Rust upstream_settled) ensures coding rows in the
    hour are summarised before the hour runs; a still-live row without a summary
    is simply skipped here.
    """
    utc_start, utc_end = local_hour_utc_bounds(hour)
    rows = conn.execute(
        """
        SELECT started_at, app_name, COALESCE(task_key, '') AS task_key,
               session_summary
        FROM app_sessions
        WHERE started_at >= ? AND started_at < ?
          AND coding_agent_session_uuid IS NOT NULL
          AND session_summary IS NOT NULL
          AND LENGTH(TRIM(session_summary)) > 0
        ORDER BY started_at
        """,
        (utc_start, utc_end),
    ).fetchall()
    return [dict(r) for r in rows]


def fetch_sessions_for_hour(conn: sqlite3.Connection, hour: str) -> list[dict]:
    """OCR/app sessions (non-coding) for the hour, for trace child spans."""
    try:
        utc_start, utc_end = local_hour_utc_bounds(hour)
        rows = conn.execute(
            """
            SELECT app_name, started_at, ended_at,
                   CAST(COALESCE(duration_s, 0) AS INTEGER) as duration_s
            FROM app_sessions
            WHERE started_at >= ? AND started_at < ?
              AND coding_agent_session_uuid IS NULL
            ORDER BY started_at
            LIMIT 25
            """,
            (utc_start, utc_end),
        ).fetchall()
    except sqlite3.OperationalError:
        rows = []
    return [dict(r) for r in rows]


def render_doc(task: dict) -> str:
    """Render a ticket into the text the reranker + matcher see."""
    desc = (task.get("description_text") or "").strip().replace("\n", " ")
    itype = task.get("issue_type") or "Task"
    epic = task.get("epic_title") or ""
    return f"[{itype}] {task.get('title', task['task_key'])}. Epic: {epic}. {desc}".strip()


def build_payload(task_key: str, window_start: str, window_end: str,
                  cycle_index: int, time_spent_seconds: int, draft,
                  reasoning: str = "") -> dict:
    """Compose the JiraUpdate-shaped payload_json the UI reads from a WorklogDraft.

    Bullet lists are wrapped as ``[{"text": ...}]`` (the UI's RawBullet shape);
    evidence_refs are omitted — the hour pipeline grounds on the report, not
    per-session ids. ``reasoning`` is the WHY this worklog maps to its task — the
    matcher's ``why`` for a matched task, or the proposer's reasoning for a new
    one — surfaced on every worklog so a post always carries its rationale.
    """
    def bullets(items: list[str]) -> list[dict]:
        return [{"text": t, "evidence_refs": []} for t in items if t and t.strip()]

    return {
        "task_key": task_key,
        "window_start": window_start,
        "window_end": window_end,
        "cycle_index": cycle_index,
        "time_spent_seconds": time_spent_seconds,
        "summary": draft.summary,
        "what_shipped": bullets(draft.what_shipped),
        "decisions": bullets(draft.decisions),
        "risk_flags": [],
        "confidence": max(0.0, min(1.0, float(draft.confidence))),
        "reasoning": reasoning,
    }


def upsert_worklog(
    conn: sqlite3.Connection,
    *,
    task_key: str,
    day_utc: str,
    cycle_index: int,
    window_start: str,
    window_end: str,
    confidence: float,
    time_spent_seconds: int,
    payload: dict,
    workflow_run_id: str | None = None,
    session_id_min: int | None = None,
    session_id_max: int | None = None,
) -> int | None:
    """UPSERT a drafted worklog. Idempotent on (task_key, day_utc, cycle_index);
    refreshes an existing draft but never overwrites an approved/posted row.
    Returns the row id, or None if an immutable row was preserved.

    ``provider`` is resolved from the matched task's own ``pm_tasks.provider``
    (COALESCE to 'jira' for the legacy single-provider case) so the worklog is
    posted to the RIGHT tracker — Jira / GitHub / Linear / Azure DevOps / Trello.
    Mirrors the Rust ``db::upsert_pm_worklog`` resolution exactly; without it every
    worklog would default to 'jira' (migration 031 default) and a GitHub/Linear/
    Azure/Trello task's worklog would post to Jira.
    """
    cur = conn.execute(
        """
        INSERT INTO pm_worklogs
            (task_key, day_utc, cycle_index, window_start, window_end, state,
             confidence, coverage, time_spent_seconds, payload_json,
             workflow_run_id, session_id_min, session_id_max, provider)
        VALUES (?, ?, ?, ?, ?, 'drafted', ?, 1.0, ?, ?, ?, ?, ?,
                COALESCE((SELECT provider FROM pm_tasks WHERE task_key = ?), 'jira'))
        ON CONFLICT (task_key, day_utc, cycle_index) DO UPDATE SET
            window_start       = excluded.window_start,
            window_end         = excluded.window_end,
            state              = 'drafted',
            confidence         = excluded.confidence,
            time_spent_seconds = excluded.time_spent_seconds,
            payload_json       = excluded.payload_json,
            workflow_run_id    = excluded.workflow_run_id,
            provider           = excluded.provider
        WHERE pm_worklogs.state IN ('drafted', 'skipped', 'failed')
        """,
        (task_key, day_utc, cycle_index, window_start, window_end,
         float(confidence), int(time_spent_seconds), json.dumps(payload),
         workflow_run_id, session_id_min, session_id_max, task_key),
    )
    conn.commit()
    row = conn.execute(
        "SELECT id, state FROM pm_worklogs WHERE task_key=? AND day_utc=? AND cycle_index=?",
        (task_key, day_utc, cycle_index),
    ).fetchone()
    if row is None:
        return None
    if cur.rowcount == 0 and row["state"] not in _OVERWRITABLE:
        # A human-owned row (approved/posted/queued) was preserved — the UPSERT was a
        # no-op. Return None so the caller does not count this run as having drafted it
        # (res.worklog_ids tracks THIS run's output, not pre-existing human-owned rows).
        log.info("worklog: preserved immutable %s row for %s", row["state"], task_key)
        return None
    return int(row["id"])


def retract_drafted_worklogs(
    conn: sqlite3.Connection,
    *,
    day_utc: str,
    cycle_index: int,
    keep_task_keys: list[str],
) -> int:
    """Mark this hour's stale DRAFTED worklogs as 'skipped'.

    Makes an hour's outcome idempotent: a re-run that matches a different task
    set (or flips to propose, keep=[]) must not leave a prior run's drafts behind.
    Only touches machine-owned 'drafted' rows — an approved/posted/queued row a
    human has acted on is never retracted. Returns the count retracted.
    """
    placeholders = ",".join("?" * len(keep_task_keys))
    keep_clause = f"AND task_key NOT IN ({placeholders})" if keep_task_keys else ""
    cur = conn.execute(
        f"""
        UPDATE pm_worklogs SET state = 'skipped'
        WHERE day_utc = ? AND cycle_index = ? AND state = 'drafted' {keep_clause}
        """,
        (day_utc, cycle_index, *keep_task_keys),
    )
    conn.commit()
    if cur.rowcount:
        log.info("worklog: retracted %d stale drafted worklog(s) for %s cyc%d",
                 cur.rowcount, day_utc, cycle_index)
    return cur.rowcount


def retract_proposed_task(
    conn: sqlite3.Connection,
    *,
    day_utc: str,
    source_hour: str,
) -> int:
    """Remove this hour's stale machine-proposed ticket.

    Called when a re-run of the hour produces NO proposal (the matches now cover
    the hour, or the proposer abstained). Only deletes a still-'proposed' row — an
    approved/dismissed proposal a human has acted on is never touched. Returns the
    count removed.
    """
    cur = conn.execute(
        "DELETE FROM pm_proposed_tasks WHERE day_utc = ? AND source_hour = ? "
        "AND state = 'proposed'",
        (day_utc, source_hour),
    )
    conn.commit()
    if cur.rowcount:
        log.info("worklog: retracted stale proposed ticket for %s", source_hour)
    return cur.rowcount


def persist_hour_text(
    conn: sqlite3.Connection,
    *,
    hour_start: str,
    body: str,
    out_chars: int,
    reduction_pct: float,
) -> None:
    """Persist the distilled hour body onto the pm_worklog_hours ledger row.

    Keyed on ``hour_start`` — the UTC ``+00:00`` hour bound the Rust driver's
    ledger uses (``ensure_hour`` inserts this row before the pipeline runs). Runs
    for EVERY distilled hour, even ones that yield no worklog, so the dashboard can
    show the hour's activity independent of ticket matching. Degrades silently on a
    pre-053 DB where the text columns don't yet exist. Logs (rather than silently
    dropping) when the UPDATE matches zero rows — meaning no ``pm_worklog_hours``
    row exists for this ``hour_start`` (``ensure_hour`` wasn't called first, or the
    Rust/Python hour_start formatting has diverged) — since that means the text is
    computed but never actually persisted.
    """
    try:
        cur = conn.execute(
            "UPDATE pm_worklog_hours "
            "SET hour_text = ?, hour_text_chars = ?, hour_text_reduction_pct = ? "
            "WHERE hour_start = ?",
            (body, out_chars, reduction_pct, hour_start),
        )
        conn.commit()
        if cur.rowcount == 0:
            log.warning(
                "persist_hour_text: no pm_worklog_hours row for hour_start — text dropped",
                extra={"hour_start": hour_start},
            )
    except sqlite3.OperationalError:
        log.warning(
            "persist_hour_text: pm_worklog_hours text columns absent (pre-053 DB) — skipped",
            extra={"hour_start": hour_start},
        )


def persist_hour_report(
    conn: sqlite3.Connection,
    *,
    hour_start: str,
    report: str,
) -> None:
    """Persist the /activity_report OUTPUT onto the pm_worklog_hours ledger row.

    Distinct from ``persist_hour_text`` (the raw distilled INPUT) — this is the
    human-readable summary the dashboard's hour-detail panel must show. Runs for
    every hour that reaches stage_report, even ones producing an empty report (no
    activity). Degrades silently on a pre-054 DB where the column doesn't exist.
    Logs when the UPDATE matches zero rows (see ``persist_hour_text``'s docstring
    for why that's worth a log line rather than a silent no-op).
    """
    try:
        cur = conn.execute(
            "UPDATE pm_worklog_hours "
            "SET hour_report = ?, hour_report_chars = ? "
            "WHERE hour_start = ?",
            (report, len(report), hour_start),
        )
        conn.commit()
        if cur.rowcount == 0:
            log.warning(
                "persist_hour_report: no pm_worklog_hours row for hour_start — report dropped",
                extra={"hour_start": hour_start},
            )
    except sqlite3.OperationalError:
        log.warning(
            "persist_hour_report: pm_worklog_hours.hour_report column absent (pre-054 DB) — skipped",
            extra={"hour_start": hour_start},
        )


def upsert_proposed_task(
    conn: sqlite3.Connection,
    *,
    day_utc: str,
    source_hour: str,
    title: str,
    description: str,
    reasoning: str = "",
    issue_type: str = "Task",
    workflow_run_id: str | None = None,
    worklog_payload: dict | None = None,
    time_spent_seconds: int = 3600,
    confidence: float = 0.0,
    window_start: str | None = None,
    window_end: str | None = None,
) -> int | None:
    """UPSERT a tier-3 proposed task together with its DRAFTED worklog.

    Idempotent on (day_utc, source_hour); refreshes a still-proposed row, leaves
    approved/dismissed untouched. Returns the row id on insert/refresh, or ``None``
    when a DECIDED (approved/dismissed) row already owns the key and the guarded
    update was a no-op — so the caller never mistakes a stale decided id for the
    freshly-persisted proposal. ``worklog_payload`` is the JiraUpdate-shaped
    draft (see :func:`build_payload`) the approval surface shows + posts; it is
    stored as JSON in ``worklog_payload_json`` (migration 050). ``issue_type`` is
    'Task' or 'Bug' (migration 051) and selects the issue type at creation time.
    """
    payload_json = json.dumps(worklog_payload) if worklog_payload is not None else None
    conn.execute(
        """
        INSERT INTO pm_proposed_tasks
            (day_utc, source_hour, title, description, reasoning, issue_type, state,
             workflow_run_id, worklog_payload_json, time_spent_seconds,
             confidence, window_start, window_end)
        VALUES (?, ?, ?, ?, ?, ?, 'proposed', ?, ?, ?, ?, ?, ?)
        ON CONFLICT (day_utc, source_hour) DO UPDATE SET
            title                = excluded.title,
            description          = excluded.description,
            reasoning            = excluded.reasoning,
            issue_type           = excluded.issue_type,
            workflow_run_id      = excluded.workflow_run_id,
            worklog_payload_json = excluded.worklog_payload_json,
            time_spent_seconds   = excluded.time_spent_seconds,
            confidence           = excluded.confidence,
            window_start         = excluded.window_start,
            window_end           = excluded.window_end
        WHERE pm_proposed_tasks.state = 'proposed'
        """,
        (day_utc, source_hour, title, description, reasoning, issue_type,
         workflow_run_id, payload_json, time_spent_seconds, confidence,
         window_start, window_end),
    )
    # changes()==0 means the row for (day_utc, source_hour) already exists in an
    # APPROVED/DISMISSED state, so the `WHERE state='proposed'` guard blocked the
    # DO UPDATE (a no-op). Return None rather than the SELECT below, which would
    # hand back the DECIDED row's id and make the caller believe the new proposal
    # was persisted (it was intentionally NOT — a user's decision is immutable).
    changed = conn.execute("SELECT changes()").fetchone()[0]
    conn.commit()
    if not changed:
        return None
    row = conn.execute(
        "SELECT id FROM pm_proposed_tasks WHERE day_utc=? AND source_hour=?",
        (day_utc, source_hour),
    ).fetchone()
    return int(row["id"]) if row else None
