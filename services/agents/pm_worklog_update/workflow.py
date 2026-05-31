"""Synth helper for the Rust pm-worklog stage.

The full PM-worklog pipeline (collect → synth → ground → post) now lives in
the Rust daemon (`src/pm_worklog/`). This module is reduced to a thin synth
helper that the MLX server's `/synthesise_worklog` endpoint calls:

  * `_render_workflow_input(bundle)` — build the agent prompt (user message)
    from a `SessionBundle`.
  * `_coerce_jira(raw)` — parse the agent's raw output back into a `JiraUpdate`.

The agno-Workflow orchestration, the CLI, and the Jira poster that used to
live here were removed once Rust took ownership of the pipeline.
"""
from __future__ import annotations

import logging

from agents.pm_worklog_update import config
from agents.pm_worklog_update.models import (
    GroundedNarrative,
    JiraUpdate,
    SessionBundle,
    UpdateState,
)

log = logging.getLogger(__name__)


# ──────────────────────── Helpers ──────────────────────────────────────────────


def _render_workflow_input(bundle: SessionBundle) -> str:
    """Compose the user-message blob fed to the Synthesise agent."""
    lines: list[str] = []

    # Ticket context — model uses this to verify sessions belong here.
    lines.append(f"# TICKET: {bundle.task_key}")
    if bundle.pm_task_title:
        lines.append(f"title: {bundle.pm_task_title}")
    if bundle.pm_task_status:
        lines.append(f"status: {bundle.pm_task_status}")
    if bundle.assignee_name:
        lines.append(f"assignee: {bundle.assignee_name}")
    if bundle.pm_task_description:
        lines.append(f"description: {bundle.pm_task_description}")
    lines.append("")

    lines.append(f"# WINDOW: {bundle.window_start} → {bundle.window_end}")
    lines.append(f"real_seconds: {bundle.real_seconds}")
    lines.append("")

    if bundle.earlier_today_summaries:
        lines.append("# EARLIER TODAY (already logged — do not repeat)")
        for s in bundle.earlier_today_summaries:
            lines.append(f"- {s}")
        lines.append("")

    # Session summaries — model must verify each belongs to this ticket.
    lines.append(f"# SESSION SUMMARIES ({len(bundle.sessions)})")
    lines.append("Each summary was classified to this ticket by an upstream model.")
    lines.append("Verify each one actually relates to the ticket before including it.")
    lines.append("")
    for s in bundle.sessions:
        lines.append(f"## session {s.id} — {s.app_name} — {s.duration_s}s")
        if s.top_titles:
            lines.append(f"windows: {', '.join(s.top_titles)}")
        if s.excerpt:
            lines.append(s.excerpt)
        lines.append("")

    return "\n".join(lines)


def _extract_jira_update(step_input) -> JiraUpdate | None:
    """Pull the JiraUpdate produced by the Synthesise step out of step_input."""
    raw = step_input.get_step_content("synthesise") or step_input.previous_step_content
    return _coerce_jira(raw)


def _extract_grounded(step_input) -> GroundedNarrative | None:
    raw = step_input.get_step_content("ground") or step_input.previous_step_content
    if isinstance(raw, GroundedNarrative):
        return raw
    if isinstance(raw, str):
        try:
            return GroundedNarrative.model_validate_json(raw)
        except Exception:
            return None
    if isinstance(raw, dict):
        try:
            return GroundedNarrative.model_validate(raw)
        except Exception:
            return None
    return None


def _coerce_jira(raw) -> JiraUpdate | None:
    if isinstance(raw, JiraUpdate):
        return raw
    if isinstance(raw, str):
        try:
            return JiraUpdate.model_validate_json(raw)
        except Exception:
            return None
    if isinstance(raw, dict):
        try:
            return JiraUpdate.model_validate(raw)
        except Exception:
            return None
    return None


def _decide_state(update: JiraUpdate, coverage: float, bundle: SessionBundle) -> UpdateState:
    """Apply the routing matrix that picks the final UpdateState."""
    if not update.summary.strip():
        return UpdateState.SKIPPED
    if update.confidence < config.PM_WORKLOG_MIN_CONFIDENCE:
        return UpdateState.DRAFTED
    return UpdateState.DRAFTED


def _state_reason(state: UpdateState, update: JiraUpdate, coverage: float) -> str:
    if state == UpdateState.SKIPPED:
        return "no worklog summary produced"
    if state == UpdateState.DRAFTED:
        return f"drafted (confidence={update.confidence:.2f})"
    return state.value
