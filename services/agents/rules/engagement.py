"""Engagement-dimension rules — single-value: deep_work / focused / context_switching / shallow / idle.

Computed from session shape (`duration_s` and content density). At Stage 1
we don't have neighbouring-session context yet, so we treat duration alone
as the signal. Stage 2/3 can refine using app-switching rate over the ETL
window.
"""
from __future__ import annotations

from agents.rules import RuleHit, rule


@rule(name="duration_engagement", dim="engagement")
def _duration_engagement(session: dict):
    duration = int(session.get("duration_s") or 0)
    titles   = session.get("window_titles") or []
    has_text = 1 if (session.get("session_text") or "").strip() else 0
    audio    = session.get("audio_snippets") or []
    content_score = len(titles) + has_text + len(audio)

    if duration <= 5 and content_score < 2:
        value, conf = "idle", 0.85
        why = f"duration={duration}s, no content"
    elif duration <= 30:
        value, conf = "shallow", 0.75
        why = f"duration={duration}s"
    elif duration <= 90:
        value, conf = "context_switching", 0.7
        why = f"duration={duration}s"
    elif duration <= 600:
        value, conf = "focused", 0.8
        why = f"duration={duration}s"
    else:
        value, conf = "deep_work", 0.9
        why = f"duration={duration}s ({duration / 60:.1f} min)"

    return RuleHit(
        dimension="engagement",
        value=value,
        confidence=conf,
        explanation=why,
    )
