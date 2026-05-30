"""Low-frequency daemon that drains the summariser queue.

Each tick: fetch a bounded batch of sealed, unsummarised coding segments and
summarise them sequentially (one transcript in memory at a time — flat memory,
no CPU burst, no rate-limit storm). If Claude reports a usage/rate limit, stop
the tick early and sleep a longer backoff instead of hammering the subscription;
the unfinished rows stay NULL and are retried later.

Idle between ticks via `threading.Event.wait`, so the process uses ~no CPU when
there's nothing to do. SIGTERM/SIGINT wake it immediately and it stops after the
in-flight summary.
"""
from __future__ import annotations

import logging
import signal
import threading

from agents import observability
from coding_agent_summariser import config, db, summariser

log = logging.getLogger(__name__)

_SHUTDOWN = threading.Event()


def main() -> int:
    observability.setup("meridian-coding-agent-summariser")
    db.ensure_schema()
    log.info(
        "starting coding_agent_summariser — model=%s poll=%ds batch=%d",
        config.CLAUDE_MODEL, config.POLL_INTERVAL_SECONDS, config.BATCH_PER_TICK,
    )
    signal.signal(signal.SIGTERM, _stop)
    signal.signal(signal.SIGINT, _stop)

    while not _SHUTDOWN.is_set():
        backoff = False
        try:
            backoff = _tick()
        except Exception as exc:                            # noqa: BLE001 — never die on a bad tick
            log.exception("tick failed: %s", exc)
        wait = config.RATE_LIMIT_BACKOFF_SECONDS if backoff else config.POLL_INTERVAL_SECONDS
        if backoff:
            log.warning("rate-limited — backing off %ds", wait)
        _SHUTDOWN.wait(timeout=wait)

    log.info("coding_agent_summariser stopped")
    observability.shutdown()
    return 0


def _tick() -> bool:
    """Summarise up to BATCH_PER_TICK of TODAY's rows. Returns True on rate limit.

    Scoped to the current local day so the daemon never tries to drain all of
    history in one go — past days are backfilled explicitly via the CLI's --day.
    """
    day = config.today_local()
    rows = db.fetch_pending(config.BATCH_PER_TICK, day=day)
    if not rows:
        log.debug("tick: queue empty for %s", day)
        return False

    wrote = failed = 0
    for row in rows:
        if _SHUTDOWN.is_set():
            break
        outcome = summariser.summarise_one(row)
        if outcome.rate_limited and not outcome.written:
            # Claude is limited and MLX didn't cover it — stop now, back off.
            log.warning("rate-limited on row_id=%d; ending tick (wrote=%d)", row.id, wrote)
            return True
        if outcome.written:
            wrote += 1
        elif outcome.error:
            failed += 1
            log.warning("row_id=%d not summarised: %s", row.id, outcome.error)

    log.info("tick: wrote=%d failed=%d of %d candidate(s)", wrote, failed, len(rows))
    return False


def _stop(signum, frame) -> None:                           # noqa: ARG001
    log.info("signal %d received — stopping after current summary", signum)
    _SHUTDOWN.set()


if __name__ == "__main__":
    raise SystemExit(main())
