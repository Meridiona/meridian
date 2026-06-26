"""Pydantic schemas for the worklog pipeline — the LLM's structured contracts.

Every model declares ``reasoning`` FIRST so greedy decoding makes the model
reason into the JSON before it commits to the decision fields (the proven
SessionClassification pattern, run_task_linker_mlx.py). Output is FSM-enforced
by outlines via the MLX server's /v1 endpoint — captured by schema, never regex.

``build_match_result`` builds the MatchResult schema with ``task_key``
constrained to the exact candidate set for THIS call, so the model literally
cannot emit a key that isn't a candidate. An empty ``matches`` list is the valid,
expected "no task" answer.
"""
from __future__ import annotations

from enum import Enum

from pydantic import BaseModel, Field, create_model


# NOTE on confidence bounds: outlines/FSM decoding cannot enforce a numeric
# range, so a `ge=0/le=1` constraint on the schema just makes the model's
# occasional out-of-range value (e.g. 1.5) fail Pydantic validation and drop the
# WHOLE object. We keep confidence an unbounded float in every schema and clamp
# to [0,1] in code (match_keys / build_payload).
def _clamp01(x: float) -> float:
    return max(0.0, min(1.0, float(x)))


class TaskMatch(BaseModel):
    task_key:   str
    confidence: float = 0.0
    why:        str = Field(description="the concrete work that advanced this task")


class MatchResult(BaseModel):
    """Generic (non-enum) match result — used when the candidate set is open."""
    reasoning: str
    matches:   list[TaskMatch] = Field(default_factory=list)


class ProposedTicket(BaseModel):
    # No `reasoning` field: this is a generation task, not a discrimination one,
    # so reasoning-first adds little and its unbounded length risks truncating
    # the JSON. Keep the output tight so it always parses.
    title:       str = Field(max_length=80, description="imperative, <=80 chars")
    description: str = Field(description="2-4 sentences of scope and intent")


class WorklogDraft(BaseModel):
    """Narrative-only worklog the LLM generates for ONE matched task.

    The persistence layer stamps the scalars (task_key, window, time_spent) and
    wraps the bullet lists into the JiraUpdate-shaped ``payload_json`` the UI
    reads — so the model never has to invent timestamps or keys. No `reasoning`
    field (see ProposedTicket) — generation steps stay tight to avoid truncation.
    """
    summary:      str = Field(description="2-4 line plain-English worklog comment")
    what_shipped: list[str] = Field(default_factory=list)
    in_progress:  list[str] = Field(default_factory=list)
    blockers:     list[str] = Field(default_factory=list)
    decisions:    list[str] = Field(default_factory=list)
    next_steps:   list[str] = Field(default_factory=list)
    confidence:   float = 0.0   # clamped to [0,1] at use (see _clamp01)


def build_match_result(candidate_keys: list[str]) -> type[BaseModel]:
    """Return a MatchResult model whose ``matches[].task_key`` is constrained to
    exactly ``candidate_keys`` (an enum), so the model cannot invent a key.

    Built per call because the candidate set changes per tier/batch.
    """
    # Enum member names must be valid identifiers; values are the real keys.
    key_enum = Enum(  # type: ignore[misc]
        "CandidateKey",
        {k.replace("-", "_"): k for k in candidate_keys},
    )
    bounded_match = create_model(
        "BoundedTaskMatch",
        task_key=(key_enum, ...),
        confidence=(float, 0.0),  # unbounded in schema; clamped in match_keys
        why=(str, Field(description="the concrete work that advanced this task")),
    )
    return create_model(
        "BoundedMatchResult",
        reasoning=(str, ...),
        matches=(list[bounded_match], Field(default_factory=list)),  # type: ignore[valid-type]
    )


def match_keys(result: BaseModel) -> list[tuple[str, float, str]]:
    """Extract (key, confidence, why) tuples from a (possibly enum-bounded)
    MatchResult, normalising enum task_key values back to plain strings.
    """
    out: list[tuple[str, float, str]] = []
    for m in result.matches:  # type: ignore[attr-defined]
        key = m.task_key.value if isinstance(m.task_key, Enum) else str(m.task_key)
        out.append((key, _clamp01(m.confidence), m.why))
    return out
