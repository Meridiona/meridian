import os
from pathlib import Path

from dotenv import load_dotenv

REPO_ROOT = Path(__file__).parent.parent
load_dotenv(REPO_ROOT / ".env")

SKILLS_SEARCH_PATHS = [
    REPO_ROOT / "skills" / "activity",
    Path.home() / ".hermes" / "skills" / "activity",
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

HERMES_HOME = Path.home() / ".hermes"
ACTIVITY_DIR = HERMES_HOME / "activity"
JIRA_DIR = HERMES_HOME / "jira"
MEMORIES_DIR = HERMES_HOME / "memories"

BUFFER_FILE = ACTIVITY_DIR / "buffer.jsonl"
CONTEXT_MAP_FILE = ACTIVITY_DIR / "context_map.json"
CURRENT_CONTEXT_FILE = ACTIVITY_DIR / "current_context.json"
JIRA_STATE_FILE = JIRA_DIR / "jira_state.json"
TICKET_MAPPINGS_FILE = JIRA_DIR / "ticket_mappings.json"

WATCHER_INTERVAL_SECONDS = 180      # 3 minutes
SYNTHESIZER_INTERVAL_SECONDS = 1200  # 20 minutes
CONFIDENCE_THRESHOLD = 0.65
BUFFER_WINDOW_MINUTES = 30
MAX_BUFFER_LINES = 500

MODEL = os.environ.get("HERMES_MODEL", "nemotron-3-super")
BASE_URL = os.environ.get("HERMES_BASE_URL", "https://ollama.com/v1")
API_KEY = os.environ.get("OLLAMA_API_KEY", "")
