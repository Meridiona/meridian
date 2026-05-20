"""Prompt-building helpers for session_task_classifier — internal, not part of the public API."""
from __future__ import annotations

import re


_VSCODE_BANNER_RE = re.compile(
    r"\s+[—-]+\s+The following extensions want to relaunch.*$",
    re.IGNORECASE | re.DOTALL,
)


def _format_session(session: dict) -> str:
    parts: list[str] = []
    parts.append(f"app: {session.get('app_name') or '?'}")
    cat = session.get("category")
    cat_conf = session.get("confidence")
    if cat:
        parts.append(f"category: {cat} (confidence {round(cat_conf or 0.0, 2)})")
    dur = session.get("duration_s")
    if dur is not None:
        parts.append(f"duration: {dur}s")
    titles = session.get("window_titles") or []
    if titles:
        parts.append("top windows:")
        for t in titles[:5]:
            if isinstance(t, dict):
                name = t.get("window_name") or t.get("title") or ""
                cnt  = t.get("count", 1)
            elif isinstance(t, (list, tuple)) and t:
                name, cnt = t[0], (t[1] if len(t) > 1 else 1)
            else:
                name, cnt = str(t), 1
            name = _VSCODE_BANNER_RE.sub("", name).strip()
            parts.append(f"  • {name} (×{cnt})")
    session_text_val = session.get("session_text") or ""
    audio = session.get("audio_snippets") or []
    parts.append(f"session_text: {len(session_text_val)} chars")
    if audio:
        parts.append(f"audio_snippets: {len(audio)} captured")
    return "\n".join(parts)


def _format_candidates(tasks: list[dict]) -> str:
    rows: list[str] = []
    for i, task in enumerate(tasks, start=1):
        title = (task.get("title") or "").strip()
        desc  = (task.get("description_text") or "").strip()
        if len(desc) > 240:
            desc = desc[:240] + "…"
        rows.append(
            f"{i}. {task['task_key']}\n"
            f"   title: {title}\n"
            f"   description: {desc or '(empty)'}"
        )
    return "\n\n".join(rows) if rows else "(no candidates)"


def build_user_message(session: dict, candidates: list[dict]) -> str:
    return (
        "SESSION:\n"
        f"{_format_session(session)}\n"
        "\n"
        "CANDIDATE TICKETS:\n"
        f"{_format_candidates(candidates)}"
    )


def parse_dimensions(obj: dict) -> dict[str, list[str]]:
    """Extract the optional 'dimensions' field from an agent response object."""
    raw = obj.get("dimensions")
    if not isinstance(raw, dict):
        return {}
    valid = {"activity", "intent", "engagement", "collaboration", "tool", "topic", "practice"}
    result: dict[str, list[str]] = {}
    for dim, vals in raw.items():
        if dim not in valid:
            continue
        if isinstance(vals, list):
            cleaned = [str(v).strip().lower() for v in vals if v]
            if cleaned:
                result[dim] = cleaned
        elif isinstance(vals, str) and vals.strip():
            result[dim] = [vals.strip().lower()]
    return result
