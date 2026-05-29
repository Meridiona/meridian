"""Debug CLI for coding_agent_indexer.

Four modes:

    # Register one specific JSONL by full path
    python -m coding_agent_indexer.cli --jsonl ~/.claude/projects/.../<uuid>.jsonl

    # Register one specific session by uuid (we'll search for it)
    python -m coding_agent_indexer.cli --session-uuid <uuid>

    # Do one poll sweep, exit (same logic as the daemon's tick, no loop)
    python -m coding_agent_indexer.cli --scan-once

    # Wipe all indexer-owned rows and re-register from disk under the
    # current schema (used once after migration 026 to split legacy
    # per-(uuid, started_at) rows into per-(uuid, day_utc) rows)
    python -m coding_agent_indexer.cli --reseed

All modes except --reseed are idempotent against the DB unique
constraint, so running them twice has no extra effect.
"""
from __future__ import annotations

import argparse
import json
import logging
import sys
import time
from datetime import datetime, timezone
from pathlib import Path
from typing import Optional

from agents import observability
from coding_agent_indexer import config, db, register
from coding_agent_indexer.daemon import _candidate_jsonls, _load_fork_skip

log = logging.getLogger(__name__)


def build_parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(
        prog="coding_agent_indexer.cli",
        description="Register ended Claude / Codex sessions as app_sessions rows (one-shot).",
    )
    g = p.add_mutually_exclusive_group(required=True)
    g.add_argument("--jsonl", type=Path, help="Path to a specific JSONL to register.")
    g.add_argument("--session-uuid", help="Session uuid (we'll search Claude project dirs for it).")
    g.add_argument("--scan-once", action="store_true",
                   help="Do one daemon-style poll sweep, register everything ready, exit.")
    g.add_argument("--reseed", action="store_true",
                   help="Wipe all indexer-owned app_sessions rows then re-scan from disk. "
                        "Use after migration 027 to re-split legacy per-day rows into "
                        "per-segment (idle-gap) rows. Destructive (rows are recreated).")
    p.add_argument("--json", dest="emit_json", action="store_true",
                   help="Emit machine-readable JSON for each result.")
    return p


def main(argv: Optional[list[str]] = None) -> int:
    observability.setup("meridian-coding-agent-indexer-cli")
    args = build_parser().parse_args(argv)

    if args.jsonl:
        rc = _do_one(args.jsonl, emit_json=args.emit_json)
    elif args.session_uuid:
        path = _find_jsonl_by_uuid(args.session_uuid)
        if path is None:
            print(f"could not find a JSONL matching uuid {args.session_uuid!r}", file=sys.stderr)
            observability.shutdown()
            return 2
        rc = _do_one(path, emit_json=args.emit_json)
    elif args.reseed:
        rc = _do_reseed(emit_json=args.emit_json)
    else:
        rc = _do_scan(emit_json=args.emit_json)

    observability.shutdown()
    return rc


# ──────────────────────── Modes ────────────────────────────────────────────────


def _do_one(jsonl_path: Path, *, emit_json: bool) -> int:
    if not jsonl_path.exists():
        print(f"file not found: {jsonl_path}", file=sys.stderr)
        return 2
    # Poll-like: don't force-seal a possibly-active session. register() still
    # seals the last segment if it has already idled out (>1h since last msg).
    result = register.register_ended_session(jsonl_path, session_ended=False)
    _print_result(result, jsonl_path, emit_json=emit_json)
    return 0 if result.outcome != register.RegisterOutcome.FAILED else 1


def _do_reseed(*, emit_json: bool) -> int:
    """Wipe all indexer-owned rows, then re-scan from disk.

    Destructive: rows are recreated. Use once after migration 026 to
    split legacy per-(uuid, started_at) rows into per-(uuid, day_utc).
    """
    deleted = db.delete_claude_session_rows()
    log.info("reseed: deleted %d existing rows; re-scanning…", deleted)
    if not emit_json:
        print(f"reseed: deleted {deleted} existing indexer-owned rows; re-scanning from disk…")
    return _do_scan(emit_json=emit_json)


def _do_scan(*, emit_json: bool) -> int:
    """One poll sweep, identical to the daemon's tick: seal settled live
    rows, then register changed files (poll path → session_ended=False)."""
    fork_skip = _load_fork_skip()
    now_dt = datetime.now().astimezone()
    now_iso = now_dt.isoformat(timespec='milliseconds')

    sealed = db.seal_stale_open_rows(now_iso=now_iso, idle_seconds=config.SEAL_IDLE_SECONDS)
    if sealed and not emit_json:
        print(f"sealed {sealed} settled open row(s)")

    wrote = skipped = failed = 0
    for jsonl in _candidate_jsonls(now=time.time()):
        result = register.register_ended_session(
            jsonl, session_ended=False, fork_skip_list=fork_skip, now=now_dt,
        )
        _print_result(result, jsonl, emit_json=emit_json)
        if result.outcome == register.RegisterOutcome.INSERTED:
            wrote += 1
        elif result.outcome == register.RegisterOutcome.FAILED:
            failed += 1
        else:
            skipped += 1
    if not emit_json:
        print(f"\n=== scan complete: sealed={sealed} wrote={wrote} skipped={skipped} failed={failed} ===")
    return 0 if failed == 0 else 1


# ──────────────────────── Helpers ──────────────────────────────────────────────


def _find_jsonl_by_uuid(uuid: str) -> Optional[Path]:
    """Search Claude projects + Codex sessions dirs for a JSONL whose stem is uuid."""
    if config.CLAUDE_PROJECTS_DIR.exists():
        for project in config.CLAUDE_PROJECTS_DIR.iterdir():
            if project.is_dir():
                hit = project / f"{uuid}.jsonl"
                if hit.exists():
                    return hit
    if config.CODEX_SESSIONS_DIR.exists():
        for year in config.CODEX_SESSIONS_DIR.iterdir():
            if year.is_dir():
                for month in year.iterdir():
                    if month.is_dir():
                        for day in month.iterdir():
                            if day.is_dir():
                                for candidate in day.glob(f"*{uuid}*.jsonl"):
                                    return candidate
    return None


def _print_result(result, jsonl_path: Path, *, emit_json: bool) -> None:
    if emit_json:
        payload = {
            "outcome":      result.outcome.value,
            "session_uuid": result.session_uuid,
            "jsonl_path":   str(jsonl_path),
            "row_ids":      list(result.row_ids),
            "sealed_ids":   list(result.sealed_ids),
            "host_app":     result.host_app,
            "user_turns":   result.meta.user_turns      if result.meta else None,
            "asst_turns":   result.meta.assistant_turns if result.meta else None,
            "active_s":     result.meta.active_seconds  if result.meta else None,
            "started_at":   result.meta.started_at      if result.meta else None,
            "ended_at":     result.meta.ended_at        if result.meta else None,
            "error":        result.error,
        }
        sys.stdout.write(json.dumps(payload) + "\n")
        return

    icon = {
        "inserted":      "+",
        "skipped_empty": "/",
        "skipped_fork":  "f",
        "failed":        "!",
    }.get(result.outcome.value, "?")
    rows = ",".join(str(r) for r in result.row_ids) if result.row_ids else "-"
    print(
        f"{icon} {result.outcome.value:<14} {result.session_uuid}  "
        f"host={result.host_app or '-':<10} "
        f"rows={rows}  {jsonl_path.name}"
    )
    if result.error:
        print(f"    error: {result.error}")


if __name__ == "__main__":
    raise SystemExit(main())
