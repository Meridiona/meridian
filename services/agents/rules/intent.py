"""Intent-dimension rules — single-value, why the user is doing what they're doing.

Mostly inferred from branch names (`feat/`, `fix/`, `refactor/`) and from
ticket-style prefixes in window titles. Falls through to `implementation`
for IDE coding sessions when no other signal is present.
"""
from __future__ import annotations

import re

from agents.rules import RuleHit, rule, extract_branches, session_text

DIFF_VIEW_RE = re.compile(r"\bdiff(?: view)?\b|\bcompare\b", re.I)
DEBUG_KEYWORDS_RE = re.compile(
    r"\b(error|exception|stack trace|traceback|panic|"
    r"failed|undefined is not|cannot read property|null pointer)\b",
    re.I,
)
DOC_FILE_RE = re.compile(r"\b\S+\.(md|mdx|rst|adoc)\b", re.I)


_BRANCH_TO_INTENT = {
    "feat":     ("implementation", 0.9),
    "feature":  ("implementation", 0.9),
    "fix":      ("validation",     0.85),
    "bug":      ("validation",     0.85),
    "refactor": ("refactor",       0.95),
    "docs":     ("documentation",  0.9),
    "doc":      ("documentation",  0.9),
    "test":     ("validation",     0.8),
    "chore":    ("implementation", 0.6),
    "style":    ("refactor",       0.6),
    "perf":     ("refactor",       0.7),
}


@rule(name="branch_name_intent", dim="intent")
def _branch_name(session: dict):
    branches = extract_branches(session)
    if not branches:
        return None
    # Take the most specific (longest) match.
    branches.sort(key=lambda b: -len(b[1]))
    kind, slug = branches[0]
    mapping = _BRANCH_TO_INTENT.get(kind.lower())
    if not mapping:
        return None
    value, conf = mapping
    return RuleHit(
        dimension="intent",
        value=value,
        confidence=conf,
        explanation=f"branch {kind}/{slug}",
    )


@rule(name="diff_view_visible", dim="intent")
def _diff_view(session: dict):
    text = session_text(session, ocr_limit=10)
    if not DIFF_VIEW_RE.search(text):
        return None
    return RuleHit(
        dimension="intent",
        value="exploration",
        confidence=0.6,
        explanation="diff/compare view in window titles",
    )


@rule(name="debug_keywords", dim="intent")
def _debug_keywords(session: dict):
    text = session_text(session, ocr_limit=10)
    matches = DEBUG_KEYWORDS_RE.findall(text)
    if len(matches) < 2:  # require at least two error mentions
        return None
    return RuleHit(
        dimension="intent",
        value="validation",
        confidence=0.65,
        explanation=f"{len(matches)} error/debug keyword(s) in OCR",
    )


@rule(name="doc_file_intent", dim="intent")
def _doc_file_intent(session: dict):
    if not DOC_FILE_RE.search(session_text(session, ocr_limit=10)):
        return None
    return RuleHit(
        dimension="intent",
        value="documentation",
        confidence=0.55,
        explanation="markdown/rst/adoc file in titles",
    )
