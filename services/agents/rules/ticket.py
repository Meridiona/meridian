"""Ticket-key extraction.

This is the Stage-1 candidate for `ticket_links`. We don't write a hit into
`session_dimensions` for the matched ticket itself — that goes to
`ticket_links` (the canonical session→task store). Instead, we expose a
helper `extract_candidate_tickets` that the tagger orchestrator calls
directly. We do, however, emit a `topic` hit for each matched ticket so the
multi-label store reflects what the user touched.
"""
from __future__ import annotations

from agents.rules import RuleHit, rule, extract_tickets


@rule(name="ticket_keys_in_text", dim="topic")
def _ticket_keys_as_topics(session: dict):
    """Add every ticket-key mention as a topic tag (e.g. 'KAN-86').

    The actual ticket_links decision is made by the tagger after consulting
    `pm_tasks`; this just records that the ticket key was *seen*.
    """
    keys = extract_tickets(session)
    if not keys:
        return None
    return [
        RuleHit(
            dimension="topic",
            value=k,
            confidence=0.95,
            explanation=f"verbatim ticket key in OCR/title",
        )
        for k in keys
    ]
