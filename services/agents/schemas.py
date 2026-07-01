"""Output schemas for the FSM-constrained JSON endpoints.

One Pydantic model per structured LLM call (classify / generate_worklog /
propose_ticket). `outlines` compiles each into a finite-state logits processor, so
the model can only emit tokens that keep the JSON valid against the schema.

Every free-text field carries an explicit ``max_length`` and every list a
``max_length`` (maxItems): these bounds are not cosmetic — they are what make FSM
safe. Without them a string/array field can decode legal tokens right up to the
generation's ``max_tokens`` and never reach its closing quote/bracket, producing a
truncated (still-unparseable) object. The bounds force the field to close.

# Who uses this
``routes/classify.py`` → :class:`ClassifyOut`; ``routes/generate.py`` →
:class:`WorklogOut`, :class:`ProposeOut`. Consumed via
``agents.structured.generate_structured(output_type=...)``.
"""
from __future__ import annotations

from typing import Literal

from pydantic import BaseModel, Field


# ── /classify_tasks ───────────────────────────────────────────────────────────

class ClassifyMatch(BaseModel):
    """One ticket the hour's activity maps to."""
    task_key:   str   = Field(max_length=32)
    # confidence is intentionally UNBOUNDED here: outlines/FSM decoding cannot enforce a
    # numeric ge/le range, so a `ge=0/le=1` constraint would just make the model's
    # occasional out-of-range value (e.g. 100) fail Pydantic validation and drop the WHOLE
    # object. It is clamped to [0,1] in the route handler instead. (See models.py.)
    confidence: float
    why:        str   = Field(max_length=240, description="one line on the concrete work")


class ClassifyOut(BaseModel):
    """Classifier output: a short reasoning preamble, then zero or more matches.

    ``reasoning`` is the bounded in-JSON scratch space that replaces the native
    <think> block. ``matches`` may be empty (nothing matched)."""
    reasoning: str                = Field(max_length=800)
    matches:   list[ClassifyMatch] = Field(default_factory=list, max_length=12)


# ── /generate_worklog ─────────────────────────────────────────────────────────

class WorklogOut(BaseModel):
    """A drafted worklog comment for one ticket."""
    summary:      str       = Field(max_length=800, description="2-4 sentence worklog comment")
    what_shipped: list[str] = Field(default_factory=list, max_length=8)
    decisions:    list[str] = Field(default_factory=list, max_length=6)
    # Unbounded for the same reason as ClassifyMatch.confidence — clamped in the route.
    confidence:   float     = 0.0


# ── /propose_ticket ───────────────────────────────────────────────────────────

class ProposeOut(BaseModel):
    """Decision + draft for a brand-new ticket covering the hour's residual work."""
    reasoning:      str               = Field(max_length=600)
    should_propose: bool              = False
    issue_type:     Literal["Task", "Bug"] = "Task"
    title:          str               = Field(default="", max_length=80)
    description:    str               = Field(default="", max_length=600)
