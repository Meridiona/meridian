"""Config for coding_agent_summariser — all env-driven.

Defaults suit a single-user laptop on a Claude Max subscription with the local
MLX server on :7823 as the fallback judge.
"""
from __future__ import annotations

import os
from datetime import datetime

from agents.config import LOG_DIR, MERIDIAN_DB, MERIDIAN_HOME  # noqa: F401
from coding_agent_indexer.config import LOCAL_TZ


def today_local() -> str:
    """Current local calendar day as 'YYYY-MM-DD', matching substr(started_at,1,10)."""
    return datetime.now(LOCAL_TZ).strftime("%Y-%m-%d")

# ── Cadence / batching ────────────────────────────────────────────────────────
#
# Summarisation is not latency-critical (it feeds periodic PM updates), so we
# poll infrequently and process a bounded batch per tick. Sequential, never
# parallel — that keeps memory flat (one transcript at a time) and avoids
# tripping the subscription's rate limit with a burst.
POLL_INTERVAL_SECONDS = int(os.environ.get("SUMMARISER_POLL_INTERVAL_S", "300"))   # 5 min
BATCH_PER_TICK        = int(os.environ.get("SUMMARISER_BATCH_PER_TICK", "8"))

# ── Model / invocation ────────────────────────────────────────────────────────
#
# Haiku by default: ~2.5x lighter on Max usage limits and ~3x faster than Sonnet,
# with the skill prompt hardened to still capture blockers/rework. Override to a
# Sonnet/Opus id for richer summaries.
CLAUDE_MODEL      = os.environ.get("SUMMARISER_MODEL", "claude-haiku-4-5-20251001")
SKILL_NAME        = os.environ.get("SUMMARISER_SKILL", "session-summary")
CLAUDE_TIMEOUT_S  = int(os.environ.get("SUMMARISER_CLAUDE_TIMEOUT_S", "240"))

# Codex-flavoured sessions are summarised via `codex exec` using the user's
# Codex auth (symmetry with Claude). Empty model → codex's configured default.
CODEX_MODEL       = os.environ.get("SUMMARISER_CODEX_MODEL", "")
CODEX_TIMEOUT_S   = int(os.environ.get("SUMMARISER_CODEX_TIMEOUT_S", "240"))

# Transcript hard cap (chars) fed to the model. Most bursts are well under this;
# the cap bounds memory + token cost for a pathological 1 MB+ transcript. We keep
# the HEAD and TAIL (where the task and the outcome live) and elide the middle.
TRANSCRIPT_CAP_CHARS = int(os.environ.get("SUMMARISER_TRANSCRIPT_CAP", "500000"))

# ── Fallback (local MLX) ──────────────────────────────────────────────────────
MLX_HOST          = os.environ.get("MLX_SERVER_HOST", "127.0.0.1")
MLX_PORT          = int(os.environ.get("MLX_SERVER_PORT", "7823"))
MLX_TIMEOUT_S     = int(os.environ.get("SUMMARISER_MLX_TIMEOUT_S", "180"))
MLX_MAX_TOKENS    = int(os.environ.get("SUMMARISER_MLX_MAX_TOKENS", "2048"))   # output cap

# MLX INPUT cap (MLX-only — Claude/Codex always get the full transcript). The
# local model receives just the TAIL (most recent activity / outcome) of the
# transcript, bounded to ~this many tokens. Token count is approximated by
# chars (no tokenizer in this lightweight process): cap_chars = tokens × ratio.
MLX_INPUT_MAX_TOKENS  = int(os.environ.get("SUMMARISER_MLX_INPUT_TOKENS", "5000"))
MLX_CHARS_PER_TOKEN   = int(os.environ.get("SUMMARISER_MLX_CHARS_PER_TOKEN", "4"))

# ── Backoff ───────────────────────────────────────────────────────────────────
#
# When BOTH Claude (rate-limited) and MLX (down) fail, sleep this long before the
# next tick instead of hammering. Rows stay NULL and are retried later.
RATE_LIMIT_BACKOFF_SECONDS = int(os.environ.get("SUMMARISER_BACKOFF_S", "1800"))   # 30 min

# ── Noise filter ──────────────────────────────────────────────────────────────
#
# Skip trivial / empty-work segments (rate-limit blips, accidental opens, an
# imported-session marker). A row must clear BOTH bars to be worth a summary —
# at least this many turns AND this much rendered transcript.
MIN_TURNS         = int(os.environ.get("SUMMARISER_MIN_TURNS", "2"))
MIN_TEXT_BYTES    = int(os.environ.get("SUMMARISER_MIN_TEXT_BYTES", "800"))

# task_method transitions (mirrors db.py constants in coding_agent_indexer)
TASK_METHOD_PENDING    = "pending_summariser"   # queue marker the indexer sets
TASK_METHOD_SUMMARISED = "summarised"           # terminal marker we set on success
SESSION_TEXT_SOURCE    = "summary"              # so downstream knows it's prose, not OCR
