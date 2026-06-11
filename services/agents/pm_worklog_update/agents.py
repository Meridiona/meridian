"""Agent definitions for the pm_worklog_update workflow.

The Synthesise agent uses agno's Skills system (LocalSkills) — the skill
metadata appears in the system prompt and full instructions are loaded on
demand via get_skill_instructions. `instructions=` is a short role description
only; the full guidance lives in skills/activity/pm-worklog-synth/SKILL.md.

Lazy imports of agno keep this module importable without the optional dependency.
"""
from __future__ import annotations

import logging
from pathlib import Path
from typing import TYPE_CHECKING

from agents.pm_worklog_update import config
from agents.pm_worklog_update.hooks import (
    ProjectSecretGuard,
    SessionBundleSizeGuard,
    time_spent_sanity_check,
)
from agents.pm_worklog_update.models import JiraUpdate

if TYPE_CHECKING:
    from agno.agent import Agent

log = logging.getLogger(__name__)

_SYNTH_NAME = "pm_worklog_synthesiser"

# Path to the skill folder — loaded via agno LocalSkills, not manually read.
_SKILL_DIR = Path(__file__).parent.parent.parent / "skills" / "activity" / "pm-worklog-synth"


def _local_model(*, max_tokens: int, temperature: float):
    """Build an OpenAILike pointing at the local MLX server."""
    from agno.models.openai.like import OpenAILike
    return OpenAILike(
        id=config.MLX_SERVER_MODEL,
        base_url=f"http://{config.MLX_SERVER_HOST}:{config.MLX_SERVER_PORT}/v1",
        api_key="local",
        max_tokens=max_tokens,
        temperature=temperature,
        request_params={"timeout": config.PM_WORKLOG_REQUEST_TIMEOUT_S},
    )


def build_synth_agent(
    *, db, debug_mode: bool = False, debug_level: int = 1,
) -> "Agent":
    """Build the Synthesise agent using agno Skills for lazy skill loading.

    The skill (pm-worklog-synth) is registered via `skills=` — agno adds its
    metadata to the system prompt and gives the agent a get_skill_instructions
    tool to fetch the full guidance when it needs it. `instructions=` is kept
    as a short role line only.

    Args:
        db: An `agno.db.sqlite.SqliteDb` pointing at meridian.db.
        learning: A `LearningMachine` instance from `learning.py`.
        debug_mode: Forward to the underlying `Agent`.
        debug_level: 1 or 2 (verbose).
    """
    from agno.agent import Agent
    from agno.guardrails import PIIDetectionGuardrail
    from agno.skills import LocalSkills, Skills

    return Agent(
        name=_SYNTH_NAME,
        debug_mode=debug_mode,
        debug_level=debug_level,
        description=(
            "Verifies session summaries belong to a Jira ticket and "
            "writes a 2-4 line worklog comment."
        ),
        model=_local_model(
            max_tokens=config.PM_WORKLOG_SYNTH_MAX_TOKENS,
            temperature=config.PM_WORKLOG_TEMP_SYNTH,
        ),
        db=db,
        # learning=learning,  # disabled — adds 140s entity extraction call per run
        # Short role description only — full guidance is in the skill.
        instructions=[
            "You are Meridian's worklog writer.",
            "Use the pm-worklog-synth skill for full instructions on how to verify sessions and write the worklog.",
        ],
        # Skill loaded via agno — metadata in system prompt, full content on demand.
        skills=Skills(loaders=[LocalSkills(str(_SKILL_DIR))]),
        # Tools removed for initial testing — add back selectively once
        # the one-shot flow is validated.
        # tools=[
        #     tools.get_session_evidence,
        #     tools.check_pm_task_status,
        #     tools.get_earlier_today_summaries,
        # ],
        pre_hooks=[
            ProjectSecretGuard(),
            SessionBundleSizeGuard(max_tokens=80_000),
        ],
        post_hooks=[
            PIIDetectionGuardrail(),
            time_spent_sanity_check,
        ],
        output_schema=JiraUpdate,
        use_json_mode=True,
        add_datetime_to_context=True,
        add_history_to_context=False,
        markdown=False,
    )


