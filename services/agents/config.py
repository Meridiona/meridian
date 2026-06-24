"""Config for the meridian-agents service.

Reads from the environment (via .env files) and exposes the small set of
tunables that the classification pipeline needs.
"""
from __future__ import annotations

import os
from pathlib import Path

from dotenv import load_dotenv

REPO_ROOT = Path(__file__).parent.parent      # the services/ directory
PROJECT_ROOT = REPO_ROOT.parent               # the repo root

# Single source of truth: the repo-root .env, shared with the Rust daemon.
# Nothing is read from outside the repo.
_ENV_FILE = PROJECT_ROOT / ".env"
if _ENV_FILE.exists():
    load_dotenv(_ENV_FILE, override=False)


def _env_bool(name: str, default: bool) -> bool:
    raw = os.environ.get(name)
    if raw is None:
        return default
    return raw.strip().lower() not in ("0", "false", "no", "off", "")


# ── DB / runtime paths ────────────────────────────────────────────────────────
MERIDIAN_HOME = Path(os.environ.get("MERIDIAN_HOME", str(Path.home() / ".meridian")))
MERIDIAN_DB   = Path(os.environ.get("MERIDIAN_DB",   str(MERIDIAN_HOME / "meridian.db")))

# ── Session distiller (session_distiller.py) ──────────────────────────────────
# Coding-agent sessions are excluded — their transcripts are indexed separately.
DISTILLER_EXCLUDE_APPS: tuple[str, ...] = (
    "Claude Code", "Codex", "GitHub Copilot", "Cursor Agent",
)
DISTILLER_MIN_SESSION_DUR: int  = int(os.getenv("DISTILLER_MIN_SESSION_DUR", "15"))
DISTILLER_SEM_DEDUP_THR:   float = float(os.getenv("DISTILLER_SEM_DEDUP_THR", "0.86"))
DISTILLER_DF_FRAC:         float = float(os.getenv("DISTILLER_DF_FRAC", "0.25"))
