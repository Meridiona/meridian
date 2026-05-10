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


# ── DB / runtime paths ────────────────────────────────────────────────────────
MERIDIAN_HOME = Path(os.environ.get("MERIDIAN_HOME", str(Path.home() / ".meridian")))
MERIDIAN_DB   = Path(os.environ.get("MERIDIAN_DB",   str(MERIDIAN_HOME / "meridian.db")))
LOG_DIR       = MERIDIAN_HOME / "logs"

# ── Loop tunables ─────────────────────────────────────────────────────────────
SESSION_BATCH_LIMIT          = int(os.environ.get("SESSION_BATCH_LIMIT", "50"))
CONTEXT_NODES_LIMIT          = int(os.environ.get("CONTEXT_NODES_LIMIT", "100"))
CONFIDENCE_THRESHOLD         = float(os.environ.get("CONFIDENCE_THRESHOLD", "0.65"))

# When ONLY_TODAY is truthy, the synthesizer only considers sessions that
# *started* on or after today's local-midnight. The cursor is still respected,
# so reruns within the same day don't re-tag completed sessions. Set
# ONLY_TODAY=0 to disable (e.g. when backfilling history).
ONLY_TODAY = os.environ.get("ONLY_TODAY", "1").strip() not in ("0", "false", "no", "")

# Pre-filter thresholds — sessions that fail every test are auto-tagged
# `overhead/skip` in Python without burning an LLM call.
MIN_LLM_DURATION_S = int(os.environ.get("MIN_LLM_DURATION_S", "30"))

# Bounded retry/backoff when an LLM call hits a rate limit (HTTP 429).
LLM_RETRY_ATTEMPTS  = max(1, int(os.environ.get("LLM_RETRY_ATTEMPTS",  "3")))
LLM_RETRY_BACKOFF_S = float(os.environ.get("LLM_RETRY_BACKOFF_S", "5"))


# ── Per-stage enable flags ────────────────────────────────────────────────────
# Each stage of the tagger pipeline can be turned off via env if it's giving
# false positives or you want to A/B test something:
#
#     STAGE1_ENABLED   rules + KAN-NN regex + trivial-overhead prefilter
#     STAGE2_ENABLED   bge-small embeddings → top-K candidates
#     STAGE3_ENABLED   small LLM tiebreak via hermes (only fires when
#                      Stage 2 returns routing=queue)
#
# Defaults: all on. CLI `--stage 1,2,3` overrides these for ad-hoc invocations.
def _env_bool(name: str, default: bool) -> bool:
    raw = os.environ.get(name)
    if raw is None:
        return default
    return raw.strip().lower() not in ("0", "false", "no", "off", "")


STAGE1_ENABLED = _env_bool("STAGE1_ENABLED", True)
STAGE2_ENABLED = _env_bool("STAGE2_ENABLED", True)
STAGE3_ENABLED = _env_bool("STAGE3_ENABLED", True)


def default_stages() -> set[int]:
    """Return the stage set the tagger should run with by default.

    Honours the STAGE{1,2,3}_ENABLED env flags. Used by both the daemon
    (long-running) and the CLI (when --stage isn't passed).
    """
    out: set[int] = set()
    if STAGE1_ENABLED:
        out.add(1)
    if STAGE2_ENABLED:
        out.add(2)
    if STAGE3_ENABLED:
        out.add(3)
    return out


def today_start_utc_iso() -> str:
    """Return today's local-midnight expressed as an ISO-8601 UTC timestamp.

    `app_sessions.started_at` is stored UTC, so we convert the local boundary
    into UTC before comparing.
    """
    local_now = datetime.now().astimezone()
    midnight_local = local_now.replace(hour=0, minute=0, second=0, microsecond=0)
    return midnight_local.astimezone(timezone.utc).isoformat()

# ── LLM ───────────────────────────────────────────────────────────────────────
MODEL    = os.environ.get("HERMES_MODEL",    "nemotron-3-super")
BASE_URL = os.environ.get("HERMES_BASE_URL", "https://ollama.com/v1")
API_KEY  = os.environ.get("OLLAMA_API_KEY",  "")
