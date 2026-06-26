"""Worklog generation — one readable draft per matched task.

Schema-enforced (WorklogDraft) so the model emits only the narrative; the db
layer stamps scalars and builds the JiraUpdate-shaped payload the UI reads.
"""
from __future__ import annotations

import logging

from agents.worklog_pipeline.models import WorklogDraft
from agents.worklog_pipeline.prompts.worklog import SYSTEM as WORKLOG_SYSTEM

log = logging.getLogger("meridian.worklog.gen")


def generate_worklog(
    agent,
    report: str,
    distilled_body: str,
    task_key: str,
    task_title: str,
    task_description: str,
    why: str,
) -> WorklogDraft | None:
    """Run the worklog agent (output_schema=WorklogDraft) for one task.

    ``agent`` is a schema-enforced agno Agent (see agent_io.make_schema_agent).
    Returns None if the model output failed to parse (caller skips the draft).
    """
    user = (
        f"TASK: {task_key} — {task_title}\n{task_description}\n\n"
        f"WHY THIS HOUR MATCHED: {why}\n\n"
        f"ACTIVITY SUMMARY (last hour):\n{report}\n\n"
        f"DISTILLED CAPTURE DETAIL (grounding):\n{distilled_body[:8000]}"
    )
    response = agent.run(input=user)
    draft = response.content
    if not isinstance(draft, WorklogDraft):
        log.warning("worklog: output did not parse to WorklogDraft (type=%s) for %s",
                    type(draft).__name__, task_key)
        return None
    return draft
