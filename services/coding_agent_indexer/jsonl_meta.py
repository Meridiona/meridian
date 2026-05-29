"""Parse a coding-agent JSONL once and return per-day session slices.

A Claude Code or Codex session writes its conversation as an append-only
.jsonl file that can span many calendar days. We slice it by the user's
local calendar day so PM-update windowing (`WHERE started_at BETWEEN
?...`) attributes time correctly — yesterday's work shows up under
yesterday, today's work under today, etc.

Public surface:

  * `parse_session_slices()` — single pass; returns the overall
    `SessionMeta` plus a list of `DaySlice` (one per local day that has
    at least one record). The indexer's UPSERT loops over the slices.

Deliberately ignored (despite being present on every record):
  * gitBranch — by product decision
  * version, parentUuid — not useful for indexing
  * UI-state types (mode, ai-title, permission-mode, last-prompt,
    pr-link, file-history-snapshot, attachment, system,
    queue-operation) — not conversation, would just bloat the
    transcript

Tolerant: malformed lines, missing fields, files truncated mid-write,
files with zero meaningful records all degrade gracefully — slices'
`is_valid` flag tells the caller whether registration should proceed;
the renderer returns an empty string on a broken file.
"""
from __future__ import annotations

import json
import logging
from dataclasses import dataclass, field
from datetime import datetime, timezone, tzinfo
from pathlib import Path
from typing import Iterator, List, Optional, Tuple

log = logging.getLogger(__name__)

# Record types we *count* as conversational turns. Everything else
# (mode, ai-title, permission-mode, last-prompt, pr-link, etc.) is
# ignored for the turn count — it's UI state, not conversation.
_TURN_TYPES_USER      = frozenset({"user"})
_TURN_TYPES_ASSISTANT = frozenset({"assistant"})

# Truncation for noisy tool_result bodies (file dumps, large search
# outputs). Keeps the rendered transcript bounded.
_TOOL_RESULT_CAP = 800
_ASSISTANT_LABEL = "claude-code"


@dataclass(frozen=True)
class SessionMeta:
    """Overall metadata for a JSONL — useful for daemon-level cursors.

    For the per-day rows written to app_sessions, use `DaySlice` below.
    """
    session_uuid:    str             # filename stem; also `sessionId` in every record
    agent:           str             # 'claude_code' | 'codex'
    cwd:             Optional[str]   # working directory the session was launched from
    started_at:      str             # ISO-8601 UTC; first record's timestamp
    ended_at:        str             # ISO-8601 UTC; last record's timestamp
    user_turns:      int             # total non-meta, non-sidechain user messages
    assistant_turns: int             # total non-meta, non-sidechain assistant messages
    total_records:   int             # all parseable records (including UI state)
    jsonl_bytes:     int             # source-file size at parse time
    active_seconds:  int             # gap-capped active engagement time across ALL days

    @property
    def is_valid(self) -> bool:
        return (
            self.user_turns + self.assistant_turns > 0
            and bool(self.started_at)
            and bool(self.ended_at)
            and self.started_at <= self.ended_at
        )


@dataclass(frozen=True)
class DaySlice:
    """One calendar-day slice of a session, in the user's local TZ.

    The indexer writes one app_sessions row per slice — `day_utc` is
    the (local) calendar day key; `started_at` / `ended_at` are the
    first / last record timestamps WITHIN that day (still emitted as
    UTC ISO strings because the rest of meridian.db stores UTC).
    """
    session_uuid:    str
    agent:           str
    cwd:             Optional[str]
    day_utc:         str             # YYYY-MM-DD in the user's local TZ
    started_at:      str             # first record ts that fell into this day (ISO UTC)
    ended_at:        str             # last record ts that fell into this day  (ISO UTC)
    user_turns:      int             # turns within this day only
    assistant_turns: int
    active_seconds:  int             # gap-capped active time, credited to this day
    transcript:      str             # rendered transcript of just this day's records

    @property
    def is_valid(self) -> bool:
        return (
            self.user_turns + self.assistant_turns > 0
            and bool(self.started_at)
            and bool(self.ended_at)
        )


# ──────────────────────── Public API ───────────────────────────────────────────


def parse_session_slices(
    jsonl_path: Path,
    *,
    agent: Optional[str] = None,
    active_gap_cap_seconds: Optional[int] = None,
    local_tz: Optional[tzinfo] = None,
) -> Tuple[SessionMeta, List[DaySlice]]:
    """Single pass over the JSONL; returns (overall SessionMeta, sorted DaySlice list).

    `agent` defaults to inferring from the path (claude_code vs codex).
    `active_gap_cap_seconds` clamps each inter-record gap before adding
    it to that record's day-bucket active total. Gaps that straddle a
    day boundary are credited to the day of the LATER record.
    `local_tz` defaults to `config.LOCAL_TZ` (user's machine TZ).
    """
    # Lazy import to avoid circular dep at module load
    from coding_agent_indexer import config

    if agent is None:
        agent = _infer_agent(jsonl_path)
    if active_gap_cap_seconds is None:
        active_gap_cap_seconds = config.ACTIVE_TIME_GAP_CAP_SECONDS
    if local_tz is None:
        local_tz = config.LOCAL_TZ

    session_uuid = jsonl_path.stem
    cwd: Optional[str] = None
    started_overall: Optional[str] = None
    ended_overall: Optional[str] = None
    user_turns_overall = 0
    assistant_turns_overall = 0
    total_records = 0
    prev_dt: Optional[datetime] = None
    days: dict[str, _DayBuilder] = {}

    try:
        jsonl_bytes = jsonl_path.stat().st_size
    except OSError:
        jsonl_bytes = 0

    for record in _iter_records(jsonl_path):
        total_records += 1
        ts = record.get("timestamp")
        cur_dt: Optional[datetime] = None
        cur_day: Optional[str] = None

        if isinstance(ts, str):
            if started_overall is None:
                started_overall = ts
            ended_overall = ts
            try:
                cur_dt = _parse_iso(ts)
                cur_day = cur_dt.astimezone(local_tz).strftime("%Y-%m-%d")
            except (ValueError, TypeError):
                pass

        if cwd is None and isinstance(record.get("cwd"), str):
            cwd = record["cwd"]

        # Each record lands in exactly one day bucket.
        slot: Optional[_DayBuilder] = None
        if cur_day is not None:
            slot = days.get(cur_day)
            if slot is None:
                slot = _DayBuilder(day_utc=cur_day)
                days[cur_day] = slot
            if slot.started_at is None and isinstance(ts, str):
                slot.started_at = ts
            if isinstance(ts, str):
                slot.ended_at = ts

        # Active-time accumulation: cap each inter-record gap at
        # active_gap_cap_seconds. Gap is credited to the day of the
        # LATER record (cur_day) — so a gap that straddles midnight
        # gets clamped and assigned to the new day.
        if cur_dt is not None and prev_dt is not None and slot is not None:
            gap = (cur_dt - prev_dt).total_seconds()
            if gap > 0:
                slot.active_seconds += min(gap, active_gap_cap_seconds)
        if cur_dt is not None:
            prev_dt = cur_dt

        # Turn counting + transcript building — only real conversational
        # records (no sidechain sub-agents, no meta local-command echoes).
        if record.get("isSidechain") or record.get("isMeta"):
            continue
        rtype = record.get("type")
        if rtype not in ("user", "assistant"):
            continue
        if slot is None:
            continue                                       # record had no usable timestamp

        if rtype == "user":
            slot.user_turns += 1
            user_turns_overall += 1
        else:
            slot.assistant_turns += 1
            assistant_turns_overall += 1
        slot.records.append(record)

    # Render transcripts per slice (sorted by day for stable output).
    slices: List[DaySlice] = []
    for day_utc in sorted(days):
        b = days[day_utc]
        slices.append(DaySlice(
            session_uuid    = session_uuid,
            agent           = agent,
            cwd             = cwd,
            day_utc         = day_utc,
            started_at      = b.started_at or "",
            ended_at        = b.ended_at or "",
            user_turns      = b.user_turns,
            assistant_turns = b.assistant_turns,
            active_seconds  = int(b.active_seconds),
            transcript      = _render_records(b.records),
        ))

    meta = SessionMeta(
        session_uuid    = session_uuid,
        agent           = agent,
        cwd             = cwd,
        started_at      = started_overall or "",
        ended_at        = ended_overall or "",
        user_turns      = user_turns_overall,
        assistant_turns = assistant_turns_overall,
        total_records   = total_records,
        jsonl_bytes     = jsonl_bytes,
        active_seconds  = sum(s.active_seconds for s in slices),
    )
    return meta, slices


# ──────────────────────── Internals ────────────────────────────────────────────


@dataclass
class _DayBuilder:
    """Mutable accumulator while parsing — converted into a frozen DaySlice at the end."""
    day_utc:         str
    started_at:      Optional[str] = None
    ended_at:        Optional[str] = None
    user_turns:      int           = 0
    assistant_turns: int           = 0
    active_seconds:  float         = 0.0
    records:         List[dict]    = field(default_factory=list)


def _infer_agent(jsonl_path: Path) -> str:
    """Heuristic: which agent wrote this JSONL?"""
    parts = {p.name for p in jsonl_path.parents}
    if "projects" in parts and ".claude" in parts:
        return "claude_code"
    if "sessions" in parts and ".codex" in parts:
        return "codex"
    s = str(jsonl_path)
    if "/.claude/" in s:
        return "claude_code"
    if "/.codex/" in s:
        return "codex"
    return "unknown"


def _iter_records(path: Path) -> Iterator[dict]:
    """Yield each well-formed JSON record. Silent on partial writes / IO errors."""
    try:
        fh = path.open("rb")
    except (FileNotFoundError, PermissionError, OSError) as exc:
        log.debug("cannot open %s: %s", path, exc)
        return
    try:
        for raw in fh:
            try:
                yield json.loads(raw)
            except Exception:
                continue
    except OSError as exc:
        log.debug("read error on %s: %s", path, exc)
        return
    finally:
        try:
            fh.close()
        except Exception:
            pass


def _parse_iso(ts: str) -> datetime:
    """Parse the JSONL's ISO-8601 timestamp (with 'Z' suffix) into an aware datetime."""
    dt = datetime.fromisoformat(ts.replace("Z", "+00:00"))
    if dt.tzinfo is None:
        dt = dt.replace(tzinfo=timezone.utc)
    return dt


# ──────────────────────── Transcript rendering ─────────────────────────────────


def _render_records(records: List[dict]) -> str:
    """Flatten a pre-filtered list of user/assistant records to a transcript."""
    blocks: List[str] = []
    for rec in records:
        msg = rec.get("message") or {}
        rtype = rec.get("type")
        role_raw = msg.get("role") or rtype
        label = _ASSISTANT_LABEL if role_raw == "assistant" else role_raw
        body = _format_message_content(msg.get("content", ""))
        if body.strip():
            blocks.append(f"[{label}] {body}")
    return "\n\n".join(blocks)


def _format_message_content(content) -> str:
    """Render a single `message.content` (string or list of typed blocks) to text."""
    if isinstance(content, str):
        return content
    if not isinstance(content, list):
        return ""
    parts: List[str] = []
    for block in content:
        if not isinstance(block, dict):
            parts.append(str(block))
            continue
        btype = block.get("type")
        if btype == "text":
            parts.append(block.get("text", ""))
        elif btype == "tool_use":
            name = block.get("name", "?")
            inp = block.get("input")
            inp_repr = json.dumps(inp, ensure_ascii=False)[:400] if inp else ""
            parts.append(f"[tool_use: {name} {inp_repr}]".rstrip())
        elif btype == "tool_result":
            tr = block.get("content", "")
            if isinstance(tr, list):
                tr = "\n".join(
                    p.get("text", "") if isinstance(p, dict) else str(p)
                    for p in tr
                )
            tr_str = str(tr).strip()
            if len(tr_str) > _TOOL_RESULT_CAP:
                tr_str = tr_str[:_TOOL_RESULT_CAP] + "…[truncated]"
            parts.append(f"[tool_result: {tr_str}]")
        elif btype == "thinking":
            t = block.get("thinking", "")
            if t:
                parts.append(f"[thinking] {t}")
    return "\n".join(p for p in parts if p)
