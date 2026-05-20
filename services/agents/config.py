"""Config for the meridian-agents service.

Reads from the environment (via .env files) and exposes the small set of
tunables that run_task_linker needs.
"""
from __future__ import annotations

import os
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
# HERMES_HOME isolates hermes memory + config to this repo.
# The setup-hermes.sh script generates services/.hermes/config.yaml with
# skills.external_dirs pointing to services/skills/activity/.
HERMES_HOME = Path(os.environ.get("HERMES_HOME", str(REPO_ROOT / ".hermes")))

MODEL           = os.environ.get("OLLAMA_MODEL",   "gemma4:31b")
BASE_URL        = os.environ.get("OLLAMA_HOST",    "https://ollama.com/v1")
API_KEY         = os.environ.get("OLLAMA_API_KEY", "")
AGENT_MAX_TOKENS = int(os.environ.get("AGENT_MAX_TOKENS", "4000"))

import logging as _logging
if not API_KEY:
    _logging.getLogger("agents.config").warning(
        "OLLAMA_API_KEY is not set — this is fine for local models "
        "but will fail against cloud endpoints that require auth"
    )
