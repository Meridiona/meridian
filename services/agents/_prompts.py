"""Prompt-building helpers for session_task_classifier — internal, not part of the public API."""
from __future__ import annotations

import os
import re
from datetime import datetime


_VSCODE_BANNER_RE = re.compile(
    r"\s+[—-]+\s+The following extensions want to relaunch.*$",
    re.IGNORECASE | re.DOTALL,
)

# Max chars of session_text included in the prompt. Default 2500 (~625 tokens at
# 4 chars/token) — enough to identify files, ticket keys, and recent activity
# without inflating context in production. Override via SESSION_TEXT_CAP env var
# for eval experiments; set to 0 to disable truncation entirely (caller is then
# responsible for not blowing the model's context window).
SESSION_TEXT_CAP = int(os.environ.get("SESSION_TEXT_CAP", "2500"))


def _fmt_dur(duration_s: int | float) -> str:
    secs = int(duration_s or 0)
    if secs < 60:
        return "<1min"
    return f"{secs // 60}min"


def _fmt_time(ts: str) -> str:
    """Parse an ISO8601 timestamp and return HH:MM in local timezone."""
    try:
        return datetime.fromisoformat(ts.replace("Z", "+00:00")).astimezone().strftime("%H:%M")
    except Exception:
        return ts[:5] if len(ts) >= 5 else ts


def _format_session(session: dict) -> str:
    parts: list[str] = []
    parts.append(f"app: {session.get('app_name') or '?'}")
    started = (session.get("started_at") or "").strip()
    ended   = (session.get("ended_at") or "").strip()
    dur     = session.get("duration_s")
    if started or ended:
        time_parts = []
        if started:
            time_parts.append(_fmt_time(started))
        if ended:
            time_parts.append(_fmt_time(ended))
        time_range = "–".join(time_parts)
        dur_str = f"  ({_fmt_dur(dur)})" if dur is not None else ""
        parts.append(f"time: {time_range}{dur_str}")
    elif dur is not None:
        parts.append(f"duration: {_fmt_dur(dur)}")
    cat = session.get("category")
    cat_conf = session.get("confidence")
    if cat:
        parts.append(f"category: {cat} (confidence {round(cat_conf or 0.0, 2)})")
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
    session_text_val = (session.get("session_text") or "").strip()
    if session_text_val:
        source = (session.get("session_text_source") or "unknown").strip().lower()
        total = len(session_text_val)
        if SESSION_TEXT_CAP > 0 and total > SESSION_TEXT_CAP:
            excerpt = session_text_val[:SESSION_TEXT_CAP]
            tail = f"\n  … ({total - SESSION_TEXT_CAP} more chars)"
        else:
            excerpt = session_text_val
            tail = ""
        parts.append(f"screen content [{source}]:\n{excerpt}{tail}")

    audio = session.get("audio_snippets") or []
    if audio:
        parts.append(f"audio_snippets: {len(audio)} captured")
    return "\n".join(parts)


def _format_candidates(tasks: list[dict]) -> str:
    rows: list[str] = []
    for i, task in enumerate(tasks, start=1):
        title       = (task.get("title") or "").strip()
        desc        = (task.get("description_text") or "").strip()
        issue_type  = (task.get("issue_type") or "").strip()
        epic_title  = (task.get("epic_title") or "").strip()
        sprint_name = (task.get("sprint_name") or "").strip()
        if len(desc) > 240:
            desc = desc[:240] + "…"
        meta_parts = [p for p in [issue_type, f"Epic: {epic_title}" if epic_title else "", sprint_name] if p]
        meta = "  [" + " · ".join(meta_parts) + "]" if meta_parts else ""
        rows.append(
            f"{i}. {task['task_key']}{meta}\n"
            f"   title: {title}\n"
            f"   description: {desc or '(empty)'}"
        )
    return "\n\n".join(rows) if rows else "(no candidates)"


def _format_recent_sessions(sessions: list[dict]) -> str:
    if not sessions:
        return "  (no recent session context)"
    rows = []
    for s in sessions:
        time_str = _fmt_time(s.get("started_at") or "")
        app = (s.get("app_name") or "?")[:14]
        dur_str = _fmt_dur(s.get("duration_s") or 0)
        task_key = s.get("task_key")
        routing = s.get("task_routing")  # None means unclassified
        category = (s.get("category") or "").strip()
        if task_key:
            target = f"→ {task_key}"
        elif routing == "untracked":
            target = "→ [untracked]"
        elif routing is None:
            # session captured but not yet classified
            target = "→ [pending]"
        else:
            target = "→ [overhead]"
        cat_tag = f"  [{category}]" if category else ""
        rows.append(f"  {time_str}  {app:<14}  {dur_str:<7}  {target}{cat_tag}")
    return "\n".join(rows)


def build_user_message(
    session: dict,
    candidates: list[dict],
    recent_sessions: list[dict] | None = None,
) -> str:
    sessions = recent_sessions or []
    has_any_task_key = any(s.get("task_key") for s in sessions)
    recent_block = (
        "RECENT WORK CONTEXT:\n"
        f"{_format_recent_sessions(sessions)}\n"
        "\n"
    ) if has_any_task_key else ""
    return (
        f"{recent_block}"
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
