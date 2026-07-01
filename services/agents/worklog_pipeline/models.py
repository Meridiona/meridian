"""Pydantic DTOs for the worklog pipeline — the typed objects passed between stages.

These are the pipeline's INTERNAL data carriers (``ClassificationResult``,
``ProposedTicket``, ``WorklogDraft``), built by ``classifier.py`` / ``generation.py``
from the MLX server's JSON HTTP responses. They are NOT the LLM output schemas —
those live in ``agents.schemas`` (``ClassifyOut`` / ``WorklogOut`` / ``ProposeOut``)
and are FSM-enforced by outlines inside the ``/classify_tasks`` · ``/generate_worklog``
· ``/propose_ticket`` endpoints. An empty ``matches`` list is the valid "no task" answer.
"""
from __future__ import annotations

from pydantic import BaseModel, Field


# NOTE on confidence bounds: outlines/FSM decoding cannot enforce a numeric
# range, so a `ge=0/le=1` constraint on the schema just makes the model's
# occasional out-of-range value (e.g. 1.5) fail Pydantic validation and drop the
# WHOLE object. We keep confidence an unbounded float in every schema and clamp
# to [0,1] in code (classification_keys / build_payload).
def _clamp01(x: float) -> float:
    return max(0.0, min(1.0, float(x)))


class TaskClassification(BaseModel):
    task_key:   str
    confidence: float = 0.0
    why:        str = Field(description="the concrete work that advanced this task")


class ClassificationResult(BaseModel):
    """Generic (non-enum) classification result — used when the candidate set is open."""
    reasoning: str
    matches:   list[TaskClassification] = Field(default_factory=list)


class ProposedTicket(BaseModel):
    """A tier-3 proposed NEW ticket — or an explicit abstention.

    The proposer runs whenever fewer than two existing tickets matched the hour.
    It is told which tickets already matched, then decides one of two things:
      • ``should_propose=False`` — the hour's residual work is NOT worth a PM
        ticket (idle, admin, personal, passive reading, already covered). Every
        other field is then ignored; the caller persists nothing.
      • ``should_propose=True``  — draft a new ticket for the uncovered work.

    This is the pipeline DTO, built field-by-field in ``generation.propose_ticket``
    from the ``/propose_ticket`` HTTP response — field order here is cosmetic and has
    no effect on decoding. The LLM's actual generation order is set by the FSM output
    schema ``agents.schemas.ProposeOut`` (reasoning-first).
    """
    should_propose: bool = Field(
        default=False,
        description="True only when the residual work genuinely warrants a NEW ticket",
    )
    issue_type:  str = Field(
        default="Task",
        description="'Bug' when fixing broken/defective behaviour, else 'Task'",
    )
    title:       str = Field(default="", max_length=80, description="imperative, <=80 chars")
    description: str = Field(default="", description="2-4 sentences of scope and intent")
    reasoning:   str = Field(
        default="", max_length=300,
        description="1-2 sentences: WHY this is a NEW ticket and not existing work",
    )


class WorklogDraft(BaseModel):
    """Narrative-only worklog the LLM generates for ONE matched task.

    The persistence layer stamps the scalars (task_key, window, time_spent) and
    wraps the bullet lists into the JiraUpdate-shaped ``payload_json`` the UI
    reads — so the model never has to invent timestamps or keys. No `reasoning`
    field (see ProposedTicket) — generation steps stay tight to avoid truncation.
    """
    summary:      str = Field(description="2-4 line plain-English worklog comment")
    what_shipped: list[str] = Field(default_factory=list)
    decisions:    list[str] = Field(default_factory=list)
    confidence:   float = 0.0   # clamped to [0,1] at use (see _clamp01)


def classification_keys(result: BaseModel) -> list[tuple[str, float, str]]:
    """Extract (key, confidence, why) tuples from a ClassificationResult,
    clamping each confidence into [0,1]."""
    out: list[tuple[str, float, str]] = []
    for m in result.matches:  # type: ignore[attr-defined]
        out.append((str(m.task_key), _clamp01(m.confidence), m.why))
    return out
