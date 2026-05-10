"""Deterministic text composition for the embedding stage.

Stage 2 needs *one stable recipe* for "what text represents this session?"
and "what text represents this pm_task?". The hash of that text is what we
use to detect "needs re-embedding" — so the recipe must be reproducible
across runs (no timestamps, no order dependence on dict iteration, etc.).
"""
from __future__ import annotations

import hashlib
import re

# How much OCR text to keep per session. Embeddings degrade past ~500 tokens
# of noise (per the bge-small docs); we cap at ~3 KB which is roughly
# 600–800 tokens for the kind of OCR fragments we see.
_OCR_BUDGET_CHARS  = 3000
_AUDIO_BUDGET_CHARS = 1000
_TASK_BUDGET_CHARS = 2000

# Strip the trailing "The following extensions want to relaunch the
# terminal..." banner that VS Code puts on the active window title — it
# pollutes the signal heavily (every Code session ends up with "Python"
# and "github.com" tokens regardless of what the user is doing).
_VSCODE_BANNER_RE = re.compile(
    r"\s+[—-]+\s+The following extensions want to relaunch.*$",
    re.IGNORECASE | re.DOTALL,
)


def _clean_title(text: str) -> str:
    text = (text or "").strip()
    return _VSCODE_BANNER_RE.sub("", text)


def _join_titles(session: dict) -> str:
    """Concatenate window titles, weighting by their on-screen count.

    The Rust ETL stores window titles as `[{"window_name": "...", "count": N}]`
    so titles seen many times (the focused window) count more than fleeting
    pop-ups. We repeat each title up to 3× by count to give it more weight
    in the embedding without making the prompt explode.
    """
    titles = session.get("window_titles") or []
    parts: list[str] = []
    for t in titles:
        name = ""
        count = 1
        if isinstance(t, dict):
            name = t.get("window_name") or t.get("title") or ""
            count = int(t.get("count") or 1)
        elif isinstance(t, (list, tuple)) and t:
            name = str(t[0])
            count = int(t[1]) if len(t) > 1 else 1
        elif isinstance(t, str):
            name = t
        cleaned = _clean_title(name)
        if not cleaned:
            continue
        weight = max(1, min(3, count))
        for _ in range(weight):
            parts.append(cleaned)
    return " | ".join(parts)


def _join_ocr(session: dict, budget: int = _OCR_BUDGET_CHARS) -> str:
    samples = session.get("ocr_samples") or []
    out: list[str] = []
    used = 0
    for s in samples:
        text = s.get("text", "") if isinstance(s, dict) else str(s)
        text = text.strip()
        if not text:
            continue
        room = budget - used
        if room <= 0:
            break
        if len(text) > room:
            text = text[:room]
        out.append(text)
        used += len(text) + 1
    return " ".join(out)


def _join_audio(session: dict, budget: int = _AUDIO_BUDGET_CHARS) -> str:
    snips = session.get("audio_snippets") or []
    out: list[str] = []
    used = 0
    for s in snips:
        text = s.get("text", "") if isinstance(s, dict) else str(s)
        text = text.strip()
        if not text:
            continue
        room = budget - used
        if room <= 0:
            break
        if len(text) > room:
            text = text[:room]
        out.append(text)
        used += len(text) + 1
    return " ".join(out)


def session_text(session: dict) -> str:
    """Return the canonical embedding input for a session.

    Format is `app | titles | ocr | audio` so the encoder gets a clear
    structural cue. Sections are dropped when empty.
    """
    parts: list[str] = []
    app = (session.get("app_name") or "").strip()
    if app:
        parts.append(f"app: {app}")
    cat = (session.get("category") or "").strip()
    if cat:
        parts.append(f"category: {cat}")
    titles = _join_titles(session)
    if titles:
        parts.append(f"windows: {titles}")
    ocr = _join_ocr(session)
    if ocr:
        parts.append(f"ocr: {ocr}")
    audio = _join_audio(session)
    if audio:
        parts.append(f"audio: {audio}")
    return "\n".join(parts)


def task_text(task: dict) -> str:
    """Return the canonical embedding input for a pm_task row."""
    title = (task.get("title") or "").strip()
    desc  = (task.get("description_text") or "").strip()
    issue_type = (task.get("issue_type") or "").strip()
    project    = (task.get("project_key") or "").strip()
    parts: list[str] = []
    if title:
        parts.append(f"title: {title}")
    if issue_type:
        parts.append(f"type: {issue_type}")
    if project:
        parts.append(f"project: {project}")
    if desc:
        if len(desc) > _TASK_BUDGET_CHARS:
            desc = desc[:_TASK_BUDGET_CHARS]
        parts.append(f"description: {desc}")
    return "\n".join(parts)


def text_hash(text: str) -> str:
    """Stable, short hash for change-detection (sha1 hex, first 16 chars)."""
    return hashlib.sha1(text.encode("utf-8", errors="replace")).hexdigest()[:16]


__all__ = ["session_text", "task_text", "text_hash"]
