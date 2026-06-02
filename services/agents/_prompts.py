"""Prompt-building helpers for session_task_classifier — internal, not part of the public API."""
from __future__ import annotations

import os
import re
from datetime import datetime


_VSCODE_BANNER_RE = re.compile(
    r"\s+[—-]+\s+The following extensions want to relaunch.*$",
    re.IGNORECASE | re.DOTALL,
)

# Max chars of session_text included in the prompt. Default 10000 (~2500 tokens
# at 4 chars/token). The old 2500 cap kept only the FIRST frames of a multi-frame
# OCR capture, so when a session spanned more than one window/app the later
# (often foreground) activity was silently dropped — e.g. a session whose head
# showed an IDE but whose tail showed the user had moved to a different app/
# project got misclassified on the stale head. The classifier model has a 128K
# context window, so 2500 was far too conservative; 10000 comfortably holds a
# full multi-frame session while staying trivial for the model. Override via
# SESSION_TEXT_CAP env var; set to 0 to disable truncation entirely (caller is
# then responsible for not blowing the model's context window).
SESSION_TEXT_CAP = int(os.environ.get("SESSION_TEXT_CAP", "10000"))

# Floors for the whole-prompt token budget (see build_user_message). When the
# selected model's context window is tight enough to force truncation, neither
# the session evidence nor the candidate-ticket list is starved below these
# char floors — the candidate list is what the model matches task_key against,
# the session text is the evidence it matches on, so both must survive.
_SESSION_TEXT_FLOOR = 500
_CANDIDATES_FLOOR = 800
# The "screen content [src]:" header + truncation tail that _format_session adds
# around session_text but that the empty-text skeleton measurement omits. Held
# as a small fixed reserve so the budget over-counts overhead rather than under.
_SCREEN_BLOCK_OVERHEAD = 64


def _fmt_dur(duration_s: int | float) -> str:
    secs = int(duration_s or 0)
    if secs < 60:
        return "<1min"
    return f"{secs // 60}min"


def _fmt_time(ts: str) -> str:
    """Parse an ISO8601 timestamp and return HH:MM in its original timezone."""
    try:
        return datetime.fromisoformat(ts).strftime("%H:%M")
    except Exception:
        return ts[:5] if len(ts) >= 5 else ts


def _format_session(session: dict, session_text_cap: int | None = None) -> str:
    cap = SESSION_TEXT_CAP if session_text_cap is None else session_text_cap
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
    # NOTE: the rule-based ETL category is intentionally NOT included here. It is
    # a cheap heuristic derived from the SAME app/window/OCR signals the LLM
    # already sees, so feeding it in only injects a correlated prior — when the
    # heuristic is wrong (e.g. background-window OCR bleed), it biases the LLM
    # toward the same mistake. The classifier re-derives category from the raw
    # evidence and its output overwrites the rule-based value anyway.
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
        if cap > 0 and total > cap:
            excerpt = session_text_val[:cap]
            tail = f"\n  … ({total - cap} more chars)"
        else:
            excerpt = session_text_val
            tail = ""
        parts.append(f"screen content [{source}]:\n{excerpt}{tail}")

    audio = session.get("audio_snippets") or []
    if audio:
        parts.append(f"audio_snippets: {len(audio)} captured")
    return "\n".join(parts)


def _format_one_candidate(i: int, task: dict) -> str:
    title       = (task.get("title") or "").strip()
    desc        = (task.get("description_text") or "").strip()
    issue_type  = (task.get("issue_type") or "").strip()
    epic_title  = (task.get("epic_title") or "").strip()
    sprint_name = (task.get("sprint_name") or "").strip()
    if len(desc) > 240:
        desc = desc[:240] + "…"
    meta_parts = [p for p in [issue_type, f"Epic: {epic_title}" if epic_title else "", sprint_name] if p]
    meta = "  [" + " · ".join(meta_parts) + "]" if meta_parts else ""
    return (
        f"{i}. {task['task_key']}{meta}\n"
        f"   title: {title}\n"
        f"   description: {desc or '(empty)'}"
    )


def _format_candidates(tasks: list[dict]) -> str:
    rows = [_format_one_candidate(i, t) for i, t in enumerate(tasks, start=1)]
    return "\n\n".join(rows) if rows else "(no candidates)"


def _fit_candidates(tasks: list[dict], char_budget: int) -> str:
    """Format candidate tickets, dropping whole tickets from the end to fit.

    Always keeps at least the first ticket (the model needs a non-empty answer
    space), and appends an explicit note when any are omitted so a partial list
    is visible to the model rather than silently truncated.
    """
    if not tasks:
        return "(no candidates)"
    rows: list[str] = []
    used = 0
    for i, task in enumerate(tasks, start=1):
        row = _format_one_candidate(i, task)
        add = len(row) + (2 if rows else 0)  # "\n\n" separator between rows
        if rows and used + add > char_budget:
            break
        rows.append(row)
        used += add
    omitted = len(tasks) - len(rows)
    block = "\n\n".join(rows)
    if omitted > 0:
        block += (
            f"\n\n… ({omitted} more candidate ticket(s) omitted to fit the "
            "model's context window)"
        )
    return block


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
        if task_key:
            target = f"→ {task_key}"
        elif routing == "untracked":
            target = "→ [untracked]"
        elif routing is None:
            # session captured but not yet classified
            target = "→ [pending]"
        else:
            target = "→ [overhead]"
        # Category is intentionally omitted — recent-context is a task-continuity
        # signal only; carrying the (rule-based or prior-LLM) category tag would
        # feed a category prior back into classification.
        rows.append(f"  {time_str}  {app:<14}  {dur_str:<7}  {target}")
    return "\n".join(rows)


def build_user_message(
    session: dict,
    candidates: list[dict],
    recent_sessions: list[dict] | None = None,
    *,
    char_budget: int | None = None,
) -> str:
    """Assemble the classifier user message.

    char_budget (chars) caps the WHOLE user message so system + this message +
    output reserve fit the selected model's context window. When None (eval with
    an explicit SESSION_TEXT_CAP, non-MLX callers), the legacy static path runs:
    session_text capped by SESSION_TEXT_CAP, candidate list uncapped.

    With a budget, session_text keeps its normal cap and the (otherwise uncapped)
    candidate list absorbs overflow first — only when the candidate floor still
    won't fit does session_text shrink toward its floor. In the common case of a
    roomy window the output is byte-identical to the static path.
    """
    sessions = recent_sessions or []
    has_any_task_key = any(s.get("task_key") for s in sessions)
    recent_block = (
        "RECENT WORK CONTEXT:\n"
        f"{_format_recent_sessions(sessions)}\n"
        "\n"
    ) if has_any_task_key else ""

    if char_budget is None:
        return (
            f"{recent_block}"
            "SESSION:\n"
            f"{_format_session(session)}\n"
            "\n"
            "CANDIDATE TICKETS:\n"
            f"{_format_candidates(candidates)}"
        )

    # Fixed overhead = everything except the two truncatable inputs (session_text
    # and the candidate block). Measure it by rendering the session scaffold with
    # the text removed, plus a small reserve for the screen-content header/tail.
    scaffold = _format_session({**session, "session_text": ""})
    overhead = (
        len(recent_block)
        + len("SESSION:\n") + len(scaffold) + len("\n\nCANDIDATE TICKETS:\n")
        + _SCREEN_BLOCK_OVERHEAD
    )
    avail = max(0, char_budget - overhead)

    session_text = (session.get("session_text") or "").strip()
    default_cap = SESSION_TEXT_CAP if SESSION_TEXT_CAP > 0 else len(session_text)
    session_text_used = min(default_cap, len(session_text))

    cand_budget = avail - session_text_used
    if cand_budget < _CANDIDATES_FLOOR:
        # Candidates would be starved — claw session_text back toward its floor.
        session_text_used = max(_SESSION_TEXT_FLOOR, avail - _CANDIDATES_FLOOR)
        cand_budget = max(_CANDIDATES_FLOOR, avail - session_text_used)

    return (
        f"{recent_block}"
        "SESSION:\n"
        f"{_format_session(session, session_text_cap=session_text_used)}\n"
        "\n"
        "CANDIDATE TICKETS:\n"
        f"{_fit_candidates(candidates, cand_budget)}"
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
