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


# ── Hot-toggle override (read live by the daemon every tick) ─────────────────
# A JSON file at ~/.meridian/tagger.config.json (or whatever
# TAGGER_CONFIG_FILE points at) lets you flip stages on/off WHILE the
# daemon is running, without restarting it. Schema:
#
#     { "stage1": true, "stage2": false, "stage3": true }
#
# The daemon re-reads this file every tick. If the file is absent or
# malformed, we fall back to default_stages() (the env-driven flags).
# Boolean fields can also be 1/0 / "true"/"false" / "yes"/"no" / "on"/"off".
#
# CLI helpers `tagger --enable-stage N` / `--disable-stage N` write this
# file for you so you don't have to hand-edit JSON.
TAGGER_CONFIG_FILE = Path(os.environ.get(
    "TAGGER_CONFIG_FILE",
    str(MERIDIAN_HOME / "tagger.config.json"),
))


def _coerce_bool(value: object) -> bool | None:
    if isinstance(value, bool):
        return value
    if isinstance(value, (int, float)):
        return bool(value)
    if isinstance(value, str):
        s = value.strip().lower()
        if s in ("1", "true", "yes", "on"):
            return True
        if s in ("0", "false", "no", "off", ""):
            return False
    return None


def stages_from_file() -> set[int] | None:
    """Return the override stage set from TAGGER_CONFIG_FILE, or None.

    None means "no override file present (or unreadable)" — caller falls
    back to default_stages(). An empty set is a valid value: it means
    "all stages are explicitly disabled via the override file".
    """
    import json
    path = TAGGER_CONFIG_FILE
    try:
        if not path.exists():
            return None
        data = json.loads(path.read_text())
    except (json.JSONDecodeError, OSError):
        return None
    if not isinstance(data, dict):
        return None
    out: set[int] = set()
    for stage_num, key in ((1, "stage1"), (2, "stage2"), (3, "stage3")):
        coerced = _coerce_bool(data.get(key))
        if coerced is True:
            out.add(stage_num)
    return out


def current_stages() -> set[int]:
    """Resolved live stage set: file override beats env default.

    Used by the daemon at each tick so the user can flip stages on/off
    without a restart. CLI invocations that pass an explicit --stage
    short-circuit this — see tagger_daemon's --stage handling.
    """
    override = stages_from_file()
    if override is not None:
        return override
    return default_stages()


def write_stages_override(*, stage1: bool, stage2: bool, stage3: bool) -> Path:
    """Write the override file with the supplied stage flags. Returns the path."""
    import json
    path = TAGGER_CONFIG_FILE
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(
        {"stage1": bool(stage1), "stage2": bool(stage2), "stage3": bool(stage3)},
        indent=2,
    ) + "\n")
    return path


def clear_stages_override() -> Path | None:
    """Delete the override file so env defaults take over again."""
    path = TAGGER_CONFIG_FILE
    if path.exists():
        path.unlink()
        return path
    return None


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
BASE_URL = os.environ.get("OLLAMA_HOST",    "https://api.ollama.ai")
API_KEY  = os.environ.get("OLLAMA_API_KEY", "")

# When true, _hermes_setup.ensure_hermes_importable() prepends services/.hermes/
# to sys.path so the local source checkout shadows the installed hermes-agent
# package. Set HERMES_DEV_MODE=1 in your .env for breakpoint debugging.
HERMES_DEV_MODE = os.environ.get("HERMES_DEV_MODE", "0") == "1"
