"""backfill_tasks — re-run hermes task classification for a specific session range.

Writes are idempotent (ON CONFLICT DO UPDATE). Does not touch agent_cursor.
Safe to re-run; existing links are overwritten with fresh results.

Usage:
  python3 -m agents.backfill_tasks --today
  python3 -m agents.backfill_tasks --yesterday
  python3 -m agents.backfill_tasks --from-date 2025-05-01 --to-date 2025-05-14
  python3 -m agents.backfill_tasks --from-id 100 --to-id 500
  python3 -m agents.backfill_tasks --from-id 100           # 100 onwards
  python3 -m agents.backfill_tasks --dry-run --today       # print without writing
"""
from __future__ import annotations

import argparse
import json
import logging
import sqlite3
import sys
from datetime import date, datetime, timedelta, timezone

from agents import observability
from agents.config import MERIDIAN_DB, MIN_LLM_DURATION_S
from agents.db import write_ticket_link, upsert_session_dimension
from agents.run_task_linker import _classify_one

observability.setup("meridian-backfill-tasks")
log = logging.getLogger("agents.backfill_tasks")


# ── Range resolution ───────────────────────────────────────────────────────────

def _date_utc_range(d: date) -> tuple[str, str]:
    """Return (from_iso, to_iso) covering a full local calendar day in UTC."""
    from_dt = datetime(d.year, d.month, d.day, tzinfo=timezone.utc)
    to_dt = from_dt + timedelta(days=1)
    return from_dt.strftime("%Y-%m-%dT%H:%M:%SZ"), to_dt.strftime("%Y-%m-%dT%H:%M:%SZ")


def _build_where(args: argparse.Namespace) -> tuple[str, list]:
    conditions: list[str] = ["duration_s >= ?"]
    binds: list = [MIN_LLM_DURATION_S]

    if args.today:
        frm, to = _date_utc_range(date.today())
        conditions.append("started_at >= ? AND started_at < ?")
        binds += [frm, to]
    elif args.yesterday:
        frm, to = _date_utc_range(date.today() - timedelta(days=1))
        conditions.append("started_at >= ? AND started_at < ?")
        binds += [frm, to]
    else:
        if args.from_date:
            frm, _ = _date_utc_range(datetime.strptime(args.from_date, "%Y-%m-%d").date())
            conditions.append("started_at >= ?")
            binds.append(frm)
        if args.to_date:
            _, to = _date_utc_range(datetime.strptime(args.to_date, "%Y-%m-%d").date())
            conditions.append("started_at < ?")
            binds.append(to)
        if args.from_id is not None:
            conditions.append("id >= ?")
            binds.append(args.from_id)
        if args.to_id is not None:
            conditions.append("id <= ?")
            binds.append(args.to_id)

    return " AND ".join(conditions), binds


# ── DB helpers ─────────────────────────────────────────────────────────────────

def _fetch_sessions(conn: sqlite3.Connection, where: str, binds: list) -> list[dict]:
    rows = conn.execute(
        f"""
        SELECT id, app_name, duration_s, session_text, window_titles,
               category, confidence, audio_snippets
        FROM app_sessions
        WHERE {where}
        ORDER BY id ASC
        """,
        binds,
    ).fetchall()
    result = []
    for r in rows:
        titles = r[4] or "[]"
        try:
            titles = json.loads(titles)
        except (json.JSONDecodeError, TypeError):
            titles = []
        audio = r[7] or "[]"
        try:
            audio = json.loads(audio)
        except (json.JSONDecodeError, TypeError):
            audio = []
        result.append({
            "id":             r[0],
            "app_name":       r[1],
            "duration_s":     r[2],
            "session_text":   r[3] or "",
            "window_titles":  titles,
            "category":       r[5],
            "confidence":     r[6],
            "audio_snippets": audio,
        })
    return result


def _fetch_pm_tasks(conn: sqlite3.Connection) -> list[dict]:
    rows = conn.execute(
        """
        SELECT task_key, title, description_text, status, status_category
        FROM pm_tasks
        WHERE status_category != 'done'
        ORDER BY updated_at DESC
        """,
    ).fetchall()
    return [
        {
            "task_key":         r[0],
            "title":            r[1] or "",
            "description_text": r[2] or "",
            "status":           r[3] or "",
            "status_category":  r[4] or "",
        }
        for r in rows
    ]


# ── Main ───────────────────────────────────────────────────────────────────────

def main() -> None:
    parser = argparse.ArgumentParser(
        description="Re-run hermes task classification for a session range."
    )
    when = parser.add_mutually_exclusive_group()
    when.add_argument("--today",     action="store_true", help="Sessions from today")
    when.add_argument("--yesterday", action="store_true", help="Sessions from yesterday")
    parser.add_argument("--from-date", metavar="YYYY-MM-DD",
                        help="Sessions starting on or after this date")
    parser.add_argument("--to-date",   metavar="YYYY-MM-DD",
                        help="Sessions starting on or before this date")
    parser.add_argument("--from-id",   type=int, metavar="N",
                        help="Sessions with id >= N")
    parser.add_argument("--to-id",     type=int, metavar="N",
                        help="Sessions with id <= N")
    parser.add_argument("--dry-run",   action="store_true",
                        help="Print results without writing to DB")
    args = parser.parse_args()

    has_range = any([
        args.today, args.yesterday,
        args.from_date, args.to_date,
        args.from_id is not None, args.to_id is not None,
    ])
    if not has_range:
        parser.error(
            "specify a range: --today, --yesterday, "
            "--from-date/--to-date, or --from-id/--to-id"
        )

    conn = sqlite3.connect(str(MERIDIAN_DB))
    conn.execute("PRAGMA journal_mode=WAL")
    conn.execute("PRAGMA foreign_keys=ON")

    try:
        where, binds = _build_where(args)
        sessions = _fetch_sessions(conn, where, binds)
        pm_tasks = _fetch_pm_tasks(conn)
        pm_lookup = {t["task_key"]: t for t in pm_tasks}

        print(f"Sessions to process: {len(sessions)}  PM tasks: {len(pm_tasks)}  "
              f"dry_run={args.dry_run}")

        if not sessions:
            print("No sessions found for the specified range.")
            return

        skipped = classified = errors = 0

        for s in sessions:
            result = _classify_one(s, pm_lookup, pm_tasks)

            if args.dry_run:
                task = result["task_key"] or "—"
                print(
                    f"  session {result['session_id']:6d}  "
                    f"task={task:12s}  "
                    f"routing={result['routing']:5s}  "
                    f"conf={result['confidence']:.2f}  "
                    f"method={result['method']}"
                )
                classified += 1
                continue

            if result["method"] == "llm_error":
                log.warning("session %d: llm_error — %s", result["session_id"], result["reasoning"])
                errors += 1
                continue

            write_ticket_link(
                conn,
                session_id=result["session_id"],
                task_key=result["task_key"],
                confidence=result["confidence"],
                session_type="work" if result["task_key"] else "overhead",
                routing=result["routing"],
                method=result["method"],
            )
            for dim, vals in (result.get("dimensions") or {}).items():
                for val in vals:
                    upsert_session_dimension(
                        conn,
                        session_id=result["session_id"],
                        dimension=dim,
                        value=val,
                        confidence=result["confidence"],
                        source="llm_backfill",
                    )
            classified += 1
            log.info(
                "session %d → task=%s routing=%s elapsed=%.1fs",
                result["session_id"], result["task_key"],
                result["routing"], result["elapsed_s"],
            )

        print(
            f"Done. classified={classified}  skipped={skipped}  errors={errors}"
            + ("  (dry run — nothing written)" if args.dry_run else "")
        )

    finally:
        conn.close()


if __name__ == "__main__":
    try:
        main()
    except Exception:
        log.exception("backfill_tasks failed")
        sys.exit(1)
