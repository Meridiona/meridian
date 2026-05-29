"""Fallback poller for coding_agent_indexer.

Catches sessions that the Claude Code SessionEnd hook missed:
  * crashes / kill -9 / force-quits
  * Codex sessions (no equivalent hook)
  * macOS sleep through a session end
  * hook script failures (bug, missing perms)

Also handles live-tracking (Option B): the currently-active session is
NOT skipped — we want a fresh, up-to-the-tick row for in-progress work
so the PM update workflow can see it. On every tick:

  for each project dir under CLAUDE_PROJECTS_DIR + CODEX_SESSIONS_DIR:
    for each JSONL whose mtime > stored row's ended_at + slack:
       register_ended_session(jsonl_path)

Most ticks are no-ops (change-detection in `_candidate_jsonls`
short-circuits unchanged files via the `MAX(ended_at)` cursor). The
daemon is cheap insurance — single-digit MB RAM, ~50-200 ms per tick.
"""
from __future__ import annotations

import json
import logging
import signal
import threading
import time
from datetime import datetime
from pathlib import Path
from typing import Iterator, Optional

from agents import observability
from coding_agent_indexer import config, db, register

log = logging.getLogger(__name__)

# `threading.Event` instead of a busy-poll sleep loop. Set in `_stop` so
# the main loop wakes immediately on SIGTERM/SIGINT instead of waiting
# out the remainder of its 10-min sleep.
_SHUTDOWN = threading.Event()


def main() -> int:
    observability.setup("meridian-coding-agent-indexer")
    log.info(
        "starting coding_agent_indexer daemon — claude=%s codex=%s poll=%ds",
        config.CLAUDE_PROJECTS_DIR, config.CODEX_SESSIONS_DIR,
        config.POLL_INTERVAL_SECONDS,
    )

    signal.signal(signal.SIGTERM, _stop)
    signal.signal(signal.SIGINT, _stop)

    fork_skip = _load_fork_skip()

    while not _SHUTDOWN.is_set():
        try:
            _tick(fork_skip)
        except Exception as exc:                            # noqa: BLE001
            log.exception("tick failed: %s", exc)

        # Reload fork skip list each tick so a separate summariser
        # process can update it without us restarting.
        try:
            fork_skip = _load_fork_skip()
        except Exception:                                   # noqa: BLE001
            pass

        _SHUTDOWN.wait(timeout=config.POLL_INTERVAL_SECONDS)

    log.info("coding_agent_indexer daemon stopped")
    observability.shutdown()
    return 0


# ──────────────────────── Tick ─────────────────────────────────────────────────


def _tick(fork_skip: set[str]) -> None:
    """One sweep — register anything changed since last tick."""
    now = time.time()
    wrote = skipped = failed = 0

    for jsonl in _candidate_jsonls(now=now):
        result = register.register_ended_session(jsonl, fork_skip_list=fork_skip)
        if result.outcome == register.RegisterOutcome.INSERTED:
            wrote += 1
        elif result.outcome == register.RegisterOutcome.FAILED:
            failed += 1
        else:
            skipped += 1

    if wrote or failed:
        log.info("tick: wrote=%d skipped=%d failed=%d", wrote, skipped, failed)
    else:
        log.debug("tick: wrote=0 skipped=%d (nothing changed)", skipped)


def _candidate_jsonls(*, now: float) -> Iterator[Path]:                    # noqa: ARG001
    """Yield JSONLs whose mtime is past the latest stored `ended_at`.

    Includes the project's currently-active JSONL — live-tracking means
    we want a fresh row each tick for in-progress sessions too.

    Change detection: compare each file's mtime against the latest
    `ended_at` for its uuid. Files where mtime ≤ stored ended_at + 5 s
    slack haven't grown — skip them without parsing.

    Order: oldest-changed first, so a restart after downtime catches
    up the longest-waiting sessions first.
    """
    candidates: list[tuple[float, Path]] = []
    endpoints = db.fetch_session_endpoints()                # {uuid: ended_at_iso}

    for project_dir in _iter_project_dirs():
        try:
            jsonls = list(project_dir.glob("*.jsonl"))
        except (FileNotFoundError, PermissionError, OSError):
            continue
        if not jsonls:
            continue

        for f in jsonls:
            try:
                mtime = f.stat().st_mtime
            except OSError:
                continue

            stored_end_iso = endpoints.get(f.stem)
            if stored_end_iso is not None:
                stored_end_epoch = _iso_to_epoch(stored_end_iso)
                # 5 s slack to absorb clock skew + ISO truncation
                if stored_end_epoch is not None and mtime <= stored_end_epoch + 5.0:
                    continue                                # nothing new since last register

            candidates.append((mtime, f))

    candidates.sort(key=lambda t: t[0])                     # oldest first
    for _, path in candidates:
        yield path


def _iso_to_epoch(iso: str) -> Optional[float]:
    """Parse an ISO-8601 UTC timestamp into Unix epoch seconds."""
    try:
        return datetime.fromisoformat(iso.replace("Z", "+00:00")).timestamp()
    except (ValueError, TypeError):
        return None


def _iter_project_dirs() -> Iterator[Path]:
    """All Claude project dirs + Codex day dirs that currently exist."""
    if config.CLAUDE_PROJECTS_DIR.exists():
        try:
            for child in config.CLAUDE_PROJECTS_DIR.iterdir():
                if child.is_dir():
                    yield child
        except (FileNotFoundError, PermissionError, OSError) as exc:
            log.warning("cannot list claude projects: %s", exc)

    if config.CODEX_SESSIONS_DIR.exists():
        # Codex layout: ~/.codex/sessions/<YYYY>/<MM>/<DD>/rollout-*.jsonl
        # Scan all leaf day dirs.
        try:
            for year in config.CODEX_SESSIONS_DIR.iterdir():
                if not year.is_dir():
                    continue
                for month in year.iterdir():
                    if not month.is_dir():
                        continue
                    for day in month.iterdir():
                        if day.is_dir():
                            yield day
        except (FileNotFoundError, PermissionError, OSError) as exc:
            log.warning("cannot list codex sessions: %s", exc)


# ──────────────────────── Fork skip list ───────────────────────────────────────


def _load_fork_skip() -> set[str]:
    """Tiny JSON file the summariser (phase 2) writes; we just read it."""
    path = config.FORK_SKIP_STATE_PATH
    if not path.exists():
        return set()
    try:
        data = json.loads(path.read_text())
        return set(data.get("fork_uuids", []))
    except Exception as exc:                                # noqa: BLE001
        log.warning("could not load fork-skip state from %s: %s", path, exc)
        return set()


# ──────────────────────── Signal handling ──────────────────────────────────────


def _stop(signum, frame) -> None:                           # noqa: ARG001
    log.info("signal %d received — stopping after current tick", signum)
    _SHUTDOWN.set()


if __name__ == "__main__":
    raise SystemExit(main())
