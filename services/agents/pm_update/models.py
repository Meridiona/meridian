"""Pydantic models for the pm_update workflow.

These models serve three purposes:

  1. **`output_schema` for agno Agents** — the Synthesise + Compose agents
     emit `JiraUpdate` instances; agno's structured-output path validates
     the LLM response against this schema.
  2. **Step-to-step data flow** — `SessionBundle` and `WorkNarrative` are
     passed between workflow steps via `StepOutput.content`.
  3. **DB row contract** — `JiraUpdate` is what we serialise into the
     `pm_updates.payload_json` column.

The anti-hallucination contract lives here: every `BulletWithEvidence`
must carry at least one `session_id` evidence ref. The `EvidenceRefValidator`
post-hook hard-rejects bullets that violate this.
"""
from __future__ import annotations

from datetime import datetime
from enum import Enum
from typing import Optional

from pydantic import BaseModel, ConfigDict, Field, model_validator


# ──────────────────────── Enums ────────────────────────────────────────────────


class RiskFlag(str, Enum):
    """Risk signals surfaced to the reviewer.

    Each flag means "this update should not auto-post without a human look".
    """
    PII_REDACTED        = "pii_redacted"
    LOW_EVIDENCE        = "low_evidence"
    CROSS_TICKET_LEAK   = "cross_ticket_leak"
    TESTS_FAILING       = "tests_failing"
    LOW_CONFIDENCE      = "low_confidence"
    TICKET_CLOSED       = "ticket_closed_upstream"
    ASSIGNEE_MISMATCH   = "assignee_mismatch"
    STALE_PM_TASK_CACHE = "stale_pm_task_cache"


class UpdateState(str, Enum):
    """Lifecycle state of a row in `pm_updates`."""
    DRAFTED  = "drafted"   # workflow produced the row, not yet routed
    QUEUED   = "queued"    # held for human review
    POSTED   = "posted"    # comment + worklog landed on Jira
    SKIPPED  = "skipped"   # gate rejected (e.g. low work, already-done ticket)
    REJECTED = "rejected"  # admin explicitly declined this update
    FAILED   = "failed"    # post attempt failed; safe to retry


# ──────────────────────── Building blocks ──────────────────────────────────────


class SessionDigest(BaseModel):
    """Compact, LLM-friendly representation of one `app_sessions` row.

    Raw `session_text` (which can be 80KB+) is NOT included by default —
    the digest carries an `excerpt` capped at 2KB. Full text is fetched on
    demand via the `get_session_evidence` tool.
    """
    model_config = ConfigDict(frozen=True)

    id:             int
    app_name:       str
    started_at:     str            # ISO-8601 UTC
    ended_at:       str
    duration_s:     int
    idle_frame_s:   int = 0
    top_titles:     list[str] = Field(default_factory=list, max_length=3)
    dimensions:     dict[str, list[str]] = Field(default_factory=dict)
    excerpt:        str = Field(default="", description="Up to 2KB of session_text")
    category:       Optional[str] = None
    text_source:    Optional[str] = None


class SessionBundle(BaseModel):
    """All sessions for one (task_key, window) — the workflow input.

    `is_heavy` is computed in the Collect step and drives the Condition
    branch downstream.
    """
    model_config = ConfigDict(frozen=True)

    task_key:        str
    window_start:    str
    window_end:      str
    cycle_index:     int = Field(default=0, description="Nth cycle today for this task, 0-indexed")
    sessions:        list[SessionDigest]
    total_seconds:   int
    real_seconds:    int = Field(description="duration minus idle, used for time_spent")
    raw_text_bytes:  int = 0
    is_heavy:        bool = False
    pm_task_status:  Optional[str] = None
    pm_task_title:   Optional[str] = None
    assignee_name:   Optional[str] = None
    earlier_today_summaries: list[str] = Field(default_factory=list)


# ──────────────────────── Output schema (Synthesise → Jira) ────────────────────


class BulletWithEvidence(BaseModel):
    """One factual claim plus the session_ids it is grounded in.

    The `EvidenceRefValidator` post-hook drops any bullet with an empty
    `evidence_refs` list, because every claim must be traceable back to a
    captured session.
    """
    text:           str  = Field(min_length=4, max_length=400)
    evidence_refs:  list[int] = Field(default_factory=list, min_length=1)

    @model_validator(mode="after")
    def _refs_must_be_non_empty(self) -> "BulletWithEvidence":
        if not self.evidence_refs:
            raise ValueError("bullet must reference at least one session_id")
        return self


class JiraUpdate(BaseModel):
    """The contract the Synthesise agent must produce.

    This is what gets persisted to `pm_updates.payload_json` and rendered
    into a Jira comment by the Compose step.
    """
    model_config = ConfigDict(extra="forbid")

    task_key:           str
    window_start:       str
    window_end:         str
    cycle_index:        int = 0
    time_spent_seconds: int = Field(ge=0)

    summary:            str = Field(max_length=80, description="≤80 char headline")
    what_shipped:       list[BulletWithEvidence] = Field(default_factory=list)
    in_progress:        list[BulletWithEvidence] = Field(default_factory=list)
    blockers:           list[BulletWithEvidence] = Field(default_factory=list)
    decisions:          list[BulletWithEvidence] = Field(default_factory=list)
    next_steps:         list[str] = Field(default_factory=list, max_length=5)

    risk_flags:          list[RiskFlag] = Field(default_factory=list)

    confidence:         float = Field(ge=0.0, le=1.0, default=0.0)
    reasoning:          str = Field(default="", description="Synth's brief justification")

    @property
    def bullets(self) -> list[BulletWithEvidence]:
        """All evidence-bearing bullets in deterministic order."""
        return [
            *self.what_shipped,
            *self.in_progress,
            *self.blockers,
            *self.decisions,
        ]


# ──────────────────────── Grounded narrative (post-Ground step) ────────────────


class GroundedNarrative(BaseModel):
    """The `JiraUpdate` after the Ground step has filtered un-evidenced bullets."""
    model_config = ConfigDict(frozen=False)

    update:           JiraUpdate
    coverage:         float = Field(ge=0.0, le=1.0)
    dropped_bullets:  list[str] = Field(default_factory=list)


# ──────────────────────── Routing outcome ──────────────────────────────────────


class RouteOutcome(BaseModel):
    """What the Router step decided. Final workflow output."""
    state:           UpdateState
    pm_update_id:    Optional[int] = None
    posted_comment_id: Optional[str] = None
    reason:          str = ""
    timestamp:       datetime = Field(default_factory=datetime.utcnow)
