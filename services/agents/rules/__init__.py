"""Stage-1 rule framework.

A rule is a function that takes a `Session` (the dict returned by
`db.fetch_unprocessed_sessions`) and returns zero or more `RuleHit`
records. Each hit is (dimension, value, confidence, source).

Rules are registered via the `@rule(name=..., dim=...)` decorator. Importing
agents.rules.* causes them to register themselves; the runner just iterates
RULE_REGISTRY in order.

Resolution policy:
* For SINGLE_VALUE_DIMENSIONS, the runner keeps only the highest-confidence
  hit per session (ties broken by rule order).
* For multi-value dimensions, all distinct (value, confidence) pairs survive
  but duplicate values keep their highest confidence.
"""
from __future__ import annotations

import importlib
import logging
import pkgutil
import re
from dataclasses import dataclass
from typing import Callable, Iterable

from agents.taxonomy import (
    DIMENSIONS,
    SINGLE_VALUE_DIMENSIONS,
    is_known_dimension,
    is_known_value,
)

log = logging.getLogger("agents.rules")


# ────────────────────────── Hit + registry ─────────────────────────────────────
@dataclass
class RuleHit:
    dimension: str
    value: str
    confidence: float = 0.8
    source: str = "rule"          # populated automatically with rule name when via @rule
    explanation: str | None = None  # human-readable trace, used for logging


# Each entry: (rule_name, dim_label_for_grouping, callable).
# A rule is *not* limited to one dimension — its return list can contain hits
# for any dimension. `dim_label_for_grouping` is just the rule's primary
# concern, used for the registry summary printout.
RULE_REGISTRY: list[tuple[str, str, Callable]] = []


def rule(name: str, dim: str = "*"):
    """Decorator. Wraps a callable that takes a session dict and returns
    `RuleHit | list[RuleHit] | None`."""
    def deco(fn: Callable):
        fn.__rule_name__ = name
        fn.__rule_dim__ = dim
        RULE_REGISTRY.append((name, dim, fn))
        return fn
    return deco


# ────────────────────────── Session text helpers ──────────────────────────────
URL_RE = re.compile(r"https?://[^\s\"<>\)]+", re.I)
TICKET_RE = re.compile(r"\b([A-Z][A-Z0-9]+-\d+)\b")

# Common non-ticket false positives the regex accidentally catches.
_TICKET_FALSE_POSITIVES = frozenset({
    "UTF-8", "UTF-16", "UTF-32", "ASCII-7", "ASCII-8",
    "HTTP-200", "HTTP-201", "HTTP-204", "HTTP-301", "HTTP-302",
    "HTTP-400", "HTTP-401", "HTTP-403", "HTTP-404", "HTTP-405",
    "HTTP-409", "HTTP-429", "HTTP-500", "HTTP-502", "HTTP-503",
    "HTTPS-443", "TCP-80", "TCP-443", "UDP-53",
    "SHA-1", "SHA-256", "SHA-512", "MD-5",
    "RGB-0", "RGBA-0", "AES-128", "AES-256",
    "RFC-2616", "RFC-7231", "ISO-8859", "BASE-64",
    "X-1", "Y-1", "Z-1", "PI-1", "C-1", "F-1",
    "GPT-3", "GPT-4", "GPT-5",
})
BRANCH_RE = re.compile(
    r"\b(feat|fix|bug|refactor|docs|test|chore|style|perf)/([a-zA-Z0-9._-]+)",
    re.I,
)


def _join_titles(session: dict) -> str:
    titles = session.get("window_titles") or []
    parts = []
    for t in titles:
        if isinstance(t, dict):
            parts.append(str(t.get("title") or t.get("window_name") or ""))
        elif isinstance(t, (list, tuple)) and t:
            parts.append(str(t[0]))
        elif isinstance(t, str):
            parts.append(t)
    return " ".join(parts)


def _join_ocr(session: dict, limit: int = 10) -> str:
    samples = session.get("ocr_samples") or []
    parts = []
    for s in samples[:limit]:
        if isinstance(s, dict):
            parts.append(str(s.get("text", "")))
        elif isinstance(s, str):
            parts.append(s)
    return " ".join(parts)


def _join_audio(session: dict) -> str:
    snips = session.get("audio_snippets") or []
    parts = []
    for s in snips:
        if isinstance(s, dict):
            parts.append(str(s.get("text", "")))
        elif isinstance(s, str):
            parts.append(s)
    return " ".join(parts)


def session_text(session: dict, *, ocr_limit: int = 10) -> str:
    """All session text concatenated for matching, lowercased upstream."""
    return " ".join([
        session.get("app_name") or "",
        _join_titles(session),
        _join_ocr(session, limit=ocr_limit),
        _join_audio(session),
    ])


def extract_urls(session: dict) -> list[str]:
    return URL_RE.findall(session_text(session, ocr_limit=20))


def extract_tickets(session: dict) -> list[str]:
    """Find all UPPERCASE-NUMERIC ticket-key candidates in titles/OCR/audio,
    minus common false positives (UTF-8, HTTP-404, GPT-4, etc.)."""
    found = TICKET_RE.findall(session_text(session, ocr_limit=20))
    deduped = list(dict.fromkeys(found))
    return [k for k in deduped if k not in _TICKET_FALSE_POSITIVES]


def extract_branches(session: dict) -> list[tuple[str, str]]:
    """Returns list of (kind, slug) tuples — e.g. [('feat', 'KAN-86-migrate')]."""
    return [(m.group(1).lower(), m.group(2)) for m in BRANCH_RE.finditer(session_text(session, ocr_limit=20))]


# ────────────────────────── Runner ─────────────────────────────────────────────
def discover_rules() -> None:
    """Import every submodule of agents.rules so each registers via @rule.

    Idempotent — repeated calls don't re-import.
    """
    package = importlib.import_module("agents.rules")
    for info in pkgutil.iter_modules(package.__path__):
        if info.name.startswith("_"):
            continue
        importlib.import_module(f"agents.rules.{info.name}")


def run_rules(session: dict) -> list[RuleHit]:
    """Run every registered rule and return all hits.

    Each hit's `source` is set to `rule:<rule_name>` so downstream consumers
    can attribute the decision. Hits for unknown dimensions or unknown values
    on closed dimensions are dropped with a warning.
    """
    raw_hits: list[RuleHit] = []
    for rule_name, _dim, fn in RULE_REGISTRY:
        try:
            result = fn(session)
        except Exception as exc:
            log.exception("rule %s raised on session %s — skipping", rule_name, session.get("id"))
            continue
        if not result:
            continue
        items = result if isinstance(result, list) else [result]
        for h in items:
            if not isinstance(h, RuleHit):
                log.warning("rule %s returned non-RuleHit: %r", rule_name, h)
                continue
            if not is_known_dimension(h.dimension):
                log.warning("rule %s: unknown dimension %r — dropping", rule_name, h.dimension)
                continue
            if not is_known_value(h.dimension, h.value):
                log.warning("rule %s: unknown value %r for dim %r — dropping",
                            rule_name, h.value, h.dimension)
                continue
            if h.source == "rule":
                h.source = f"rule:{rule_name}"
            log.info(
                "    fire  %-22s → %-13s = %-22s conf=%.2f%s",
                rule_name,
                h.dimension,
                h.value,
                h.confidence,
                f"  ({h.explanation})" if h.explanation else "",
            )
            raw_hits.append(h)
    return raw_hits


def resolve_hits(hits: Iterable[RuleHit]) -> list[RuleHit]:
    """Apply single-value-dimension policy and dedupe.

    For each session×dimension×value, keep the highest-confidence hit. For
    SINGLE_VALUE_DIMENSIONS, additionally keep only the highest-confidence
    value (one row per dimension).
    """
    # Step 1: dedupe by (dim, value), keep highest confidence.
    by_key: dict[tuple[str, str], RuleHit] = {}
    for h in hits:
        key = (h.dimension, h.value)
        prev = by_key.get(key)
        if prev is None or h.confidence > prev.confidence:
            by_key[key] = h
    deduped = list(by_key.values())

    # Step 2: for single-value dimensions, take the top one.
    singles: dict[str, RuleHit] = {}
    multi: list[RuleHit] = []
    for h in deduped:
        if h.dimension in SINGLE_VALUE_DIMENSIONS:
            prev = singles.get(h.dimension)
            if prev is None or h.confidence > prev.confidence:
                singles[h.dimension] = h
        else:
            multi.append(h)
    return list(singles.values()) + multi


__all__ = [
    "RuleHit",
    "rule",
    "RULE_REGISTRY",
    "discover_rules",
    "run_rules",
    "resolve_hits",
    "session_text",
    "extract_urls",
    "extract_tickets",
    "extract_branches",
    "URL_RE",
    "TICKET_RE",
    "BRANCH_RE",
]
