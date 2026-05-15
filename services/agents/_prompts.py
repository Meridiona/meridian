"""Prompt-building helpers for task_classifier_agent — internal, not part of the public API."""
from __future__ import annotations

import re
from typing import Any

from agents.config import load_skill_addendum

SKILL_NAME = ""  # resolved lazily from task_classifier_agent.SKILL_NAME

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


def _format_dimensions(dims_grouped: dict[str, set[str]]) -> str:
    if not dims_grouped:
        return "(none)"
    out: list[str] = []
    for dim in ("activity", "intent", "engagement", "collaboration",
                "tool", "topic", "practice"):
        vals = sorted(dims_grouped.get(dim, set()))
        if not vals:
            continue
        out.append(f"  - {dim}: {', '.join(vals[:8])}"
                   + (f" (+{len(vals) - 8} more)" if len(vals) > 8 else ""))
    return "\n".join(out) or "(none)"


def _format_candidates(top_candidates: list, pm_task_lookup: dict[str, dict]) -> str:
    rows: list[str] = []
    for i, c in enumerate(top_candidates, start=1):
        task = pm_task_lookup.get(c.task_key, {})
        title = (task.get("title") or "").strip()
        desc  = (task.get("description_text") or "").strip()
        if len(desc) > 240:
            desc = desc[:240] + "…"
        rows.append(
            f"{i}. {c.task_key} (cosine={c.cosine:.2f}, dim_overlap={c.dim_overlap:.2f}, "
            f"score={c.score:.2f})\n"
            f"   title: {title}\n"
            f"   description: {desc or '(empty)'}"
        )
    return "\n\n".join(rows) if rows else "(no candidates)"


def _prefilter_tasks(
    session: dict,
    all_pm_tasks: list[dict],
    max_tasks: int,
) -> list[dict]:
    """Return up to max_tasks tasks ordered by keyword overlap with the session."""
    words: set[str] = set()
    for t in (session.get("window_titles") or []):
        name = (t.get("window_name") or t.get("title") or "") if isinstance(t, dict) else str(t)
        words.update(w.lower() for w in re.split(r"\W+", name) if len(w) > 3)
    session_text_words = (session.get("session_text") or "")
    words.update(w.lower() for w in re.split(r"\W+", session_text_words) if len(w) > 3)
    if not words:
        return all_pm_tasks[:max_tasks]
    scored: list[tuple[int, dict]] = []
    for task in all_pm_tasks:
        task_text = f"{task.get('title', '')} {task.get('description_text', '')}".lower()
        score = sum(1 for w in words if w in task_text)
        scored.append((score, task))
    scored.sort(key=lambda x: -x[0])
    return [t for _, t in scored[:max_tasks]]


def _format_candidates_standalone(tasks: list[dict]) -> str:
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


def build_system_prompt(skill_name: str, mode: str, base: str) -> str:
    """Append the mode-specific addendum from SKILL-{mode}.md to the base prompt."""
    addendum = load_skill_addendum(skill_name, mode)
    return (base + "\n" + addendum) if addendum.strip() else base


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


def build_user_message(
    session: dict,
    dims_grouped: dict[str, set[str]],
    top_candidates: list,
    pm_task_lookup: dict[str, dict],
    *,
    mode: str,
    mode_tiebreak: str,
    mode_no_dims: str,
    mode_standalone: str,
    standalone_tasks: list[dict] | None = None,
) -> str:
    if mode == mode_no_dims:
        dims_section = "(Stage 1 disabled — no rule-extracted dimensions available)"
    elif mode == mode_standalone:
        dims_section = (
            "(Stages 1 and 2 disabled — no dimensions available; "
            "infer from session evidence and include a `dimensions` field in your JSON)"
        )
    else:
        dims_section = _format_dimensions(dims_grouped)

    if mode == mode_standalone and standalone_tasks is not None:
        candidates_section = _format_candidates_standalone(standalone_tasks)
    else:
        candidates_section = _format_candidates(top_candidates, pm_task_lookup)

    return (
        "SESSION:\n"
        f"{_format_session(session)}\n"
        "\n"
        "OBSERVED DIMENSIONS (rule-extracted):\n"
        f"{dims_section}\n"
        "\n"
        "CANDIDATE TICKETS:\n"
        f"{candidates_section}"
    )
