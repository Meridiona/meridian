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


# Sessions that cross midnight produce two rows split on this TZ's calendar boundary.
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
# During AI-assisted coding, active work emits records frequently (prompts,
# the agent's streamed tool round-trips). A gap with NO record for >2 min
# almost always means the user stepped away — so capping each gap at 2 min
# credits the brief transition/think and discards the idle remainder.
# (Measured: dropping 5min→2min removed mostly genuine 5–60min idle gaps,
# not real activity.) Env-tunable; raise it if you want longer no-record
# reading stretches to count in full.
ACTIVE_TIME_GAP_CAP_SECONDS = int(os.environ.get("INDEXER_ACTIVE_GAP_CAP_S", "120"))

# ── Segmentation + sealing ────────────────────────────────────────────────────
#
# A coding-agent session is sliced into SEGMENTS split on idle gaps larger
# than this between consecutive messages. A new burst of activity after a
# gap this long becomes a NEW app_sessions row (same claude_session_uuid,
# later segment_started_at), and the prior segment is sealed.
#
# Distinct from ACTIVE_TIME_GAP_CAP_SECONDS: that caps each gap when summing
# *active* duration_s (so a 50-min think within a segment doesn't inflate
# work time); this decides where one row ends and the next begins.
#
# The threshold is STRICTLY exceeded to split: a gap of exactly
# SEGMENT_GAP_SECONDS stays in the same segment (mirrors the Rust ETL's
# "> threshold" gap convention).
SEGMENT_GAP_SECONDS = int(os.environ.get("INDEXER_SEGMENT_GAP_S", "3600"))      # 1 hour

# A live (unsealed) segment whose last message is older than this is
# considered settled and gets sealed by the poll sweep — even if its JSONL
# never changes again (crash, force-quit, macOS sleep, file deleted). Kept
# equal to SEGMENT_GAP_SECONDS so "settled" and "would start a new segment"
# mean the same elapsed idleness.
SEAL_IDLE_SECONDS = int(os.environ.get("INDEXER_SEAL_IDLE_S", str(SEGMENT_GAP_SECONDS)))

# ── Time-box ──────────────────────────────────────────────────────────────────
#
# A long CONTINUOUS session (no >1h gap, never ended) would otherwise stay one
# live row indefinitely — its summary, and the Jira update that depends on it,
# would wait unpredictably. So we also split a segment once its span reaches
# this many seconds: the prior chunk immediately becomes a non-last (→ sealed →
# summarisable) row, giving a fresh Jira-ready summary on a predictable cadence.
# Boundaries are deterministic (start, start+box, start+2·box, …) so re-parses
# stay idempotent. Set to 0 to disable time-boxing (gap/end sealing only).
MAX_SEGMENT_SECONDS = int(os.environ.get("INDEXER_MAX_SEGMENT_S", "3600"))      # 1 hour

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
