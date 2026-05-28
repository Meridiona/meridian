"""Config for the meridian-agents service.

Reads from the environment (via .env files) and exposes the small set of
tunables that the classification pipeline needs.
"""
from __future__ import annotations

import logging as _logging
import os
from datetime import datetime, timezone
from pathlib import Path

from dotenv import load_dotenv

REPO_ROOT = Path(__file__).parent.parent

# Load .env files in priority order (later loads do NOT override earlier ones).
# 1. services/.env            — service-specific overrides (preferred)
# 2. services/.hermes/.env    — repo-local hermes home (OLLAMA_API_KEY lives here)
# 3. <repo>/.env              — repo root, shared with the Rust daemon
# 4. ~/.hermes/.env           — global fallback
_ENV_CANDIDATES = [
    REPO_ROOT / ".env",
    REPO_ROOT / ".hermes" / ".env",
    REPO_ROOT.parent / ".env",
    Path.home() / ".hermes" / ".env",
]
for _candidate in _ENV_CANDIDATES:
    if _candidate.exists():
        load_dotenv(_candidate, override=False)

# ── Hermes (AIAgent library) ──────────────────────────────────────────────────
HERMES_HOME = Path(os.environ.get("HERMES_HOME", str(REPO_ROOT / ".hermes")))

# Directories searched for skill files (SKILL.md, SKILL-*.md).
SKILLS_SEARCH_PATHS: list[Path] = [
    REPO_ROOT / "skills" / "activity",
    HERMES_HOME / "skills",
]

# ── LLM ───────────────────────────────────────────────────────────────────────
MODEL            = os.environ.get("OLLAMA_MODEL")
BASE_URL         = os.environ.get("OLLAMA_HOST")
API_KEY          = os.environ.get("OLLAMA_API_KEY")
AGENT_MAX_TOKENS = int(os.environ.get("AGENT_MAX_TOKENS", "4000"))

if not API_KEY:
    _logging.getLogger("agents.config").warning(
        "OLLAMA_API_KEY is not set — this is fine for local models "
        "but will fail against cloud endpoints that require auth"
    )

# Local model selection — Apple Silicon only.
# LLM_PREFER_LOCAL=1 tries a local model before the cloud AIAgent path.
# LLM_BUDGET_PCT controls the fraction of available Metal headroom to allocate
# (0.5 = 50% of free GPU memory). Set to 0 or LLM_PREFER_LOCAL=0 to disable.


def _env_bool(name: str, default: bool) -> bool:
    raw = os.environ.get(name)
    if raw is None:
        return default
    return raw.strip().lower() not in ("0", "false", "no", "off", "")


LLM_PREFER_LOCAL = _env_bool("LLM_PREFER_LOCAL", True)
LLM_BUDGET_PCT   = float(os.environ.get("LLM_BUDGET_PCT", "0.5"))

# When true, _hermes_setup.ensure_hermes_importable() prepends services/.hermes/
# to sys.path so the local source checkout shadows the installed hermes-agent package.
HERMES_DEV_MODE = os.environ.get("HERMES_DEV_MODE", "0") == "1"

# ── DB / runtime paths ────────────────────────────────────────────────────────
MERIDIAN_HOME = Path(os.environ.get("MERIDIAN_HOME", str(Path.home() / ".meridian")))
MERIDIAN_DB   = Path(os.environ.get("MERIDIAN_DB",   str(MERIDIAN_HOME / "meridian.db")))
LOG_DIR       = MERIDIAN_HOME / "logs"

# ── Loop tunables ─────────────────────────────────────────────────────────────
CONFIDENCE_THRESHOLD = float(os.environ.get("CONFIDENCE_THRESHOLD", "0.65"))
MIN_LLM_DURATION_S   = int(os.environ.get("MIN_LLM_DURATION_S", "30"))

# ── Jira updater ──────────────────────────────────────────────────────────────
UPDATE_INTERVAL_HOURS = float(os.environ.get("UPDATE_INTERVAL_HOURS", "4"))
OFFICE_START_HOUR     = int(os.environ.get("OFFICE_START_HOUR", "9"))
OFFICE_END_HOUR       = int(os.environ.get("OFFICE_END_HOUR", "17"))
JIRA_POST_NO_ACTIVITY = _env_bool("JIRA_POST_NO_ACTIVITY", True)
MERIDIAN_MCP_PATH     = Path(os.environ.get(
    "MERIDIAN_MCP_PATH",
    str(REPO_ROOT.parent / "packages" / "meridian-mcp" / "dist" / "index.js"),
))

# ── Skill loading ─────────────────────────────────────────────────────────────

def load_skill(name: str) -> str:
    """Load the primary SKILL.md for a skill. Returns empty string when absent."""
    for base in SKILLS_SEARCH_PATHS:
        skill_file = base / name / "SKILL.md"
        if skill_file.exists():
            return skill_file.read_text()
    return ""


def load_skill_addendum(name: str, mode: str) -> str:
    """Load the mode-specific addendum for a skill (e.g. SKILL-tiebreak.md).

    Returns empty string when the file is absent so callers can treat missing
    addenda as a no-op rather than an error.
    """
    for base in SKILLS_SEARCH_PATHS:
        addendum_file = base / name / f"SKILL-{mode}.md"
        if addendum_file.exists():
            return addendum_file.read_text()
    return ""


def today_start_utc_iso() -> str:
    """Return today's local-midnight expressed as an ISO-8601 UTC timestamp."""
    local_now = datetime.now().astimezone()
    midnight_local = local_now.replace(hour=0, minute=0, second=0, microsecond=0)
    return midnight_local.astimezone(timezone.utc).isoformat()
