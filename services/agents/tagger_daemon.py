"""Long-running tagger daemon.

Polls `app_sessions` every TICK_SECS seconds. When new rows have been
written by the Rust ETL since `agent_cursor.last_session_id`, runs the
3-stage tagger pipeline (rules → embeddings → LLM tiebreak) on them.
At idle (no new rows) the loop is a single SELECT 1 short-circuit so we
don't burn CPU.

Designed to be supervised by launchd — see scripts/install-tagger-daemon.sh.
Restart-safe: cursor + UPSERT semantics mean dropping the process at any
point loses at most the in-flight session, which is re-picked on the
next tick. On startup we sweep any zombie `agent_runs` rows from a
crashed previous instance.

Run manually:
    cd services/
    python -m agents.tagger_daemon
    python -m agents.tagger_daemon --tick 5 --stage 1,2,3
"""
from __future__ import annotations

import argparse
import logging
import os
import signal
import sqlite3
import sys
import threading
import time
from datetime import datetime, timezone
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent.parent))
from agents import db                                     # noqa: E402
from agents import tagger                                 # noqa: E402
from agents.config import (                            # noqa: E402
    LOG_DIR, ONLY_TODAY, today_start_utc_iso,
    default_stages, STAGE1_ENABLED, STAGE2_ENABLED, STAGE3_ENABLED,
)

log = logging.getLogger("tagger_daemon")

# ────────────────────────── Config / defaults ─────────────────────────────────
DEFAULT_TICK_SECS  = int(os.environ.get("TAGGER_TICK_SECS", "7"))
HEARTBEAT_SECS     = int(os.environ.get("TAGGER_HEARTBEAT_SECS", "300"))   # 5 min
# 'auto' resolves at parse time to the STAGE{1,2,3}_ENABLED env flags. The
# legacy TAGGER_STAGES env var still works for explicit overrides
# (e.g. `TAGGER_STAGES=1,2`); when unset, we honour the per-stage flags.
DEFAULT_STAGES_RAW = os.environ.get("TAGGER_STAGES", "auto")


# ────────────────────────── Logging ───────────────────────────────────────────
def _configure_logging() -> Path:
    LOG_DIR.mkdir(parents=True, exist_ok=True)
    log_path = LOG_DIR / "tagger-daemon.log"
    level_name = os.environ.get("LOG_LEVEL", "INFO").upper()
    level = getattr(logging, level_name, logging.INFO)
    logging.basicConfig(
        level=level,
        format="%(asctime)s [%(levelname)s] %(name)s: %(message)s",
        handlers=[logging.FileHandler(log_path), logging.StreamHandler(sys.stdout)],
    )
    for noisy in ("httpx", "httpcore", "openai._base_client"):
        logging.getLogger(noisy).setLevel(logging.WARNING)
    log.info("LOG_LEVEL=%s log=%s", logging.getLevelName(level), log_path)
    return log_path


# ────────────────────────── Helpers ───────────────────────────────────────────
def _has_new_work(conn: sqlite3.Connection) -> bool:
    """Cheap probe: is there at least one row in app_sessions past the cursor?"""
    cursor = db.get_cursor(conn)
    row = conn.execute(
        "SELECT 1 FROM app_sessions WHERE id > ? LIMIT 1",
        (int(cursor),),
    ).fetchone()
    return row is not None


def _sweep_zombie_agent_runs(conn: sqlite3.Connection) -> int:
    """Mark any agent_runs left in 'running' state by a previous crash as 'aborted'.

    Mirrors the Rust daemon's `cleanup_incomplete_runs` for ETL. Returns the
    number of rows updated.
    """
    cur = conn.execute(
        """
        UPDATE agent_runs
           SET status      = 'aborted',
               error       = COALESCE(error, 'daemon restart'),
               finished_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now')
         WHERE status = 'running'
        """
    )
    return int(cur.rowcount or 0)


def _summarise_state(conn: sqlite3.Connection) -> dict:
    cursor = db.get_cursor(conn)
    row = conn.execute("SELECT MAX(id) AS m FROM app_sessions").fetchone()
    max_id = int(row["m"] or 0) if row else 0
    return {"cursor": cursor, "max_session_id": max_id, "backlog": max(0, max_id - cursor)}


def _parse_stages(spec: str) -> set[int]:
    """Parse the daemon's --stage CLI flag.

    'auto' (the default) defers to STAGE{1,2,3}_ENABLED env flags via
    config.default_stages(). An explicit comma list overrides them.
    """
    if spec is None or spec.strip() in ("", "auto"):
        return default_stages() or {1}
    out: set[int] = set()
    for piece in spec.split(","):
        piece = piece.strip()
        if not piece:
            continue
        if piece not in ("1", "2", "3"):
            raise ValueError(f"unknown stage {piece!r} (valid: 1, 2, 3)")
        out.add(int(piece))
    return out or default_stages() or {1}


# ────────────────────────── Tick + main loop ──────────────────────────────────
_shutdown = threading.Event()
_in_flight_session_id: int | None = None


def _tick(stages: set[int]) -> dict:
    """Run one poll cycle. Returns a small report dict."""
    with db.connection() as conn:
        if not _has_new_work(conn):
            return {"sessions_processed": 0, "elapsed_s": 0.0}

    since_iso = today_start_utc_iso() if ONLY_TODAY else None
    return tagger.run_once(since_iso=since_iso, stages=stages) or {}


def _install_signal_handlers() -> None:
    def _handle(sig, _frame):
        log.info("daemon: received %s — shutdown requested", signal.Signals(sig).name)
        _shutdown.set()
    signal.signal(signal.SIGINT, _handle)
    signal.signal(signal.SIGTERM, _handle)


def run_forever(tick_secs: int, stages: set[int]) -> None:
    log.info("=" * 76)
    log.info("tagger-daemon up | tick=%ds stages=%s", tick_secs, sorted(stages))
    log.info(
        "stage flags (env): STAGE1_ENABLED=%s  STAGE2_ENABLED=%s  STAGE3_ENABLED=%s",
        STAGE1_ENABLED, STAGE2_ENABLED, STAGE3_ENABLED,
    )
    if not stages:
        log.warning("All stages disabled — daemon will idle until at least one stage is enabled")

    with db.connection() as conn:
        aborted = _sweep_zombie_agent_runs(conn)
        if aborted:
            log.info("zombie sweep: %d agent_runs marked aborted", aborted)
        state = _summarise_state(conn)
        log.info("startup state: cursor=%d  max_session_id=%d  backlog=%d",
                 state["cursor"], state["max_session_id"], state["backlog"])

    last_heartbeat = time.time()
    consecutive_errors = 0

    while not _shutdown.is_set():
        try:
            t0 = time.time()
            report = _tick(stages)
            dt = time.time() - t0
            n = int(report.get("sessions_processed") or report.get("sessions") or 0)
            if n:
                log.info(
                    "tick: run_id=%s sessions=%d kept=%s tickets_decided=%s "
                    "auto=%s elapsed=%.2fs cursor→%s",
                    report.get("run_id"),
                    n,
                    report.get("kept"),
                    report.get("tickets_decided"),
                    report.get("auto_tickets"),
                    dt,
                    report.get("run_id") and "advanced",
                )
                consecutive_errors = 0
                last_heartbeat = time.time()
            else:
                if (time.time() - last_heartbeat) >= HEARTBEAT_SECS:
                    with db.connection() as conn:
                        state = _summarise_state(conn)
                    log.info(
                        "heartbeat: idle  cursor=%d  max_session_id=%d  backlog=%d",
                        state["cursor"], state["max_session_id"], state["backlog"],
                    )
                    last_heartbeat = time.time()
        except Exception as exc:
            consecutive_errors += 1
            log.exception("tick failed (%d consecutive): %s", consecutive_errors, exc)
            # Back off briefly on repeated errors so we don't hammer the DB
            # or LLM provider during an outage.
            if consecutive_errors >= 3:
                back_off = min(60, tick_secs * (2 ** min(consecutive_errors - 2, 4)))
                log.warning("backing off %ds after %d consecutive errors",
                            back_off, consecutive_errors)
                if _shutdown.wait(back_off):
                    break

        # Sleep with shutdown-awareness — wake immediately on signal.
        if _shutdown.wait(timeout=tick_secs):
            break

    log.info("tagger-daemon stopped cleanly")


# ────────────────────────── CLI ───────────────────────────────────────────────
def main() -> None:
    parser = argparse.ArgumentParser(
        description="Long-running tagger daemon — polls app_sessions and runs the 3-stage classifier."
    )
    parser.add_argument(
        "--tick", type=int, default=DEFAULT_TICK_SECS,
        help=f"Poll interval in seconds (default: {DEFAULT_TICK_SECS}, env TAGGER_TICK_SECS).",
    )
    parser.add_argument(
        "--stage", default=DEFAULT_STAGES_RAW,
        help="Comma list of stages to run (e.g. '1,2,3'). Stage 3 only fires when "
             "Stage 2 returns routing=queue.",
    )
    args = parser.parse_args()

    _configure_logging()
    _install_signal_handlers()
    stages = _parse_stages(args.stage)
    run_forever(args.tick, stages)


if __name__ == "__main__":
    main()
