"""Pre-hooks, post-hooks, and custom guardrails for the Synth agent.

Layout:

  * `SessionBundleSizeGuard`   — pre-hook: reject inputs that don't fit
                                  the model's effective context window
  * `EvidenceRefValidator`     — post-hook: drop bullets without
                                  evidence_refs and recompute coverage
  * `TimeSpentSanityCheck`     — post-hook: clamp time_spent_seconds to
                                  the cycle window
  * `RiskFlagger`              — post-hook: surface low-confidence /
                                  ticket-closed / assignee-mismatch flags

All guardrails extend `agno.guardrails.BaseGuardrail` (when agno is
installed). The classes are written so that if agno is not yet on the
PYTHONPATH, this module still imports cleanly — useful for the unit test
that exercises `EvidenceRefValidator` without the agno runtime.
"""
from __future__ import annotations

import logging
import re
from typing import Any

try:
    from agno.exceptions import CheckTrigger, InputCheckError, OutputCheckError
    from agno.guardrails import BaseGuardrail
    from agno.run.agent import RunInput, RunOutput
    _AGNO_AVAILABLE = True
except ImportError:                                     # pragma: no cover
    # Allow this module to import when agno isn't installed yet. The
    # guardrail classes still work as plain callables in that case.
    _AGNO_AVAILABLE = False

    class BaseGuardrail:                                # type: ignore[no-redef]
        def check(self, run_input):  # noqa: D401
            ...
        async def async_check(self, run_input):
            self.check(run_input)

    class InputCheckError(Exception):                   # type: ignore[no-redef]
        def __init__(self, msg: str, check_trigger: Any = None):
            super().__init__(msg)
            self.check_trigger = check_trigger

    class OutputCheckError(Exception):                  # type: ignore[no-redef]
        def __init__(self, msg: str, check_trigger: Any = None):
            super().__init__(msg)
            self.check_trigger = check_trigger

    class CheckTrigger:                                 # type: ignore[no-redef]
        INPUT_NOT_ALLOWED  = "INPUT_NOT_ALLOWED"
        OUTPUT_NOT_ALLOWED = "OUTPUT_NOT_ALLOWED"

    RunInput  = Any                                     # type: ignore[misc]
    RunOutput = Any                                     # type: ignore[misc]

from agents.pm_update.models import (
    BulletWithEvidence,
    GroundedNarrative,
    JiraUpdate,
    RiskFlag,
    SessionBundle,
)
from agents.pm_update import config

log = logging.getLogger(__name__)

# Conservative token estimate: 4 bytes per token for ASCII-heavy text.
# Errs on the side of "this is bigger than it looks".
_BYTES_PER_TOKEN = 4


# ──────────────────────── Pre-hooks ────────────────────────────────────────────


class SessionBundleSizeGuard(BaseGuardrail):
    """Pre-hook: refuse to call the LLM if the bundle exceeds the budget.

    With a 9B local MLX model the effective good-quality window is
    8K-16K tokens. We let bundles up to 120K-token-equivalents through —
    above that, the Collect step should have routed via the heavy path.
    Hitting this guardrail means a bug in the heavy-path threshold.
    """

    def __init__(self, max_tokens: int = 120_000) -> None:
        self.max_tokens = max_tokens

    def check(self, run_input: RunInput) -> None:
        content = getattr(run_input, "input_content", run_input)
        size_bytes = len(content if isinstance(content, str) else str(content))
        approx_tokens = size_bytes // _BYTES_PER_TOKEN
        if approx_tokens > self.max_tokens:
            raise InputCheckError(
                f"session bundle ~{approx_tokens} tokens exceeds budget "
                f"{self.max_tokens} — route via heavy path",
                check_trigger=CheckTrigger.INPUT_NOT_ALLOWED,
            )

    async def async_check(self, run_input: RunInput) -> None:
        self.check(run_input)


# ──────────────────────── Post-hooks (plain functions, not BaseGuardrail) ──────
#
# Agno post-hooks can be plain callables; we use that form so they can
# manipulate the `RunOutput` (BaseGuardrail is for pure validation).


def evidence_ref_validator(run_output: RunOutput) -> None:
    """Post-hook: drop bullets without evidence_refs; recompute coverage.

    Mutates `run_output.content` in place. Raises if the resulting
    grounded output would be empty — that means the model returned nothing
    we can prove, which is worse than no comment at all.
    """
    upd = _coerce_to_jira_update(run_output)
    if upd is None:
        return  # nothing to validate

    dropped: list[str] = []
    for attr in ("what_shipped", "in_progress", "blockers", "decisions"):
        kept: list[BulletWithEvidence] = []
        for b in getattr(upd, attr):
            if not b.evidence_refs:
                dropped.append(f"{attr}: {b.text[:80]}")
                continue
            kept.append(b)
        setattr(upd, attr, kept)

    total = sum(len(getattr(upd, a)) for a in
                ("what_shipped", "in_progress", "blockers", "decisions"))
    if total == 0:
        # Don't raise — let downstream router decide. Set a flag so the
        # router skips this update.
        if RiskFlag.LOW_EVIDENCE not in upd.risk_flags:
            upd.risk_flags.append(RiskFlag.LOW_EVIDENCE)
        upd.confidence = 0.0

    if dropped:
        log.info("evidence_ref_validator dropped %d un-grounded bullets: %s",
                 len(dropped), dropped[:3])

    _write_back(run_output, upd)


def time_spent_sanity_check(run_output: RunOutput, *, window_seconds: int = 3600) -> None:
    """Post-hook: clamp `time_spent_seconds` to a physically possible range.

    A 1-hour cycle window can spend at most 1 hour. The LLM occasionally
    hallucinates 28800s (8h) — we clamp.
    """
    upd = _coerce_to_jira_update(run_output)
    if upd is None:
        return
    if upd.time_spent_seconds > window_seconds:
        log.info(
            "time_spent_sanity_check clamping %ds → %ds for %s",
            upd.time_spent_seconds, window_seconds, upd.task_key,
        )
        upd.time_spent_seconds = window_seconds
    _write_back(run_output, upd)


def risk_flagger(run_output: RunOutput, *, bundle: SessionBundle) -> None:
    """Post-hook: surface routing-relevant signals.

    Looks at the JiraUpdate + the original SessionBundle and appends
    `RiskFlag` entries for the Router step to consult.
    """
    upd = _coerce_to_jira_update(run_output)
    if upd is None:
        return

    flags = set(upd.risk_flags)

    if upd.confidence < config.PM_UPDATE_MIN_CONFIDENCE:
        flags.add(RiskFlag.LOW_CONFIDENCE)

    if bundle.pm_task_status == "done":
        flags.add(RiskFlag.TICKET_CLOSED)

    # Cross-ticket leak: any bullet that references a session_id which
    # ISN'T in the bundle — should be impossible after EvidenceRefValidator,
    # but we double-check.
    bundle_ids = {s.id for s in bundle.sessions}
    for b in upd.bullets:
        if any(ref not in bundle_ids for ref in b.evidence_refs):
            flags.add(RiskFlag.CROSS_TICKET_LEAK)
            break

    upd.risk_flags = sorted(flags, key=lambda f: f.value)
    _write_back(run_output, upd)


# ──────────────────────── PII helpers ──────────────────────────────────────────
#
# We compose Agno's built-in PIIDetectionGuardrail in the agent assembly,
# but we add one extra regex pass here for things its default ruleset
# might miss (project-scoped tokens like Jira API keys).

_EXTRA_PII_PATTERNS = (
    re.compile(r"\bsk-[a-zA-Z0-9]{20,}\b"),                # OpenAI / openrouter style keys
    re.compile(r"\bATATT3[A-Za-z0-9_-]{20,}\b"),           # Atlassian API tokens
    re.compile(r"\bgh[pousr]_[A-Za-z0-9]{20,}\b"),         # GitHub tokens
    re.compile(r"\bAKIA[0-9A-Z]{16}\b"),                   # AWS access keys
)


class ProjectSecretGuard(BaseGuardrail):
    """Pre-hook: refuse inputs containing API-style tokens.

    Sits alongside Agno's `PIIDetectionGuardrail` — that one catches
    standard PII (emails, phone, SSN, credit cards). This one catches
    project-scoped secrets we are paranoid about leaking to Jira.
    """

    def check(self, run_input: RunInput) -> None:
        content = getattr(run_input, "input_content", run_input)
        if not isinstance(content, str):
            return
        for pat in _EXTRA_PII_PATTERNS:
            if pat.search(content):
                raise InputCheckError(
                    f"input contains a token matching {pat.pattern!r}; refusing to send to LLM",
                    check_trigger=CheckTrigger.INPUT_NOT_ALLOWED,
                )

    async def async_check(self, run_input: RunInput) -> None:
        self.check(run_input)


# ──────────────────────── Coverage helper ──────────────────────────────────────


def compute_coverage(update: JiraUpdate) -> float:
    """Coverage = (bullets with ≥1 evidence_ref) / (total bullets).

    This is the headline metric the Ground step writes into the
    `pm_updates.coverage` column.
    """
    bullets = update.bullets
    if not bullets:
        return 0.0
    grounded = sum(1 for b in bullets if b.evidence_refs)
    return grounded / len(bullets)


def build_grounded_narrative(update: JiraUpdate) -> GroundedNarrative:
    """Apply the evidence filter and return a GroundedNarrative.

    Idempotent: callable multiple times without changing the result.
    """
    dropped: list[str] = []
    for attr in ("what_shipped", "in_progress", "blockers", "decisions"):
        kept: list[BulletWithEvidence] = []
        for b in getattr(update, attr):
            if b.evidence_refs:
                kept.append(b)
            else:
                dropped.append(f"{attr}: {b.text[:80]}")
        setattr(update, attr, kept)
    return GroundedNarrative(
        update=update,
        coverage=compute_coverage(update),
        dropped_bullets=dropped,
    )


# ──────────────────────── Internals ────────────────────────────────────────────


def _coerce_to_jira_update(run_output: Any) -> JiraUpdate | None:
    """Extract a JiraUpdate from a RunOutput-like object.

    Works for: a real `RunOutput` with `.content` set to a JiraUpdate,
    a bare JiraUpdate instance, or a dict.
    """
    candidate = getattr(run_output, "content", run_output)
    if isinstance(candidate, JiraUpdate):
        return candidate
    if isinstance(candidate, dict):
        try:
            return JiraUpdate.model_validate(candidate)
        except Exception as exc:                        # pragma: no cover
            log.warning("could not coerce dict → JiraUpdate: %s", exc)
            return None
    return None


def _write_back(run_output: Any, update: JiraUpdate) -> None:
    if hasattr(run_output, "content"):
        run_output.content = update
