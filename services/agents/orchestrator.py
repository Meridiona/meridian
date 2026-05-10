"""Orchestrator — runs the Synthesizer on a fixed cadence against meridian.db.

The legacy watcher loop (screenpipe-MCP polling) is gone: the Rust ETL already
publishes normalised sessions into meridian.db, so the synthesizer reads
straight from there. The jira-keeper drain loop is intentionally not wired
in yet — it will read dispatch_queue in a follow-up.
"""
from __future__ import annotations

import argparse
import asyncio
import logging
import os
import signal
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent.parent))
from agents.config import (
    LOG_DIR,
    SYNTHESIZER_INTERVAL_SECONDS,
)
from agents.synthesizer import run_synthesizer

LOG_DIR.mkdir(parents=True, exist_ok=True)
_log_path = LOG_DIR / "agents.log"

# LOG_LEVEL env var overrides the default. Use DEBUG to see the raw
# bundle/sessions/pm_tasks dumps from synthesizer._log_db_inputs / _log_bundle.
_level_name = os.environ.get("LOG_LEVEL", "INFO").upper()
_level = getattr(logging, _level_name, logging.INFO)

logging.basicConfig(
    level=_level,
    format="%(asctime)s [%(levelname)s] %(name)s: %(message)s",
    handlers=[
        logging.FileHandler(_log_path),
        logging.StreamHandler(sys.stdout),
    ],
)
log = logging.getLogger("orchestrator")
log.info("LOG_LEVEL=%s (file=%s)", logging.getLevelName(_level), _log_path)

_shutdown = asyncio.Event()


# ── Single-shot ────────────────────────────────────────────────────────────────
def run_once() -> None:
    log.info("=" * 60)
    log.info("E2E run — synthesizer (one-shot)")
    ctx = run_synthesizer()
    log.info(
        "Synthesizer: project=%s | jira=%s | conf=%.2f | sync=%s",
        ctx.get("active_project"),
        ctx.get("jira_key"),
        ctx.get("confidence", 0.0),
        ctx.get("trigger_jira_sync"),
    )
    log.info("=" * 60)


# ── Loop mode ──────────────────────────────────────────────────────────────────
async def _synthesizer_loop(interval: int) -> None:
    log.info("Synthesizer loop started (interval=%ds)", interval)
    # Run once immediately so the first cycle does not wait `interval` seconds.
    while not _shutdown.is_set():
        try:
            ctx = await asyncio.to_thread(run_synthesizer)
            log.info(
                "Synthesizer: project=%s | jira=%s | conf=%.2f | sync=%s",
                ctx.get("active_project"),
                ctx.get("jira_key"),
                ctx.get("confidence", 0.0),
                ctx.get("trigger_jira_sync"),
            )
        except Exception as exc:
            log.error("Synthesizer error: %s", exc, exc_info=True)

        try:
            await asyncio.wait_for(_shutdown.wait(), timeout=interval)
        except asyncio.TimeoutError:
            pass


def _handle_signal(sig, _frame) -> None:
    log.info("Received %s — shutting down", signal.Signals(sig).name)
    _shutdown.set()


async def _loop_main(synthesizer_interval: int) -> None:
    signal.signal(signal.SIGINT, _handle_signal)
    signal.signal(signal.SIGTERM, _handle_signal)

    log.info("meridian-agents starting | synthesizer every %ds", synthesizer_interval)
    await _synthesizer_loop(synthesizer_interval)
    log.info("meridian-agents stopped")


def main() -> None:
    parser = argparse.ArgumentParser(description="meridian-agents orchestrator")
    parser.add_argument("--once", action="store_true",
                        help="Run one synthesizer pass and exit")
    parser.add_argument("--synthesizer-interval", type=int,
                        default=SYNTHESIZER_INTERVAL_SECONDS,
                        help=f"Loop interval in seconds (default: {SYNTHESIZER_INTERVAL_SECONDS})")
    args = parser.parse_args()

    if args.once:
        run_once()
        return

    asyncio.run(_loop_main(args.synthesizer_interval))


if __name__ == "__main__":
    main()
