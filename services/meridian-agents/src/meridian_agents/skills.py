# meridian — normalises screenpipe activity into structured app sessions

"""Skill prompt loader for meridian-agents.

Skills are SKILL.md files at `services/meridian-agents/skills/<name>/SKILL.md`,
loaded once at agent startup and used as the system prompt for every LLM call.
The contract matches hermes' upstream `load_skill` function (returns the file
contents as a string) so swapping in hermes-agent's skill loader later is a
one-line change.
"""

from __future__ import annotations

from pathlib import Path

# Skills live next to the package source, not inside it. The repo layout is:
#   services/meridian-agents/
#     pyproject.toml
#     skills/
#       synthesizer/SKILL.md
#     src/meridian_agents/
#       skills.py   (this file)
SERVICE_ROOT = Path(__file__).resolve().parents[2]
SKILLS_DIR = SERVICE_ROOT / "skills"


def load_skill(name: str) -> str:
    """Return the system-prompt text for the named skill.

    Raises FileNotFoundError with the searched path on miss so debugging
    a missing skill doesn't require digging through the loader.
    """
    skill_file = SKILLS_DIR / name / "SKILL.md"
    if not skill_file.is_file():
        raise FileNotFoundError(
            f"Skill {name!r} not found at {skill_file}. "
            f"Expected layout: {SKILLS_DIR}/<name>/SKILL.md"
        )
    return skill_file.read_text()
