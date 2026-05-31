"""Config for the pm_worklog_update workflow.

All tunables are read from the environment (loaded by the parent
`agents.config` module). This file exposes them as typed constants so the
rest of the package can `from agents.pm_worklog_update.config import X`.

The defaults are chosen for a single-user laptop setup with the local 9B
MLX server on :7823 and the user's screen-capture history in
`~/.meridian/meridian.db`.
"""
from __future__ import annotations

import os

# Re-export the shared paths so callers don't have to know about the parent
# config module.
from agents.config import LOG_DIR, MERIDIAN_DB, MERIDIAN_HOME  # noqa: F401

# ── Cadence ───────────────────────────────────────────────────────────────────
# How often the daemon should fire a new PM update cycle, in hours.
# 3h is the recommended default — enough material to summarise, infrequent
# enough not to spam the ticket.
PM_WORKLOG_INTERVAL_HOURS = float(os.environ.get("PM_WORKLOG_INTERVAL_HOURS", "1"))

# ── Model / inference ─────────────────────────────────────────────────────────
# The local MLX server is OpenAI-compatible; we point agno at it via
# OpenAILike. The port is the same one the classifier uses.
MLX_SERVER_HOST   = os.environ.get("MLX_SERVER_HOST", "127.0.0.1")
MLX_SERVER_PORT   = int(os.environ.get("MLX_SERVER_PORT", "7823"))
MLX_SERVER_MODEL  = os.environ.get("MLX_SERVER_MODEL", "qwen3.5-9b-instruct")

# Token caps. The MLX model exposes 128-262K context — a single Synthesise
# call comfortably swallows even the heaviest hour of work.
PM_WORKLOG_SYNTH_MAX_TOKENS  = int(os.environ.get("PM_WORKLOG_SYNTH_MAX_TOKENS",  "8000"))
PM_WORKLOG_REQUEST_TIMEOUT_S = int(os.environ.get("PM_WORKLOG_REQUEST_TIMEOUT_S", "300"))

# Temperature tuned for each step. Lower = more deterministic.
PM_WORKLOG_TEMP_COLLECT = 0.0
PM_WORKLOG_TEMP_SYNTH   = float(os.environ.get("PM_WORKLOG_TEMP_SYNTH",   "0.3"))
PM_WORKLOG_TEMP_COMPOSE = float(os.environ.get("PM_WORKLOG_TEMP_COMPOSE", "0.5"))

# ── Routing / posting thresholds ──────────────────────────────────────────────
# Coverage = fraction of generated bullets that have at least one
# evidence_ref pointing into the source session bundle. Below this, the
# update is held back from auto-post.
PM_WORKLOG_MIN_COVERAGE   = float(os.environ.get("PM_WORKLOG_MIN_COVERAGE",   "0.80"))
PM_WORKLOG_MIN_CONFIDENCE = float(os.environ.get("PM_WORKLOG_MIN_CONFIDENCE", "0.65"))

# Above this fraction of duration in `idle_frame_count`, mark the cycle as
# probably idle (lunch / meeting in the background) and shrink time_spent.
PM_WORKLOG_IDLE_DISCOUNT_THRESHOLD = float(
    os.environ.get("PM_WORKLOG_IDLE_DISCOUNT_THRESHOLD", "0.50")
)

# ── Heavy-path signal (informational only) ────────────────────────────────────
# `SessionBundle.is_heavy` is still computed and stored on the row for
# observability and possible future optimisation, but the workflow no
# longer branches on it — every bundle flows through a single Synthesise.
PM_WORKLOG_HEAVY_SESSION_COUNT = int(os.environ.get("PM_WORKLOG_HEAVY_SESSION_COUNT", "60"))
PM_WORKLOG_HEAVY_TEXT_BYTES    = int(os.environ.get("PM_WORKLOG_HEAVY_TEXT_BYTES",    "400000"))

