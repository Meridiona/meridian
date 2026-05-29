"""Config for coding_agent_indexer — all tunables are env-driven.

Defaults suit a single-user laptop with the standard Claude Code and
Codex install locations. Override via environment variables so the
launchd plist or the SessionEnd hook can change behaviour without code.
"""
from __future__ import annotations

import os
from datetime import datetime, timezone, tzinfo
from pathlib import Path

from agents.config import LOG_DIR, MERIDIAN_DB, MERIDIAN_HOME  # noqa: F401


def _resolve_local_tz() -> tzinfo:
    """Return the user's local TZ for calendar-day bucketing.

    Honours `MERIDIAN_TZ` (e.g. 'Asia/Kolkata') when set; otherwise
    falls back to the host machine's local TZ. Never raises.
    """
    explicit = os.environ.get("MERIDIAN_TZ", "").strip()
    if explicit:
        try:
            from zoneinfo import ZoneInfo
            return ZoneInfo(explicit)
        except Exception:
            pass
    tz = datetime.now().astimezone().tzinfo
    return tz if tz is not None else timezone.utc


# Day bucketing TZ. A session crossing midnight produces two rows split
# on this TZ's calendar boundary.
LOCAL_TZ = _resolve_local_tz()

# ── Source directories ────────────────────────────────────────────────────────

CLAUDE_PROJECTS_DIR = Path(
    os.environ.get("CLAUDE_PROJECTS_DIR", "~/.claude/projects")
).expanduser()

# Codex layout: ~/.codex/sessions/<YYYY>/<MM>/<DD>/rollout-*.jsonl
CODEX_SESSIONS_DIR = Path(
    os.environ.get("CODEX_SESSIONS_DIR", "~/.codex/sessions")
).expanduser()

# ── Daemon cadence ────────────────────────────────────────────────────────────
#
# The SessionEnd hook catches ~99 % of Claude Code sessions in real time.
# The daemon sweeps up crashes, force-quits, macOS-sleep, and ALL Codex
# sessions (no hook). Also provides live-tracking: in-progress sessions
# are re-UPSERTed each tick so the PM workflow sees fresh data.
POLL_INTERVAL_SECONDS = int(os.environ.get("INDEXER_POLL_INTERVAL_S", "600"))

# ── Active-time cap ───────────────────────────────────────────────────────────
#
# duration_s = gap-capped active time, not wall-clock. Each inter-record
# gap is clamped to this value before being summed. Gaps longer than this
# (lunch, overnight, day off) don't count as active engagement.
ACTIVE_TIME_GAP_CAP_SECONDS = int(os.environ.get("INDEXER_ACTIVE_GAP_CAP_S", "300"))

# ── Fork skip list ────────────────────────────────────────────────────────────
#
# The summariser creates throw-away `claude --fork-session` JSONLs whose
# UUIDs are stored here so the indexer ignores them. The indexer only
# reads this file; the summariser writes to it.
FORK_SKIP_STATE_PATH = Path(
    os.environ.get(
        "INDEXER_FORK_SKIP_STATE",
        str(MERIDIAN_HOME / "coding_agent_indexer_state.json"),
    )
)
