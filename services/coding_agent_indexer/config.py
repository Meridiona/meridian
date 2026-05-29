"""Config for coding_agent_indexer.

All tunables are env-driven so the launchd plist or the SessionEnd hook
can override without code changes. Defaults are chosen for a single-user
laptop with the standard Claude Code + Codex install locations.
"""
from __future__ import annotations

import os
from datetime import datetime, tzinfo
from pathlib import Path

# Re-export shared meridian paths so callers don't have to know about the
# parent agents.config module.
from agents.config import LOG_DIR, MERIDIAN_DB, MERIDIAN_HOME  # noqa: F401


def _resolve_local_tz() -> tzinfo:
    """User-local TZ for day bucketing.

    Honours `MERIDIAN_TZ` env (e.g. 'Asia/Kolkata') when set; otherwise
    falls back to the host machine's local TZ via `datetime.astimezone`.
    """
    explicit = os.environ.get("MERIDIAN_TZ", "").strip()
    if explicit:
        try:
            from zoneinfo import ZoneInfo
            return ZoneInfo(explicit)
        except Exception:
            pass                                            # fall through to host TZ
    tz = datetime.now().astimezone().tzinfo
    if tz is not None:
        return tz
    # Last resort — system clock without TZ info. Use UTC so we never raise.
    from datetime import timezone
    return timezone.utc


# Days for `app_sessions.day_utc` are bucketed in this TZ. A continuous
# coding session that crosses midnight produces two rows split on this
# TZ's calendar boundary.
LOCAL_TZ = _resolve_local_tz()

# ── Source dirs to watch ──────────────────────────────────────────────────────

CLAUDE_PROJECTS_DIR = Path(
    os.environ.get("CLAUDE_PROJECTS_DIR", "~/.claude/projects")
).expanduser()

CODEX_SESSIONS_DIR = Path(
    os.environ.get("CODEX_SESSIONS_DIR", "~/.codex/sessions")
).expanduser()


# ── Cadence ───────────────────────────────────────────────────────────────────
#
# The fallback poll runs this often. The Claude Code SessionEnd hook
# catches ~99% of sessions in real time; the poll sweeps up crashes,
# force-kills, macOS-sleep cases, and Codex sessions (no hook).
#
# With live-tracking (every tick UPSERTs the active session's row),
# this is also the upper-bound staleness window for the currently
# in-progress conversation in `app_sessions`.
POLL_INTERVAL_SECONDS = int(os.environ.get("INDEXER_POLL_INTERVAL_S", "600"))      # 10 min

# ── Active-time calculation ───────────────────────────────────────────────────
#
# A session's `duration_s` is gap-capped active time, NOT wall-clock.
# We walk consecutive records and sum the time between them, capping
# each gap at this threshold. Gaps bigger than this (lunch break,
# overnight, day off) don't contribute — the user wasn't actively
# engaged during them.
#
# 5 min is the sweet spot: short enough that a coffee break stops
# counting, long enough that a slow Claude response or a moment of
# thinking still counts as active time.
ACTIVE_TIME_GAP_CAP_SECONDS = int(os.environ.get("INDEXER_ACTIVE_GAP_CAP_S", "300"))

# ── Fork skip list ────────────────────────────────────────────────────────────
#
# The summariser (separate process, phase 2) creates throw-away
# `claude --fork-session` files. Their session_uuids are persisted here
# so the indexer ignores them on next poll. Phase-1 indexer just reads
# the file; it doesn't write to it (the summariser will, once it exists).
FORK_SKIP_STATE_PATH = Path(
    os.environ.get(
        "INDEXER_FORK_SKIP_STATE",
        str(MERIDIAN_HOME / "coding_agent_indexer_state.json"),
    )
)
