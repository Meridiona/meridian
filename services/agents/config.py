"""Config for the meridian-agents service.

Reads from the environment (via .env in the service root) and exposes the
small set of paths and tunables the synthesizer needs. The agent state lives
in meridian.db now — the old ~/.hermes/* JSON files are gone.
"""
from __future__ import annotations

import os
from datetime import datetime, timezone
from pathlib import Path

from dotenv import load_dotenv

REPO_ROOT = Path(__file__).parent.parent

# Load .env files in priority order (later loads do NOT override earlier ones).
# 1. services/.env       — service-specific overrides (preferred)
# 2. <repo>/.env          — repo root, shared with the Rust daemon
# 3. ~/.hermes/.env       — legacy hermes home, where OLLAMA_API_KEY etc. live
_ENV_CANDIDATES = [
    REPO_ROOT / ".env",
    REPO_ROOT.parent / ".env",
    Path.home() / ".hermes" / ".env",
]
for _candidate in _ENV_CANDIDATES:
    if _candidate.exists():
        load_dotenv(_candidate, override=False)

SKILLS_SEARCH_PATHS = [
    REPO_ROOT / "skills" / "activity",
    Path.home() / ".meridian" / "skills" / "activity",
]


def load_skill(name: str) -> str:
    for base in SKILLS_SEARCH_PATHS:
        skill_file = base / name / "SKILL.md"
        if skill_file.exists():
            return skill_file.read_text()
    raise FileNotFoundError(
        f"Skill {name!r} not found in any of: "
        + ", ".join(str(p) for p in SKILLS_SEARCH_PATHS)
    )


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


# ── DB / runtime paths ────────────────────────────────────────────────────────
MERIDIAN_HOME = Path(os.environ.get("MERIDIAN_HOME", str(Path.home() / ".meridian")))
MERIDIAN_DB   = Path(os.environ.get("MERIDIAN_DB",   str(MERIDIAN_HOME / "meridian.db")))
LOG_DIR       = MERIDIAN_HOME / "logs"

# State files used by jira_keeper to persist sync history between runs.
JIRA_DIR              = MERIDIAN_HOME / "jira"
JIRA_STATE_FILE       = JIRA_DIR / "jira_state.json"
CURRENT_CONTEXT_FILE  = JIRA_DIR / "current_context.json"

# ── Loop tunables ─────────────────────────────────────────────────────────────
CONFIDENCE_THRESHOLD         = float(os.environ.get("CONFIDENCE_THRESHOLD", "0.65"))

# Pre-filter threshold — sessions shorter than this skip the LLM call.
MIN_LLM_DURATION_S = int(os.environ.get("MIN_LLM_DURATION_S", "30"))


def _env_bool(name: str, default: bool) -> bool:
    raw = os.environ.get(name)
    if raw is None:
        return default
    return raw.strip().lower() not in ("0", "false", "no", "off", "")


def today_start_utc_iso() -> str:
    """Return today's local-midnight expressed as an ISO-8601 UTC timestamp.

    `app_sessions.started_at` is stored UTC, so we convert the local boundary
    into UTC before comparing.
    """
    local_now = datetime.now().astimezone()
    midnight_local = local_now.replace(hour=0, minute=0, second=0, microsecond=0)
    return midnight_local.astimezone(timezone.utc).isoformat()

# ── LLM ───────────────────────────────────────────────────────────────────────
MODEL    = os.environ.get("OLLAMA_MODEL",   "gemma4:31b-cloud")
BASE_URL = os.environ.get("OLLAMA_HOST",    "https://ollama.com/v1")
API_KEY  = os.environ.get("OLLAMA_API_KEY", "")

# When true, _hermes_setup.ensure_hermes_importable() prepends services/.hermes/
# to sys.path so the local source checkout shadows the installed hermes-agent
# package. Set HERMES_DEV_MODE=1 in your .env for breakpoint debugging.
HERMES_DEV_MODE = os.environ.get("HERMES_DEV_MODE", "0") == "1"

# ── Jira updater ──────────────────────────────────────────────────────────────
UPDATE_INTERVAL_HOURS = float(os.environ.get("UPDATE_INTERVAL_HOURS", "4"))
OFFICE_START_HOUR     = int(os.environ.get("OFFICE_START_HOUR", "9"))
OFFICE_END_HOUR       = int(os.environ.get("OFFICE_END_HOUR", "17"))
JIRA_POST_NO_ACTIVITY = _env_bool("JIRA_POST_NO_ACTIVITY", True)
MERIDIAN_MCP_PATH     = Path(os.environ.get(
    "MERIDIAN_MCP_PATH",
    str(REPO_ROOT.parent / "packages" / "meridian-mcp" / "dist" / "index.js"),
))
