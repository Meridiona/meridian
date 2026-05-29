"""Function tools exposed to the Synth agent.

agno introspects each function's name, signature and docstring to build
the tool spec the LLM sees. So the docstrings here matter — they're the
LLM-facing documentation. Keep them imperative and specific.

We deliberately do NOT expose raw SQL to the model. Every tool below is a
typed, narrow accessor. If we ever want richer querying, we'll add more
narrow tools — never a generic SQL endpoint, since "replace the dev's PM
work" means the surface area must stay auditable.
"""
from __future__ import annotations

import json
import logging
from datetime import datetime, timedelta, timezone
from pathlib import Path
from typing import Any

from agents.pm_update import db
from agents.pm_update.config import MERIDIAN_DB

log = logging.getLogger(__name__)

# Cap evidence excerpts shown to the model so a 50KB OCR blob doesn't
# blow past a single tool-result token budget.
EVIDENCE_CHAR_CAP = 4_000


def get_session_evidence(session_id: int, max_chars: int = EVIDENCE_CHAR_CAP) -> str:
    """Return the raw OCR/text excerpt for one session as proof of activity.

    Use this when you need more detail than the 2KB excerpt in the digest —
    for example, to verify whether a specific file was edited or to quote
    a commit message. The session_id MUST appear in the bundle you were
    given; using it outside the bundle is a hallucination.

    Args:
        session_id: The `app_sessions.id` to fetch.
        max_chars: Cap on returned text size (default 4000).

    Returns:
        The session_text, truncated to `max_chars`. Empty string if the
        session has no captured text.
    """
    text = db.fetch_session_text(session_id, db_path=MERIDIAN_DB)
    if len(text) > max_chars:
        return text[:max_chars] + "\n…[truncated]"
    return text


def check_pm_task_status(task_key: str) -> dict[str, Any]:
    """Look up the current cached Jira state of a ticket.

    Use this BEFORE proposing a status transition or writing a comment
    that depends on the ticket's lifecycle phase. Returns a dict with:
        status_category : 'todo' | 'in_progress' | 'done'
        issue_type      : 'Story' | 'Bug' | 'Task' | …
        title           : human-readable summary
        epic_title      : parent epic if any
        sprint_name     : active sprint if any
        assignee_name   : current assignee
        fetched_at      : when meridian last refreshed this row

    If the cache is older than 30 minutes, treat the data as stale and
    say so in your `reasoning` field.

    Args:
        task_key: Jira ticket key, e.g. "KAN-64".

    Returns:
        Dict of ticket fields, or {"error": "not_found"} if the ticket is
        not in the local cache.
    """
    row = db.fetch_pm_task(task_key, db_path=MERIDIAN_DB)
    if row is None:
        return {"error": "not_found", "task_key": task_key}
    # Drop the description_text — it can be huge and is rarely useful in
    # a tool result (we already have it in the bundle context).
    row.pop("description_text", None)
    if row.get("fetched_at"):
        try:
            fetched = datetime.fromisoformat(row["fetched_at"].replace("Z", "+00:00"))
            age = datetime.now(timezone.utc) - fetched
            row["cache_age_minutes"] = int(age.total_seconds() // 60)
            row["cache_is_stale"] = age > timedelta(minutes=30)
        except (ValueError, TypeError):
            pass
    return row


def get_earlier_today_summaries(task_key: str) -> list[str]:
    """Headlines of earlier posts on this ticket today (oldest first).

    Use this to avoid repeating yourself across the day's cycles. If you
    see "Wired migration 022" in this list, you don't need to say that
    again in cycle 3 — focus on what's new.

    Args:
        task_key: Jira ticket key.

    Returns:
        Ordered list of summary headlines, oldest first. Empty list if
        this is the first cycle of the day.
    """
    from datetime import datetime as _dt
    return db._fetch_earlier_today_summaries(
        task_key, _dt.now(timezone.utc), db_path=MERIDIAN_DB
    )


def get_feedback_examples(task_key: str, limit: int = 3) -> list[dict[str, str]]:
    """Recent admin edits/rejections — use them to match the user's style.

    Each item is `{kind, original, edited, note}`. The Synth agent should
    treat `edited` versions as positive examples (this is how the user
    wants comments worded) and `reject` entries as negative examples
    (avoid this shape).

    Args:
        task_key: Jira ticket key.
        limit: Max number of items.

    Returns:
        List ordered newest first.
    """
    rows = db.fetch_recent_feedback(task_key, limit=limit, db_path=MERIDIAN_DB)
    examples: list[dict[str, str]] = []
    for r in rows:
        examples.append({
            "kind":     r["feedback_kind"],
            "original": (r.get("original_text") or "")[:600],
            "edited":   (r.get("edited_text") or "")[:600],
            "note":     (r.get("note") or "")[:200],
        })
    return examples


def render_jira_comment_markdown(
    summary: str,
    what_shipped: list[str],
    in_progress: list[str],
    blockers: list[str],
    decisions: list[str],
    next_steps: list[str],
    time_spent_seconds: int,
) -> str:
    """Render the final Jira-flavoured Markdown comment.

    This is a deterministic Python function — not an LLM call — because
    the structure of the comment must be stable across cycles. The Synth
    agent fills the `JiraUpdate` model; this helper serialises it.

    Args:
        summary: ≤80-char headline.
        what_shipped: Bullets for completed work.
        in_progress: Bullets for ongoing work.
        blockers: Bullets for current blockers.
        decisions: Bullets for design/tech decisions.
        next_steps: Short list of next actions.
        time_spent_seconds: For the "time spent" footer.

    Returns:
        Markdown string ready to POST to Jira.
    """
    def _bullets(items: list[str]) -> str:
        return "\n".join(f"* {i}" for i in items) if items else "_(none)_"

    minutes = max(1, time_spent_seconds // 60)
    parts: list[str] = [
        f"**{summary}**",
        "",
        f"_Meridian PM update — time spent: ~{minutes} min_",
        "",
        "### What shipped",
        _bullets(what_shipped),
        "",
        "### In progress",
        _bullets(in_progress),
    ]
    if blockers:
        parts += ["", "### Blockers", _bullets(blockers)]
    if decisions:
        parts += ["", "### Decisions", _bullets(decisions)]
    if next_steps:
        parts += ["", "### Next", _bullets(next_steps)]
    return "\n".join(parts)
