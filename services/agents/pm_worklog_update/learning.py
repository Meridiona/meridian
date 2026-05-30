"""Self-learning configuration for the pm_worklog_update workflow.

Agno exposes a `LearningMachine` that wires up five complementary stores:

  * **User Profile**     — structured facts about the human user
  * **User Memory**      — unstructured observations from runs
  * **Session Context**  — within-run goals/progress
  * **Entity Memory**    — facts about external entities (here: tickets)
  * **Learned Knowledge** — cross-run insights to transfer between users
  * **Decision Log**     — every routing/posting decision + outcome

For Meridian, the entities we want to remember are **Jira tickets** —
not "users" in the conversational sense. So we use `user_id=task_key`
as the identity key: each ticket gets its own memory pile.

This gives the agent:
  - **Entity Memory** for KAN-64: themes, files, blockers seen
  - **Decision Log** of every cycle: what was posted, was it edited,
    did the routing gate fire
  - **Learned Knowledge** in Propose mode: the agent suggests
    generalisations ("user prefers terse bullets") that we accept
    explicitly via the feedback table

Combined with the `pm_worklog_feedback` table (read by the
`get_feedback_examples` tool), this closes the loop: every admin edit
becomes a few-shot hint for the next cycle.
"""
from __future__ import annotations

import logging
from typing import TYPE_CHECKING

from agents.pm_worklog_update import config

if TYPE_CHECKING:
    from agno.learn import LearningMachine

log = logging.getLogger(__name__)


def build_learning_machine() -> "LearningMachine":
    """Return the LearningMachine instance wired for PM updates.

    The defaults are deliberately conservative:
      * `user_profile=False`  — we have one human user, no need to
        extract structured facts about them
      * `user_memory=True` (ALWAYS) — accumulate ticket-scoped notes
      * `entity_memory=True` (ALWAYS) — per-ticket facts
      * `learned_knowledge=True` (AGENTIC) — agent saves
        generalisations on its own; cheap because we cap tool calls
      * `decision_log=True` (AGENTIC) — only logged when the agent
        decides a decision is worth recording, avoiding spam

    Returns:
        A configured `LearningMachine` ready to pass to `Agent(learning=)`.
    """
    from agno.learn import (
        DecisionLogConfig,
        EntityMemoryConfig,
        LearnedKnowledgeConfig,
        LearningMachine,
        LearningMode,
        UserMemoryConfig,
    )

    return LearningMachine(
        user_profile=False,
        user_memory=UserMemoryConfig(mode=LearningMode.ALWAYS),
        entity_memory=EntityMemoryConfig(mode=LearningMode.ALWAYS),
        learned_knowledge=LearnedKnowledgeConfig(mode=LearningMode.AGENTIC),
        decision_log=DecisionLogConfig(mode=LearningMode.AGENTIC),
    )


def prune_old_memories(*, max_age_days: int = 90) -> None:
    """Call from a weekly cron to keep the memory store healthy.

    Memory growth is the #1 cost trap agno calls out in their
    production-best-practices doc — at 200+ memories per entity the
    per-LLM-call cost balloons.
    """
    from agno.db.sqlite import SqliteDb
    from agno.learn import LearningMachine

    db = SqliteDb(db_file=str(config.MERIDIAN_DB))
    lm = LearningMachine(db=db)  # type: ignore[call-arg]
    lm.curator.prune(max_age_days=max_age_days)
    lm.curator.deduplicate()
    log.info("pruned pm_update memories older than %d days", max_age_days)
