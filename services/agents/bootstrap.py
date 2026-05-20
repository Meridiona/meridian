#!/usr/bin/env python3
"""Bootstrap — sanity-check meridian.db is reachable and the agent tables exist.

The Rust daemon owns all DDL via sqlx migrations; this script just verifies the
required tables are present so the synthesizer doesn't blow up at runtime.
"""
from __future__ import annotations

import sys
from pathlib import Path

from agents._hermes_setup import ensure_hermes_importable
ensure_hermes_importable()
from agents.config import LOG_DIR, MERIDIAN_DB, MERIDIAN_HOME
from agents import db


REQUIRED_TABLES = (
    "app_sessions",
    "active_session",
    "pm_tasks",            # populated by the Rust jira provider (003_intelligence)
    # ticket_links merged into app_sessions (018_merge_ticket_links)
    "agent_runs",
    "agent_cursor",
    "session_summaries",
    "dispatch_queue",
    "context_graph_nodes",
    "activity_context",
)


def _exists(conn, table: str) -> bool:
    row = conn.execute(
        "SELECT name FROM sqlite_master WHERE type='table' AND name=?",
        (table,),
    ).fetchone()
    return row is not None


def main() -> int:
    print("=== meridian-agents bootstrap ===\n")

    print(f"MERIDIAN_HOME = {MERIDIAN_HOME}")
    print(f"MERIDIAN_DB   = {MERIDIAN_DB}")
    LOG_DIR.mkdir(parents=True, exist_ok=True)
    print(f"LOG_DIR       = {LOG_DIR} ✓")

    if not Path(MERIDIAN_DB).expanduser().exists():
        print(f"\n✗ {MERIDIAN_DB} not found.")
        print("  Start the Rust daemon (`cargo run --release`) at least once to create the DB.")
        return 1

    missing = []
    with db.connection() as conn:
        for t in REQUIRED_TABLES:
            ok = _exists(conn, t)
            print(f"  {'✓' if ok else '✗'} table {t}")
            if not ok:
                missing.append(t)

    if missing:
        print(f"\n✗ Missing tables: {missing}")
        print("  Run the Rust daemon (`cargo run --release`) to apply all migrations.")
        return 1

    print("\n✓ meridian-agents bootstrap complete.")
    print("\nNext steps:")
    print("  1. Make sure the Rust daemon has run at least once (pm_tasks is populated by")
    print("     src/intelligence/providers/jira.rs every 30 min).")
    print("  2. Run synthesizer → python -m agents.orchestrator --once")
    return 0


if __name__ == "__main__":
    sys.exit(main())
