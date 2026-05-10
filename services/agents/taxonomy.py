"""Multi-dimensional tagging taxonomy.

A session can carry many tags. Each tag belongs to a `dimension`. Closed
dimensions have a fixed allowed-value set; open dimensions accept any string.
The DB doesn't enforce the taxonomy — this file is the source of truth, so
we can evolve labels without a migration. Unknown dimension/value pairs go
through with a warning, never rejected.

`SINGLE_VALUE_DIMENSIONS` is the set of dimensions where a session can have
at most one value (e.g. "activity": coding, not coding+meeting). The runner
resolves multiple hits on a single-value dimension by picking the highest
confidence (ties broken by rule order).
"""
from __future__ import annotations

# ─────────────────────────── Closed-vocab dimensions ──────────────────────────
ACTIVITIES: frozenset[str] = frozenset({
    "coding",
    "code_review",
    "debugging",
    "ai_pair_programming",
    "prompt_engineering",
    "learning",
    "planning",
    "design",
    "deployment_devops",
    "documentation",
    "research",
    "communication",
    "meeting",
    "admin",
    "idle_personal",
})

INTENTS: frozenset[str] = frozenset({
    "implementation",
    "exploration",
    "validation",
    "refactor",
    "documentation",
    "communication",
    "learning",
})

ENGAGEMENTS: frozenset[str] = frozenset({
    "deep_work",          # >= 10 min single session, low switching
    "focused",            # 1–10 min, on-task
    "context_switching",  # < 1 min, frequent app flips
    "shallow",            # very short or passive
    "idle",               # no signal
})

PRACTICES: frozenset[str] = frozenset({
    "tests_written",
    "code_review_done",
    "type_checking",
    "error_handling",
    "documentation_updated",
    "refactoring",
    "security_check",
    "peer_review",
    "linting",
    "ci_check",
})

COLLABORATIONS: frozenset[str] = frozenset({
    "solo",
    "ai_assisted",
    "pair_programming",
    "team_review",
})


# ─────────────────────────── Dimension registry ───────────────────────────────
DIMENSIONS: dict[str, frozenset[str] | None] = {
    "activity":      ACTIVITIES,
    "intent":        INTENTS,
    "engagement":    ENGAGEMENTS,
    "collaboration": COLLABORATIONS,
    "practice":      PRACTICES,
    "tool":          None,  # open vocabulary
    "topic":         None,  # open vocabulary
}

SINGLE_VALUE_DIMENSIONS: frozenset[str] = frozenset({
    "activity",
    "intent",
    "engagement",
    "collaboration",
})


def is_known_dimension(name: str) -> bool:
    return name in DIMENSIONS


def is_known_value(dim: str, value: str) -> bool:
    """For closed dimensions, validate the value. Open dimensions always pass."""
    allowed = DIMENSIONS.get(dim)
    return allowed is None or value in allowed
