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
    """Return the canonical embedding input for a session (single-vector mode).

    Kept for callers that still want a one-string view of the session, but
    Stage 2 now uses `session_text_samples` for multi-vector encoding.
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


# Per-sample budgets: each piece is independently meaningful, so we don't need
# the global 3 KB cap any more. Per-OCR-sample cap of 1500 keeps the encoder
# in its sweet spot (≤512 tokens) while preserving Tailscale-style content.
_PER_OCR_SAMPLE_CAP = 1500
_MIN_OCR_SAMPLE_CHARS = 30
_MAX_OCR_SAMPLES      = 20
_PER_AUDIO_CAP        = 1500


def session_text_samples(session: dict) -> list[tuple[str, str]]:
    """Return a list of (label, text) tuples to embed independently.

    Stage 2 max-pools cosine similarity over these — so each sample only
    needs to be meaningful on its own, and noisy ones (e.g. an OS chrome
    OCR frame) won't drag the matched ticket out of the running.

    Labels are stable: 'titles', 'audio', 'ocr_0' ... 'ocr_N'. The
    matching code uses these labels for debug output ("which sample
    matched best for this ticket?").
    """
    out: list[tuple[str, str]] = []

    # Title block — the strongest "what is the user looking at" signal.
    # We prepend app + category as a metadata header so the encoder gets
    # the right register (e.g. "app: Code, category: coding | windows: ...").
    titles = _join_titles(session)
    if titles:
        meta = " ".join(filter(None, [
            f"app: {session.get('app_name')}" if session.get("app_name") else "",
            f"category: {session.get('category')}" if session.get("category") else "",
        ])).strip()
        out.append(("titles", f"{meta} | windows: {titles}" if meta else f"windows: {titles}"))
    elif session.get("app_name"):
        # No titles — at least give the encoder the app name + category.
        meta = " ".join(filter(None, [
            f"app: {session.get('app_name')}" if session.get("app_name") else "",
            f"category: {session.get('category')}" if session.get("category") else "",
        ])).strip()
        if meta:
            out.append(("titles", meta))

    # Each OCR sample as its own document. Tiny and obviously-junk samples
    # are skipped — they only add noise to the max-pool.
    for i, s in enumerate((session.get("ocr_samples") or [])[:_MAX_OCR_SAMPLES]):
        text = (s.get("text", "") if isinstance(s, dict) else str(s)).strip()
        if len(text) < _MIN_OCR_SAMPLE_CHARS:
            continue
        if len(text) > _PER_OCR_SAMPLE_CAP:
            text = text[:_PER_OCR_SAMPLE_CAP]
        out.append((f"ocr_{i}", text))

    # Audio block — small enough that one combined doc is fine.
    audio = _join_audio(session, budget=_PER_AUDIO_CAP)
    if audio:
        out.append(("audio", audio))

    if not out:
        # Pathological — make sure we still have *something* to embed
        # (otherwise stage 2 returns empty and the inspector reports nothing).
        out.append(("empty", session.get("app_name") or "session"))

    return out


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


__all__ = ["session_text", "session_text_samples", "task_text", "text_hash"]
