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

# Max chars of each candidate ticket's description included in the prompt.
# Default 0 = NO cap — the full description is sent. This field was previously
# hard-capped at 240 chars, which dropped 56-83% of real ticket text (measured:
# avg 548 chars, max 1440 across the live board), and the discriminating scope a
# session must be matched against frequently lives past char 240. With the
# 128K-context classifier and plan-only candidate sets (2-3 tickets), the prompt
# has ample budget, so descriptions are sent in full by default. Set
# CANDIDATE_DESC_CAP=<n> to re-impose a ceiling if an unusually long description
# ever bloats the prompt (e.g. on a full-candidate fallback day).
CANDIDATE_DESC_CAP = int(os.environ.get("CANDIDATE_DESC_CAP", "0"))

# Recent-work continuity window (minutes). The prompt summarises the developer's
# tracked work in this many minutes BEFORE the current session, aggregated per
# ticket, as a weak continuity prior. Time-windowed (not count-windowed) on
# purpose: session length is wildly variable, so "last N sessions" can be 90s of
# micro-glances or 3h of deep work. Shared with run_task_linker_mlx.py, which
# fetches the window. Override via CONTINUITY_WINDOW_MIN.
_CONTINUITY_WINDOW_MIN = int(os.environ.get("CONTINUITY_WINDOW_MIN", "30"))


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
        tags        = (task.get("tags") or "").strip()
        if CANDIDATE_DESC_CAP > 0 and len(desc) > CANDIDATE_DESC_CAP:
            desc = desc[:CANDIDATE_DESC_CAP] + "…"
        meta_parts = [p for p in [issue_type, f"Epic: {epic_title}" if epic_title else "", sprint_name, f"tags: {tags}" if tags else ""] if p]
        meta = "  [" + " · ".join(meta_parts) + "]" if meta_parts else ""
        # The dev declared this ticket as today's focus on the plan page. It's a
        # tie-breaking prior, not a forced answer — only matches if the evidence fits.
        focus = " ★ TODAY'S FOCUS" if task.get("is_today_focus") else ""
        rows.append(
            f"{i}. {task['task_key']}{focus}{meta}\n"
            f"   title: {title}\n"
            f"   description: {desc or '(empty)'}"
        )
    return "\n\n".join(rows) if rows else "(no candidates)"


def _fmt_continuity_mins(seconds: float) -> str:
    """Coarse minutes label for the continuity block: '<1 min' or '~N min'."""
    secs = int(seconds or 0)
    if secs < 60:
        return "<1 min"
    return f"~{round(secs / 60)} min"


def _format_continuity(activity: list[dict], now_iso: str | None = None) -> str:
    """Render the recent-ticket continuity prior — one bullet per ticket worked in
    the window, ordered most-recent-first: total time spent, how many sessions it
    spanned, and how long before the current session it was last active.

    `activity` entries come from `_fetch_recent_ticket_activity` (already
    aggregated, candidate-gated, confidence-filtered, recency-sorted). Empty input
    → an explicit "no tracked work" line (not ""), so the block is ALWAYS present:
    that tells the model definitively "there is no recent continuity — rely on this
    session's own evidence" (silence is ambiguous — it can't tell "no work" from
    "not provided") and keeps the trace node legible instead of blank. We
    deliberately do NOT emit a raw per-session log: those rows leak internal state
    (sub-threshold micro-sessions, not-yet-classified neighbours, two interleaved
    classify pipelines) that the model misreads as signal. This is a derived,
    calibrated statement of recent tracked work.
    """
    if not activity:
        return "  (no tracked work in this window)"
    lines = []
    for a in activity:
        total = _fmt_continuity_mins(a.get("total_s", 0))
        n = int(a.get("sessions", 0) or 0)
        sess = "1 session" if n == 1 else f"{n} sessions"
        ago_s = a.get("ago_s")
        if ago_s is None:
            recency = ""
        elif ago_s < 60:
            recency = ", last active just before this session"
        else:
            recency = f", last active ~{round(ago_s / 60)} min before this session"
        lines.append(f"  • {a['task_key']} — {total} over {sess}{recency}")
    return "\n".join(lines)


def build_user_message(
    session: dict,
    candidates: list[dict],
    recent_activity: list[dict] | None = None,
    now_iso: str | None = None,
) -> str:
    continuity = _format_continuity(recent_activity or [], now_iso)
    # ALWAYS emitted (even when empty, where `continuity` is an explicit
    # "no tracked work" line) so the model gets a definitive signal rather than
    # ambiguous silence, and the trace node is never blank. Framed as a WEAK prior,
    # never an instruction: an assertive "user was working on KAN-X" anchors the
    # model into force-linking — the exact false-positive failure mode the SKILL
    # warns against. The block states facts (ticket, time, recency); the SKILL's
    # "classify by THIS session's evidence" rule governs.
    recent_block = (
        f"RECENT WORK CONTEXT — the developer's tracked work in the last "
        f"{_CONTINUITY_WINDOW_MIN} minutes before this session. This is a WEAK "
        "continuity hint, NOT proof: continue the most-recent ticket ONLY if this "
        "session's own evidence also fits it; never link on continuity alone.\n"
        f"{continuity}\n"
        "\n"
    )
    # When the dev declared a focus for the day, name it in the header so the model
    # treats ★ rows as a prior — preferred when the evidence plausibly fits, but
    # never forced. Recall is preserved: every candidate is still listed.
    has_focus = any(c.get("is_today_focus") for c in candidates)
    candidate_header = (
        "CANDIDATE TICKETS (★ = the dev declared this as a task they're working on "
        "today; prefer a ★ ticket when the session plausibly matches it, but only "
        "if the evidence fits — never force a match):\n"
    ) if has_focus else "CANDIDATE TICKETS:\n"
    return (
        f"{recent_block}"
        "SESSION:\n"
        f"{_format_session(session)}\n"
        "\n"
        f"{candidate_header}"
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
