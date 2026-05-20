"""run_jira_updater — thin entry point spawned by the Rust daemon.

Runs one Jira update cycle for the look-back window ending now.
Exits 0 on success (including no-op when tasks have no activity).
Exits 1 on unhandled error.

Spawned by src/intelligence/jira_updater.rs after the Rust daemon
verifies that the update interval has elapsed and office hours apply.
"""
from __future__ import annotations

import logging
import sys
from datetime import datetime, timedelta, timezone

from agents import observability
from agents.config import UPDATE_INTERVAL_HOURS
from agents.jira_updater import run_update

observability.setup("meridian-jira-updater")
log = logging.getLogger("agents.run_jira_updater")


def main() -> None:
    now = datetime.now(tz=timezone.utc)
    to_time = now.strftime("%Y-%m-%dT%H:%M:%SZ")
    from_time = (now - timedelta(hours=UPDATE_INTERVAL_HOURS)).strftime(
        "%Y-%m-%dT%H:%M:%SZ"
    )
    log.info("jira update window %s → %s", from_time, to_time)
    results = run_update(from_time=from_time, to_time=to_time)
    for r in results:
        log.info(
            "task=%s state=%s duration_s=%d had_activity=%s",
            r.task_key, r.state, r.duration_s, r.had_activity,
        )


if __name__ == "__main__":
    try:
        main()
    except Exception:
        log.exception("run_jira_updater failed")
        sys.exit(1)
