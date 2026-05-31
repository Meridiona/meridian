"""CLI entry point for the pm_worklog_update workflow.

Run a single PM-update cycle for one ticket from the command line:

    cd services
    .venv/bin/python -m agents.pm_worklog_update.cli \\
        --task KAN-64 \\
        --window-start 2026-05-28T09:00:00Z \\
        --window-end   2026-05-28T12:00:00Z

Defaults:

    --window-end   = now (UTC)
    --window-start = (now - PM_WORKLOG_INTERVAL_HOURS)
    --cycle-index  = 0
    --dry-run      = true (no Jira post)

This is the only blessed way to exercise the workflow until it's
stitched into the daemon. Returns exit code 0 on success, 1 on
internal failure, 2 on "skipped" (e.g. window too quiet).
"""
from __future__ import annotations

import argparse
import json
import logging
import sys
from datetime import datetime, timedelta, timezone

from agents import observability
from agents.pm_worklog_update import config
from agents.pm_worklog_update.models import UpdateState

log = logging.getLogger(__name__)


def _parse_iso(s: str) -> datetime:
    """Parse an ISO-8601 string, defaulting to UTC if naive."""
    cleaned = s.replace("Z", "+00:00")
    dt = datetime.fromisoformat(cleaned)
    if dt.tzinfo is None:
        dt = dt.replace(tzinfo=timezone.utc)
    return dt.astimezone(timezone.utc)


def build_parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(
        prog="agents.pm_worklog_update.cli",
        description="Run one PM-update cycle for one Jira ticket against meridian.db",
    )
    p.add_argument("--task", required=True, help="Jira ticket key, e.g. KAN-64")
    p.add_argument(
        "--window-start",
        type=_parse_iso,
        default=None,
        help="ISO-8601 UTC start of the window. Default: now - PM_WORKLOG_INTERVAL_HOURS",
    )
    p.add_argument(
        "--window-end",
        type=_parse_iso,
        default=None,
        help="ISO-8601 UTC end of the window. Default: now",
    )
    p.add_argument(
        "--cycle-index",
        type=int,
        default=0,
        help="Nth cycle of the day for this ticket (0-indexed). Default: 0",
    )
    p.add_argument(
        "--post",
        action="store_true",
        help="Actually post the worklog to Jira (default: dry-run, only writes to "
             "pm_updates table). Skips automatically if real_seconds < 60.",
    )
    p.add_argument(
        "--json",
        dest="emit_json",
        action="store_true",
        help="Emit only the RouteOutcome JSON on stdout (suppresses human-readable summary).",
    )
    p.add_argument(
        "--debug",
        action="store_true",
        help="Turn on agno debug_mode for the workflow + all agents (shows prompts, "
             "tool calls, model responses). Equivalent to AGNO_DEBUG=True.",
    )
    p.add_argument(
        "--debug-verbose",
        action="store_true",
        help="As --debug, plus debug_level=2 (much more detail; spammy).",
    )
    return p


def _resolve_window(args: argparse.Namespace) -> tuple[datetime, datetime]:
    end = args.window_end or datetime.now(timezone.utc)
    start = args.window_start or (
        end - timedelta(hours=config.PM_WORKLOG_INTERVAL_HOURS)
    )
    if start >= end:
        raise SystemExit(f"window-start ({start}) must be earlier than window-end ({end})")
    return start, end


def main(argv: list[str] | None = None) -> int:
    # Late import so `--help` works even when agno isn't installed.
    from agents.pm_worklog_update.workflow import run_cycle

    observability.setup("meridian-pm-update")
    args = build_parser().parse_args(argv)
    start, end = _resolve_window(args)

    log.info(
        "pm_update CLI: task=%s window=%s→%s cycle=%d post=%s",
        args.task, start.isoformat(), end.isoformat(), args.cycle_index, args.post,
    )

    debug_mode  = args.debug or args.debug_verbose
    debug_level = 2 if args.debug_verbose else 1

    try:
        outcome = run_cycle(
            task_key=args.task,
            window_start=start,
            window_end=end,
            cycle_index=args.cycle_index,
            dry_run=not args.post,
            debug_mode=debug_mode,
            debug_level=debug_level,
        )
    except Exception as exc:                            # noqa: BLE001 — CLI top level
        log.exception("pm_update cycle failed: %s", exc)
        observability.shutdown()
        return 1

    if args.emit_json:
        sys.stdout.write(outcome.model_dump_json() + "\n")
    else:
        sys.stdout.write(
            f"\n=== PM update outcome ===\n"
            f"  state:        {outcome.state.value}\n"
            f"  pm_worklog_id: {outcome.pm_worklog_id}\n"
            f"  reason:       {outcome.reason}\n"
        )

    observability.shutdown()
    if outcome.state == UpdateState.FAILED:
        return 1
    if outcome.state == UpdateState.SKIPPED:
        return 2
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
