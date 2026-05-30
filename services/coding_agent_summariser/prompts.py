"""Shared summary contract — the rules, the schema, and limit detection.

Lives in one place so the three engines stay consistent:
  * Claude loads the `session-summary` skill (same rules, authored in SKILL.md).
  * Codex and MLX get `SUMMARY_INSTRUCTION` as their prompt / system message.
All three target `SUMMARY_SCHEMA`.
"""
from __future__ import annotations

from typing import Optional

# The shared rules, without an output-format clause (engines append their own).
SUMMARY_RULES = (
    "You summarise ONE work-burst of a developer's coding-agent session for a "
    "Jira work-log. The transcript (provided on stdin) is timestamped as "
    "`[<ISO ts>] [role] <message>`. Write a factual prose summary of 10-40 "
    "sentences: name the files edited, commands run, errors hit, decisions "
    "made, tests/validations performed, and any rework or blockers (an approach "
    "abandoned, a failed build/test, something deleted and rebuilt). State ONLY "
    "what is in the transcript — never invent files, tickets, commands, or "
    "outcomes. No preamble, no markdown headings, no bullet lists — just clear "
    "paragraphs. If an 'EARLIER IN THIS SESSION' section is present, do not "
    "repeat it; summarise only this burst."
)

# For schema-enforced engines (Codex `--output-schema`): ask for JSON.
SUMMARY_INSTRUCTION = (
    SUMMARY_RULES + " Return JSON matching the schema: `summary` (the prose) "
    "and `blockers` (a list of distinct blockers / failures / rework, possibly empty)."
)

# For the MLX fallback (no schema enforcement, and the local model is a reasoner):
# demand ONLY the prose, no thinking/JSON, so it doesn't leak its reasoning.
MLX_PROSE_INSTRUCTION = (
    SUMMARY_RULES + " Output ONLY the final prose summary itself — do NOT show "
    "your reasoning or thinking, do NOT wrap it in JSON, do NOT add any preamble "
    "or labels. Begin directly with the first sentence of the summary."
)

# Structured-output contract. `summary` is the prose we store; `blockers`
# forces the model to surface rework/failures (the bit weaker models drop).
SUMMARY_SCHEMA = {
    "type": "object",
    "properties": {
        "summary":  {"type": "string", "minLength": 1},
        "blockers": {"type": "array", "items": {"type": "string"}},
    },
    "required": ["summary"],
}

# Substrings that mark a subscription usage/rate limit in CLI stderr/output —
# for Claude (`claude -p`) and Codex (`codex exec`) alike.
RATE_LIMIT_MARKERS = (
    "usage limit",
    "rate limit",
    "rate_limit",
    "429",
    "overloaded",
    "exceeded your",
    "quota",
    "resets at",
    "try again at",
    "upgrade to plus",
)


def looks_rate_limited(text: str) -> bool:
    low = (text or "").lower()
    return any(m in low for m in RATE_LIMIT_MARKERS)


def first_line(text: Optional[str]) -> str:
    if not text:
        return ""
    for line in text.splitlines():
        line = line.strip()
        if line:
            return line[:200]
    return ""
