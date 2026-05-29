"""Agent definitions for the pm_update workflow.

A single Synthesise agent reads the session bundle, calls reasoning +
evidence-fetching tools, and produces a strict JiraUpdate. With the
local MLX model's 262K context, no chunked / map-reduce path is needed
— even heavy bundles fit comfortably in one call.

Lazy imports of agno keep this module importable without the optional
dependency.
"""
from __future__ import annotations

import logging
from typing import TYPE_CHECKING

from agents.pm_update import config, tools
from agents.pm_update.hooks import (
    ProjectSecretGuard,
    SessionBundleSizeGuard,
    evidence_ref_validator,
    time_spent_sanity_check,
)
from agents.pm_update.models import JiraUpdate

if TYPE_CHECKING:
    from agno.agent import Agent

log = logging.getLogger(__name__)

_SYNTH_NAME = "pm_update_synthesiser"


def _load_skill_prompt(name: str) -> str:
    """Read a skill's SKILL.md from the configured skills paths.

    Re-uses the existing `agents.config.load_skill` helper so we don't
    duplicate path search logic.
    """
    from agents.config import load_skill
    raw = load_skill(name)
    if not raw:
        raise RuntimeError(
            f"skill '{name}' not found in SKILLS_SEARCH_PATHS — "
            "check services/skills/activity/ exists"
        )
    return raw


def _local_model(*, max_tokens: int, temperature: float):
    """Build an OpenAILike pointing at the local MLX server."""
    from agno.models.openai.like import OpenAILike
    return OpenAILike(
        id=config.MLX_SERVER_MODEL,
        base_url=f"http://{config.MLX_SERVER_HOST}:{config.MLX_SERVER_PORT}/v1",
        api_key="local",
        max_tokens=max_tokens,
        temperature=temperature,
        request_params={"timeout": config.PM_UPDATE_REQUEST_TIMEOUT_S},
    )


def build_synth_agent(
    *, db, learning, debug_mode: bool = False, debug_level: int = 1,
) -> "Agent":
    """Build the headline Synthesise agent.

    Arguments are injected so `workflow.py` controls the shared
    `SqliteDb` and `LearningMachine` instances.

    Args:
        db: An `agno.db.sqlite.SqliteDb` pointing at meridian.db.
        learning: A `LearningMachine` instance from `learning.py`.
        debug_mode: Forward to the underlying `Agent` — prints prompts,
            tool calls, and model responses to stderr.
        debug_level: 1 or 2 (verbose).

    Returns:
        Configured `Agent` ready to plug into a Workflow `Step`.
    """
    from agno.agent import Agent
    from agno.guardrails import PIIDetectionGuardrail
    from agno.tools.reasoning import ReasoningTools

    return Agent(
        name=_SYNTH_NAME,
        debug_mode=debug_mode,
        debug_level=debug_level,
        description=(
            "Reads a window of classified work sessions for one Jira "
            "ticket and produces a professional, evidence-grounded "
            "JiraUpdate ready to post as a comment."
        ),
        model=_local_model(
            max_tokens=config.PM_UPDATE_SYNTH_MAX_TOKENS,
            temperature=config.PM_UPDATE_TEMP_SYNTH,
        ),
        db=db,
        learning=learning,
        instructions=_load_skill_prompt("pm-update-synth"),
        tools=[
            ReasoningTools(add_instructions=True),
            tools.get_session_evidence,
            tools.check_pm_task_status,
            tools.get_earlier_today_summaries,
            tools.get_feedback_examples,
        ],
        pre_hooks=[
            PIIDetectionGuardrail(),
            ProjectSecretGuard(),
            SessionBundleSizeGuard(max_tokens=120_000),
        ],
        post_hooks=[
            evidence_ref_validator,
            time_spent_sanity_check,
        ],
        output_schema=JiraUpdate,
        use_json_mode=True,                     # safest for local 9B
        tool_call_limit=12,
        add_datetime_to_context=True,
        add_history_to_context=False,           # workflow session_state replaces this
        markdown=False,                         # we want pure JSON
    )


