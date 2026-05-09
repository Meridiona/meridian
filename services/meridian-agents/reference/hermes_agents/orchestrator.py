"""Orchestrator — runs the Watcher, Synthesizer, and Jira Keeper in a coordinated asyncio loop."""
import argparse
import asyncio
import logging
import signal
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent.parent))
from agents.config import (
    HERMES_HOME,
    WATCHER_INTERVAL_SECONDS,
    SYNTHESIZER_INTERVAL_SECONDS,
    CONFIDENCE_THRESHOLD,
)
from agents.watcher import run_watcher
from agents.synthesizer import run_synthesizer
from agents.jira_keeper import run_jira_keeper

_log_path = HERMES_HOME / "logs" / "activity-agent.log"
_log_path.parent.mkdir(parents=True, exist_ok=True)

logging.basicConfig(
    level=logging.INFO,
    format="%(asctime)s [%(levelname)s] %(name)s: %(message)s",
    handlers=[
        logging.FileHandler(_log_path),
        logging.StreamHandler(sys.stdout),
    ],
)
log = logging.getLogger("orchestrator")

_shutdown = asyncio.Event()


# ── Single-shot (--once) ───────────────────────────────────────────────────────
def run_once():
    """Run watcher → synthesizer → jira_keeper once and print a summary."""
    log.info("=" * 60)
    log.info("E2E run — watcher")
    event = run_watcher()
    log.info("Watcher: app=%s | task=%s | conf=%.2f",
             event.get("active_app"), event.get("inferred_task", "?")[:60], event.get("confidence", 0.0))

    log.info("=" * 60)
    log.info("E2E run — synthesizer")
    ctx = run_synthesizer()
    log.info("Synthesizer: project=%s | jira=%s | conf=%.2f | sync=%s",
             ctx.get("active_project"), ctx.get("jira_key"),
             ctx.get("confidence", 0.0), ctx.get("trigger_jira_sync"))

    if ctx.get("trigger_jira_sync") and ctx.get("confidence", 0.0) >= CONFIDENCE_THRESHOLD:
        log.info("=" * 60)
        log.info("E2E run — jira_keeper")
        result = run_jira_keeper()
        log.info("Jira Keeper: status=%s | key=%s | actions=%s",
                 result.get("status"), result.get("jira_key"), result.get("actions", []))
    else:
        log.info("Jira Keeper skipped (trigger_jira_sync=%s conf=%.2f)",
                 ctx.get("trigger_jira_sync"), ctx.get("confidence", 0.0))

    log.info("=" * 60)
    log.info("E2E run complete")


# ── Loop mode ─────────────────────────────────────────────────────────────────
async def _watcher_loop(interval: int):
    log.info("Watcher started (interval=%ds)", interval)
    while not _shutdown.is_set():
        try:
            event = await asyncio.to_thread(run_watcher)
            log.info(
                "Watcher: %s | app=%s | conf=%.2f",
                event.get("inferred_task", "?")[:60],
                event.get("active_app", "?"),
                event.get("confidence", 0.0),
            )
        except Exception as exc:
            log.error("Watcher error: %s", exc, exc_info=True)
        try:
            await asyncio.wait_for(_shutdown.wait(), timeout=interval)
        except asyncio.TimeoutError:
            pass


async def _synthesizer_loop(interval: int):
    log.info("Synthesizer started (interval=%ds)", interval)
    while not _shutdown.is_set():
        try:
            await asyncio.wait_for(_shutdown.wait(), timeout=interval)
        except asyncio.TimeoutError:
            pass
        if _shutdown.is_set():
            break
        try:
            ctx = await asyncio.to_thread(run_synthesizer)
            log.info(
                "Synthesizer: project=%s | jira=%s | conf=%.2f | sync=%s",
                ctx.get("active_project"),
                ctx.get("jira_key"),
                ctx.get("confidence", 0.0),
                ctx.get("trigger_jira_sync"),
            )
            if ctx.get("trigger_jira_sync") and ctx.get("confidence", 0.0) >= CONFIDENCE_THRESHOLD:
                asyncio.create_task(_run_jira_keeper())
        except Exception as exc:
            log.error("Synthesizer error: %s", exc, exc_info=True)


async def _run_jira_keeper():
    try:
        result = await asyncio.to_thread(run_jira_keeper)
        log.info("Jira Keeper: status=%s", result.get("status"))
    except Exception as exc:
        log.error("Jira Keeper error: %s", exc, exc_info=True)


def _handle_signal(sig, _frame):
    log.info("Received %s — shutting down", signal.Signals(sig).name)
    _shutdown.set()


async def _loop_main(watcher_interval: int, synthesizer_interval: int):
    signal.signal(signal.SIGINT, _handle_signal)
    signal.signal(signal.SIGTERM, _handle_signal)

    log.info("Activity Intelligence agent system starting")
    log.info(
        "Watcher every %ds | Synthesizer every %ds | Confidence threshold=%.2f",
        watcher_interval,
        synthesizer_interval,
        CONFIDENCE_THRESHOLD,
    )

    await asyncio.gather(
        _watcher_loop(watcher_interval),
        _synthesizer_loop(synthesizer_interval),
    )

    log.info("Agent system stopped")


def main():
    parser = argparse.ArgumentParser(description="Hermes activity intelligence orchestrator")
    parser.add_argument("--once", action="store_true",
                        help="Run watcher → synthesizer → jira_keeper once and exit")
    parser.add_argument("--watcher-interval",     type=int, default=WATCHER_INTERVAL_SECONDS,
                        help=f"Watcher loop interval in seconds (default: {WATCHER_INTERVAL_SECONDS})")
    parser.add_argument("--synthesizer-interval", type=int, default=SYNTHESIZER_INTERVAL_SECONDS,
                        help=f"Synthesizer loop interval in seconds (default: {SYNTHESIZER_INTERVAL_SECONDS})")
    args = parser.parse_args()

    if args.once:
        run_once()
        return

    asyncio.run(_loop_main(args.watcher_interval, args.synthesizer_interval))


if __name__ == "__main__":
    main()
