"""Collaboration-dimension rules — single-value: solo / ai_assisted / pair / team.

The runner picks the highest-confidence hit, so we order rules by specificity:
ai_assisted (Cursor, Claude visible) > pair_programming (live share) > team_review
(meeting + screen share) > solo (default).
"""
from __future__ import annotations

import re

from agents.rules import RuleHit, rule, session_text

PAIR_KEYWORDS_RE = re.compile(
    r"\b(live[- ]share|tuple|code[- ]with[- ]me|pair[- ]programming|"
    r"replit collab|copilot workspace)\b",
    re.I,
)
MEETING_AND_SCREEN_RE = re.compile(
    r"\b(zoom|google meet|microsoft teams|webex|gather)\b.*\b(share screen|screen share|sharing screen)\b",
    re.I,
)
AI_ASSIST_RE = re.compile(
    r"\b(claude|chatgpt|copilot|cursor|gemini|perplexity|llm|"
    r"prompt|assistant\b)",
    re.I,
)


@rule(name="pair_programming_signal", dim="collaboration")
def _pair_programming(session: dict):
    text = session_text(session, ocr_limit=15)
    if PAIR_KEYWORDS_RE.search(text):
        return RuleHit(
            dimension="collaboration",
            value="pair_programming",
            confidence=0.85,
            explanation="pair-programming keyword visible",
        )
    return None


@rule(name="team_review_signal", dim="collaboration")
def _team_review(session: dict):
    text = session_text(session, ocr_limit=15)
    if MEETING_AND_SCREEN_RE.search(text):
        return RuleHit(
            dimension="collaboration",
            value="team_review",
            confidence=0.8,
            explanation="meeting tool with screen share",
        )
    return None


@rule(name="ai_assist_keywords", dim="collaboration")
def _ai_assist_keywords(session: dict):
    """Lower-confidence catch — overridden by activity.cursor_with_chat_panel."""
    if (session.get("app_name") or "") in ("Claude", "ChatGPT", "Cursor", "Gemini"):
        return None  # already handled by activity rules with higher confidence
    text = session_text(session, ocr_limit=15)
    if not AI_ASSIST_RE.search(text):
        return None
    return RuleHit(
        dimension="collaboration",
        value="ai_assisted",
        confidence=0.55,
        explanation="ai-assist keyword in OCR",
    )


@rule(name="solo_default", dim="collaboration")
def _solo_default(session: dict):
    """Last-resort — assigns 'solo' at very low confidence so it loses to
    any ai_assisted / pair / team hit but still tags sessions that have no
    other collaboration evidence.
    """
    return RuleHit(
        dimension="collaboration",
        value="solo",
        confidence=0.3,
        explanation="default — no collaboration signal",
    )
