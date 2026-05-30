"""Debug / one-shot CLI for the summariser.

    # Drain one batch (same as a daemon tick), write to DB
    python -m coding_agent_summariser.cli --once

    # Summarise one session's pending segments
    python -m coding_agent_summariser.cli --session <uuid>

    # Eyeball output without writing (any mode)
    python -m coding_agent_summariser.cli --session <uuid> --dry-run
    python -m coding_agent_summariser.cli --row <id> --dry-run

All modes are idempotent against the DB (write is `WHERE session_summary IS
NULL`); `--dry-run` never writes.
"""
from __future__ import annotations

import argparse
import logging
import sys
from typing import List, Optional

from agents import observability
from coding_agent_summariser import config, db, summariser
from coding_agent_summariser.db import PendingRow

log = logging.getLogger(__name__)


def build_parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(
        prog="coding_agent_summariser.cli",
        description="Generate session_summary for sealed coding-agent segments.",
    )
    g = p.add_mutually_exclusive_group(required=True)
    g.add_argument("--once", action="store_true",
                   help="Summarise one batch for a single day (default today), like a daemon tick.")
    g.add_argument("--session", metavar="UUID", help="Summarise this session's pending segments.")
    g.add_argument("--row", type=int, metavar="ID", help="Summarise one app_sessions row by id.")
    p.add_argument("--day", metavar="YYYY-MM-DD",
                   help="Day to backfill with --once (default: today). Backfills only that day.")
    p.add_argument("--limit", type=int, default=config.BATCH_PER_TICK,
                   help=f"Max rows to process (default {config.BATCH_PER_TICK}).")
    p.add_argument("--dry-run", action="store_true",
                   help="Generate but do not write; print the summary.")
    return p


def main(argv: Optional[list[str]] = None) -> int:
    observability.setup("meridian-coding-agent-summariser-cli")
    db.ensure_schema()
    args = build_parser().parse_args(argv)

    if args.row is not None:
        row = db.fetch_by_id(args.row)
        rows: List[PendingRow] = [row] if row else []
        if not rows:
            print(f"no coding-agent row with id {args.row}", file=sys.stderr)
            observability.shutdown()
            return 2
    elif args.session:
        rows = db.fetch_pending(args.limit, session_uuid=args.session)
    else:  # --once — scoped to a single day (today unless --day given)
        day = args.day or config.today_local()
        print(f"backfilling day {day}")
        rows = db.fetch_pending(args.limit, day=day)

    if not rows:
        print("nothing to summarise (queue empty for this selection)")
        observability.shutdown()
        return 0

    wrote = failed = 0
    for row in rows:
        outcome = summariser.summarise_one(row, write=not args.dry_run)
        _print(row, outcome, dry_run=args.dry_run)
        if outcome.written:
            wrote += 1
        elif outcome.error:
            failed += 1
        if outcome.rate_limited and not outcome.written:
            print("  (rate-limited — stopping)", file=sys.stderr)
            break

    if not args.dry_run:
        print(f"\n=== done: wrote={wrote} failed={failed} of {len(rows)} ===")
    observability.shutdown()
    return 1 if failed and not wrote else 0


def _print(row: PendingRow, outcome, *, dry_run: bool) -> None:
    head = (f"row {row.id} · {row.session_uuid[:8]} · seg {row.segment_started_at} "
            f"· {row.duration_s}s · {row.text_bytes}B")
    if outcome.error:
        print(f"✗ {head}\n  error: {outcome.error}")
        return
    tag = "[dry-run]" if dry_run else ("[wrote]" if outcome.written else "[noop]")
    print(f"✓ {tag} {head}  via {outcome.source.value}")
    if dry_run and outcome.summary:
        print("  " + outcome.summary.replace("\n", "\n  "))


if __name__ == "__main__":
    raise SystemExit(main())
