"""Jira updater daemon.

Fires `run_update` once per scheduled slot within office hours.  Slots are
computed from OFFICE_START_HOUR, OFFICE_END_HOUR, and UPDATE_INTERVAL_HOURS
(e.g. 9–17 with a 4 h interval → slots at 13:00 and 17:00).

Designed to be supervised by launchd — see scripts/install-jira-updater-daemon.sh.
Run manually:
    cd services/
    python -m agents.jira_updater_daemon                 # daemon (blocks)
    python -m agents.jira_updater_daemon --trigger-now   # one-shot all tasks
    python -m agents.jira_updater_daemon --task KAN-87   # one-shot single task
    python -m agents.jira_updater_daemon --dry-run       # print only
    python -m agents.jira_updater_daemon --interval 2    # custom look-back hours
"""
from __future__ import annotations

import argparse
import asyncio
import functools
import logging
import signal
from datetime import datetime, timedelta, timezone
from pathlib import Path

from opentelemetry import trace as trace_api

from agents import observability
from agents.config import (
    MERIDIAN_DB,
    OFFICE_END_HOUR,
    OFFICE_START_HOUR,
    UPDATE_INTERVAL_HOURS,
)

tracer = observability.setup("meridian-jira-updater")
log = logging.getLogger("jira_updater_daemon")


# ── Slot helpers ──────────────────────────────────────────────────────────────

def compute_slots(office_start: int, office_end: int, interval_h: float) -> list[int]:
    """Hours at which updates fire. E.g. [13, 17] for 9–17 with 4h interval."""
    slots: list[int] = []
    t = office_start + interval_h
    while t <= office_end:
        slots.append(round(t))
        t += interval_h
    if not slots:
        raise ValueError(
            f"No update slots fit interval {interval_h}h within office hours "
            f"{office_start}–{office_end}"
        )
    return slots


def slot_window(slot_hour: int, interval_h: float) -> tuple[str, str]:
    """Return (from_time, to_time) as ISO 8601 UTC for today's slot_hour."""
    now_local = datetime.now().astimezone()
    midnight = now_local.replace(hour=0, minute=0, second=0, microsecond=0)
    to_local = midnight + timedelta(hours=slot_hour)
    from_local = to_local - timedelta(hours=interval_h)
    fmt = "%Y-%m-%dT%H:%M:%SZ"
    return (
        from_local.astimezone(timezone.utc).strftime(fmt),
        to_local.astimezone(timezone.utc).strftime(fmt),
    )


def next_slot_dt(slots: list[int]) -> datetime:
    """Return datetime of the next scheduled slot (may be tomorrow)."""
    if not slots:
        raise ValueError("slots must be non-empty")
    now = datetime.now().astimezone()
    midnight = now.replace(hour=0, minute=0, second=0, microsecond=0)
    for h in sorted(slots):
        dt = midnight + timedelta(hours=h)
        if dt > now:
            return dt
    return midnight + timedelta(days=1, hours=slots[0])


# ── One-shot helper ───────────────────────────────────────────────────────────

def _run_one_shot(
    *,
    interval_h: float,
    task_key: str | None,
    dry_run: bool,
) -> None:
    from agents.jira_updater import run_update

    now_utc = datetime.now(tz=timezone.utc)
    to_time = now_utc.strftime("%Y-%m-%dT%H:%M:%SZ")
    from_time = (now_utc - timedelta(hours=interval_h)).strftime("%Y-%m-%dT%H:%M:%SZ")
    log.info("one-shot window %s → %s task=%s dry_run=%s",
             from_time, to_time, task_key or "all", dry_run)
    results = run_update(
        from_time=from_time,
        to_time=to_time,
        dry_run=dry_run,
        task_filter=task_key,
    )
    for r in results:
        log.info("task=%s state=%s duration=%ds had_activity=%s",
                 r.task_key, r.state, r.duration_s, r.had_activity)


# ── Daemon loop ───────────────────────────────────────────────────────────────

async def daemon_loop(dry_run: bool = False) -> None:
    """Async daemon loop — sleeps until the next slot, then fires run_update.

    run_update is dispatched via run_in_executor so its internal asyncio.run()
    calls (MCP stdio clients) don't conflict with the running event loop.
    """
    from agents.jira_updater import run_update

    slots = compute_slots(OFFICE_START_HOUR, OFFICE_END_HOUR, UPDATE_INTERVAL_HOURS)

    with tracer.start_as_current_span(
        "jira_updater_daemon.startup",
        attributes={
            "office_hours.start": OFFICE_START_HOUR,
            "office_hours.end": OFFICE_END_HOUR,
            "slots": str(slots),
            "interval_h": UPDATE_INTERVAL_HOURS,
        },
    ):
        log.info(
            "jira-updater starting: office=%d–%d slots=%s interval=%.1fh",
            OFFICE_START_HOUR, OFFICE_END_HOUR, slots, UPDATE_INTERVAL_HOURS,
        )

    stop = asyncio.Event()
    loop = asyncio.get_running_loop()
    for sig in (signal.SIGINT, signal.SIGTERM):
        loop.add_signal_handler(sig, stop.set)

    while not stop.is_set():
        nxt = next_slot_dt(slots)
        sleep_s = max(0.0, (nxt - datetime.now().astimezone()).total_seconds())
        log.info("next update at %s (%.0fs)", nxt.strftime("%H:%M"), sleep_s)

        # Wait for the next slot, waking early if stop is signalled.
        # Using wait_for on the coroutine directly avoids the orphaned-task
        # accumulation of the asyncio.shield(ensure_future(...)) pattern.
        try:
            await asyncio.wait_for(stop.wait(), timeout=sleep_s)
            break  # stop was set — exit cleanly
        except asyncio.TimeoutError:
            pass

        if stop.is_set():
            break

        from_time, to_time = slot_window(nxt.hour, UPDATE_INTERVAL_HOURS)
        current_slot_str = nxt.strftime("%Y-%m-%dT%H:%M")
        log.info("running update window %s → %s", from_time, to_time)
        with tracer.start_as_current_span(
            "jira_updater_daemon.tick",
            attributes={"slot": current_slot_str},
        ) as tick_span:
            try:
                # run_update is synchronous and uses asyncio.run() internally.
                # Dispatch to a thread so those nested event loops don't conflict
                # with this one, and so SIGTERM is not blocked during the update.
                fn = functools.partial(
                    run_update, from_time=from_time, to_time=to_time, dry_run=dry_run
                )
                results = await loop.run_in_executor(None, fn)
                for r in results:
                    log.info(
                        "task=%s state=%s duration=%ds had_activity=%s",
                        r.task_key, r.state, r.duration_s, r.had_activity,
                    )
                tick_span.set_attribute("tasks.updated", len(results))
            except Exception as exc:
                tick_span.record_exception(exc)
                tick_span.set_status(trace_api.StatusCode.ERROR, str(exc))
                log.exception("update run failed")

    log.info("jira-updater stopped")


# ── CLI ───────────────────────────────────────────────────────────────────────

def main() -> None:
    parser = argparse.ArgumentParser(
        description="Jira updater daemon — fires run_update on a slot schedule within office hours."
    )
    parser.add_argument(
        "--trigger-now", action="store_true",
        help="Run a one-shot update immediately (all tasks) then exit.",
    )
    parser.add_argument(
        "--task", metavar="KEY",
        help="One-shot update for a single Jira task key (e.g. KAN-87), then exit.",
    )
    parser.add_argument(
        "--dry-run", action="store_true",
        help="Print what would be posted to Jira without writing anything.",
    )
    parser.add_argument(
        "--interval", type=float, default=UPDATE_INTERVAL_HOURS, metavar="HOURS",
        help=f"Look-back window in hours for one-shot runs (default: {UPDATE_INTERVAL_HOURS}).",
    )
    args = parser.parse_args()

    if args.trigger_now or args.task:
        _run_one_shot(
            interval_h=args.interval,
            task_key=args.task,
            dry_run=args.dry_run,
        )
        return

    asyncio.run(daemon_loop(dry_run=args.dry_run))


# Both `python -m agents.jira_updater_daemon` and `python -m agents.jira_updater`
# (if __main__.py is added there) resolve to this entry point.
if __name__ == "__main__":
    main()
