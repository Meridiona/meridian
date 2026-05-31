# meridian — normalises screenpipe activity into structured app sessions
"""Pydantic models for the pm_worklog_update workflow."""
from __future__ import annotations

from datetime import datetime
from enum import Enum
from typing import Optional

from pydantic import BaseModel, ConfigDict, Field


class RiskFlag(str, Enum):
    PII_REDACTED        = "pii_redacted"
    LOW_EVIDENCE        = "low_evidence"
    CROSS_TICKET_LEAK   = "cross_ticket_leak"
    TESTS_FAILING       = "tests_failing"
    LOW_CONFIDENCE      = "low_confidence"
    TICKET_CLOSED       = "ticket_closed_upstream"
    ASSIGNEE_MISMATCH   = "assignee_mismatch"
    STALE_PM_TASK_CACHE = "stale_pm_task_cache"


class UpdateState(str, Enum):
    DRAFTED  = "drafted"
    QUEUED   = "queued"
    POSTED   = "posted"
    SKIPPED  = "skipped"
    REJECTED = "rejected"
    FAILED   = "failed"


class SessionDigest(BaseModel):
    model_config = ConfigDict(frozen=True)

    id:           int
    app_name:     str
    started_at:   str
    ended_at:     str
    duration_s:   int
    idle_frame_s: int = 0
    top_titles:   list[str] = Field(default_factory=list)
    dimensions:   dict[str, list[str]] = Field(default_factory=dict)
    excerpt:      str = Field(default="")
    category:     Optional[str] = None
    text_source:  Optional[str] = None


class SessionBundle(BaseModel):
    model_config = ConfigDict(frozen=True)

    task_key:            str
    window_start:        str
    window_end:          str
    cycle_index:         int = 0
    sessions:            list[SessionDigest]
    total_seconds:       int
    real_seconds:        int
    raw_text_bytes:      int = 0
    is_heavy:            bool = False
    pm_task_status:      Optional[str] = None
    pm_task_title:       Optional[str] = None
    pm_task_description: Optional[str] = None
    assignee_name:       Optional[str] = None
    earlier_today_summaries: list[str] = Field(default_factory=list)


class BulletWithEvidence(BaseModel):
    text:          str       = Field(min_length=4, max_length=400)
    evidence_refs: list[int] = Field(default_factory=list)


class JiraUpdate(BaseModel):
    model_config = ConfigDict(extra="forbid")

    task_key:           str
    window_start:       str
    window_end:         str
    cycle_index:        int = 0
    time_spent_seconds: int = Field(ge=0)

    # 2-4 line worklog comment — primary output.
    summary:     str = Field(max_length=500, description="2-4 line worklog comment")
    what_shipped: list[BulletWithEvidence] = Field(default_factory=list)
    in_progress:  list[BulletWithEvidence] = Field(default_factory=list)
    blockers:     list[BulletWithEvidence] = Field(default_factory=list)
    decisions:    list[BulletWithEvidence] = Field(default_factory=list)
    next_steps:   list[str] = Field(default_factory=list)

    risk_flags:  list[RiskFlag] = Field(default_factory=list)
    confidence:  float = Field(ge=0.0, le=1.0, default=0.0)
    reasoning:   str = Field(default="")

    @property
    def bullets(self) -> list[BulletWithEvidence]:
        return [*self.what_shipped, *self.in_progress, *self.blockers, *self.decisions]


class GroundedNarrative(BaseModel):
    model_config = ConfigDict(frozen=False)

    update:          JiraUpdate
    coverage:        float = Field(ge=0.0, le=1.0)
    dropped_bullets: list[str] = Field(default_factory=list)


class RouteOutcome(BaseModel):
    state:             UpdateState
    pm_worklog_id:     Optional[int] = None
    posted_comment_id: Optional[str] = None
    reason:            str = ""
    timestamp:         datetime = Field(default_factory=lambda: datetime.now().astimezone())
