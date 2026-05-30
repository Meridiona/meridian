# meridian — normalises screenpipe activity into structured app sessions
"""Daemon that auto-posts Jira worklogs for classified sessions on a repeating interval.

Runs one full cycle across all active tickets on startup (catch-up), then
sleeps PM_WORKLOG_INTERVAL_HOURS and repeats indefinitely.

Usage:
    python -m agents.pm_worklog_update.jira_worklog_daemon          # run daemon
    python -m agents.pm_worklog_update.jira_worklog_daemon --once   # one cycle then exit
    python -m agents.pm_worklog_update.jira_worklog_daemon --task KAN-X
"""
from __future__ import annotations

import argparse
import logging
import os
import signal
import sqlite3
import sys
import time
import traceback
from datetime import datetime, timedelta, timezone
from pathlib import Path

from dotenv import load_dotenv

# Load credentials from repo root .env then services/.env (same order as jira_poster).
_SERVICES_ROOT = Path(__file__).parents[3]
for _env in [_SERVICES_ROOT / ".env", _SERVICES_ROOT / "services" / ".env"]:
    if _env.exists():
        load_dotenv(_env, override=False)

from agents import observability
from agents.pm_worklog_update import config
from agents.pm_worklog_update.db import has_recent_classified_work, last_posted_window_end
from agents.pm_worklog_update.models import UpdateState
from agents.pm_worklog_update.workflow import run_cycle

log = logging.getLogger(__name__)

_AGENT_NAME = "pm-worklog-daemon"
_DAEMON_VERSION = "1.0.0"

# How far back to look for sessions when no worklog has ever been posted.

# Maximum window length for a single run_cycle call; longer windows are split.
_MAX_WINDOW_HOURS = 1.0   # each worklog covers exactly 1 hour of work

# Minimum window length (seconds) worth submitting.
_MIN_WINDOW_SECONDS = 60

# Shutdown flag set by signal handlers.
_shutdown: bool = False


# ──────────────────────── Signal handling ─────────────────────────────────────

def _install_signal_handlers() -> None:
    def _handler(signum, frame):  # noqa: ARG001
        global _shutdown
        log.info("shutdown signal received", extra={"signal": signum})
        _shutdown = True

    signal.signal(signal.SIGTERM, _handler)
    signal.signal(signal.SIGINT, _handler)


# ──────────────────────── Active ticket discovery ─────────────────────────────

def _fetch_active_task_keys(lookback_cutoff: datetime) -> list[str]:
    """Return distinct task_keys that have recent classified task sessions."""
    cutoff_iso = lookback_cutoff.astimezone(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
    try:
        conn = sqlite3.connect(
            f"file:{config.MERIDIAN_DB}?mode=ro", uri=True, timeout=10
        )
        conn.row_factory = sqlite3.Row
        rows = conn.execute(
            """
            SELECT DISTINCT task_key FROM app_sessions
            WHERE task_session_type = 'task'
              AND task_key IS NOT NULL
              AND ended_at > ?
            """,
            (cutoff_iso,),
        ).fetchall()
        conn.close()
        return [r["task_key"] for r in rows]
    except Exception:
        log.exception("failed to fetch active task keys")
        return []


# ──────────────────────── Cycle index helper ──────────────────────────────────

def _count_posted_today(task_key: str, day_utc: str) -> int:
    """Count POSTED worklogs for this ticket today to determine the next cycle_index."""
    try:
        conn = sqlite3.connect(
            f"file:{config.MERIDIAN_DB}?mode=ro", uri=True, timeout=10
        )
        conn.row_factory = sqlite3.Row
        row = conn.execute(
            """
            SELECT COUNT(*) AS n FROM pm_worklogs
            WHERE task_key = ? AND day_utc = ? AND state = ?
            """,
            (task_key, day_utc, UpdateState.POSTED.value),
        ).fetchone()
        conn.close()
        return row["n"] if row else 0
    except Exception:
        log.exception("failed to count posted worklogs", extra={"task_key": task_key})
        return 0


# ──────────────────────── Single-ticket processing ────────────────────────────

def _process_ticket(task_key: str, now: datetime) -> None:
    """Run one or more cycle calls for a single ticket, covering the full window."""
    last_end = last_posted_window_end(task_key)
    if last_end is None:
        # First run: start from midnight today (local time) so the daemon
        # catches up on all hours worked today, one hour at a time.
        local_now = now.astimezone()
        window_start = local_now.replace(hour=0, minute=0, second=0, microsecond=0).astimezone(timezone.utc)
    else:
        window_start = last_end

    window_end = now

    span_seconds = (window_end - window_start).total_seconds()
    if span_seconds < _MIN_WINDOW_SECONDS:
        log.info(
            "skipping ticket — window too short",
            extra={"task_key": task_key, "span_seconds": span_seconds},
        )
        return

    if not has_recent_classified_work(task_key, since=window_start):
        log.info(
            "skipping ticket — no classified work in window",
            extra={"task_key": task_key, "window_start": window_start.isoformat()},
        )
        return

    # Split windows longer than _MAX_WINDOW_HOURS into daily sub-windows.
    sub_windows = _split_into_hourly_windows(window_start, window_end)

    for sub_start, sub_end in sub_windows:
        if _shutdown:
            log.info("shutdown requested — stopping mid-ticket", extra={"task_key": task_key})
            break

        sub_span = (sub_end - sub_start).total_seconds()
        if sub_span < _MIN_WINDOW_SECONDS:
            continue

        day_utc = sub_start.astimezone(timezone.utc).strftime("%Y-%m-%d")
        cycle_index = _count_posted_today(task_key, day_utc)

        log.info(
            "running cycle",
            extra={
                "task_key": task_key,
                "window_start": sub_start.isoformat(),
                "window_end": sub_end.isoformat(),
                "cycle_index": cycle_index,
            },
        )

        try:
            outcome = run_cycle(
                task_key=task_key,
                window_start=sub_start,
                window_end=sub_end,
                cycle_index=cycle_index,
                dry_run=False,
            )
            log.info(
                "cycle complete",
                extra={
                    "task_key": task_key,
                    "state": outcome.state.value,
                    "reason": outcome.reason,
                    "pm_worklog_id": outcome.pm_worklog_id,
                    "posted_comment_id": outcome.posted_comment_id,
                },
            )
        except Exception:
            log.error(
                "cycle failed",
                extra={
                    "task_key": task_key,
                    "window_start": sub_start.isoformat(),
                    "window_end": sub_end.isoformat(),
                    "traceback": traceback.format_exc(),
                },
            )
            # One sub-window failure does not abort remaining sub-windows.


def _split_into_hourly_windows(
    start: datetime, end: datetime
) -> list[tuple[datetime, datetime]]:
    """Split a time range into 1-hour sub-windows (catch-up from start of day)."""
    max_delta = timedelta(hours=_MAX_WINDOW_HOURS)
    windows: list[tuple[datetime, datetime]] = []
    cursor = start
    while cursor < end:
        chunk_end = min(cursor + max_delta, end)
        windows.append((cursor, chunk_end))
        cursor = chunk_end
    return windows


# ──────────────────────── Full cycle across all tickets ───────────────────────

def _run_full_cycle(task_key_filter: str | None = None) -> None:
    """Run one pass across all active tickets (or a single filtered ticket)."""
    now = datetime.now(tz=timezone.utc)
    lookback_cutoff = now - timedelta(hours=_DEFAULT_LOOKBACK_HOURS)

    if task_key_filter:
        task_keys = [task_key_filter]
    else:
        task_keys = _fetch_active_task_keys(lookback_cutoff)

    log.info(
        "cycle start",
        extra={"ticket_count": len(task_keys), "task_keys": task_keys},
    )

    for task_key in task_keys:
        if _shutdown:
            log.info("shutdown requested — aborting cycle")
            break
        try:
            _process_ticket(task_key, now)
        except Exception:
            log.error(
                "unhandled error processing ticket",
                extra={"task_key": task_key, "traceback": traceback.format_exc()},
            )

    log.info("cycle end", extra={"ticket_count": len(task_keys)})


# ──────────────────────── Daemon loop ─────────────────────────────────────────

def _run_daemon(task_key_filter: str | None = None) -> None:
    interval_seconds = config.PM_WORKLOG_INTERVAL_HOURS * 3600

    log.info(
        "daemon starting",
        extra={
            "version": _DAEMON_VERSION,
            "python": sys.version,
            "interval_hours": config.PM_WORKLOG_INTERVAL_HOURS,
            "lookback_hours": _DEFAULT_LOOKBACK_HOURS,
            "min_confidence": config.PM_WORKLOG_MIN_CONFIDENCE,
            "meridian_db": str(config.MERIDIAN_DB),
        },
    )

    while not _shutdown:
        _run_full_cycle(task_key_filter)

        if _shutdown:
            break

        log.info(
            "sleeping until next cycle",
            extra={"interval_seconds": interval_seconds},
        )

        # Sleep in short increments so SIGTERM is handled promptly.
        slept = 0.0
        while slept < interval_seconds and not _shutdown:
            chunk = min(5.0, interval_seconds - slept)
            time.sleep(chunk)
            slept += chunk

    log.info("daemon exiting cleanly")
    observability.shutdown()


# ──────────────────────── Entry point ─────────────────────────────────────────

def main() -> None:
    parser = argparse.ArgumentParser(
        description="Jira worklog daemon — auto-posts worklogs for classified sessions"
    )
    parser.add_argument(
        "--once",
        action="store_true",
        help="Run one cycle then exit (no daemon loop)",
    )
    parser.add_argument(
        "--task",
        metavar="TASK_KEY",
        default=None,
        help="Process a single ticket only (e.g. KAN-42)",
    )
    args = parser.parse_args()

    observability.setup(_AGENT_NAME)
    _install_signal_handlers()

    if args.once:
        log.info("one-shot mode", extra={"task_key": args.task})
        _run_full_cycle(args.task)
        observability.shutdown()
    else:
        _run_daemon(args.task)


if __name__ == "__main__":
    main()
