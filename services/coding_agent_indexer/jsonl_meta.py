"""Parse a coding-agent JSONL once and return idle-gap session segments.

A Claude Code or Codex session writes its conversation as an append-only
.jsonl file. A single file can hold several distinct work bursts separated by
long idle gaps (lunch, a meeting, picking the session back up the next morning
via `claude --continue` / `codex resume`). We slice the file into SEGMENTS
split on idle gaps larger than `config.SEGMENT_GAP_SECONDS` (default 1h): each
continuous burst becomes one `app_sessions` row.

Claude and Codex use different on-disk event schemas, so each record is first
normalised to a common `_NormRecord` (timestamp, cwd, is_turn, is_user, body).
Everything downstream — segmentation, active-time, the timestamped transcript —
is agent-agnostic.

Timestamps are stored as the JSONL's own UTC ISO-8601 (`...Z`); callers derive
is bucketed in the user's local TZ (for PM-update windowing).

Public surface:
  * `parse_session_segments()` — single pass; returns the overall `SessionMeta`
    plus a list of `Segment` (one per work burst). The indexer UPSERTs one row
    per segment and seals settled ones.

`start_after_ts` excludes content already captured by a sealed segment: any
record at or before it is ignored and the first record after it begins a fresh
segment regardless of gap — the "sealed content is immutable; newer is a new
segment" invariant.

Tolerant: malformed lines, missing fields, files truncated mid-write, and files
with zero meaningful records all degrade gracefully — each segment's `is_valid`
flag tells the caller whether to register it.
"""
from __future__ import annotations

import json
import logging
from dataclasses import dataclass, field
from datetime import datetime, timezone, tzinfo
from pathlib import Path
from typing import Iterator, List, Optional, Tuple

log = logging.getLogger(__name__)

# Truncation for noisy tool_result bodies (file dumps, large search outputs).
_TOOL_RESULT_CAP = 800
_CLAUDE_ASSISTANT_LABEL = "claude-code"
_CODEX_ASSISTANT_LABEL = "codex"


@dataclass(frozen=True)
class SessionMeta:
    """Overall metadata for a JSONL — useful for daemon-level cursors.

    For the per-segment rows written to app_sessions, use `Segment` below.
    """
    session_uuid:    str
    agent:           str             # 'claude_code' | 'codex'
    cwd:             Optional[str]
    started_at:      str
    ended_at:        str
    user_turns:      int
    assistant_turns: int
    total_records:   int
    jsonl_bytes:     int
    active_seconds:  int

    @property
    def is_valid(self) -> bool:
        return (
            self.user_turns + self.assistant_turns > 0
            and bool(self.started_at)
            and bool(self.ended_at)
            and self.started_at <= self.ended_at
        )


@dataclass(frozen=True)
class Segment:
    """One continuous work burst of a session (gaps < SEGMENT_GAP_SECONDS).

    One app_sessions row per segment, keyed on (claude_session_uuid,
    segment_started_at). `is_last` marks the final segment — the only one that
    may still be live (unsealed).
    """
    session_uuid:       str
    agent:              str
    cwd:                Optional[str]
    segment_started_at: str          # first record ts in this segment (ISO UTC) — the key
    started_at:         str          # == segment_started_at
    ended_at:           str          # last record ts in this segment (ISO UTC)
    user_turns:         int
    assistant_turns:    int
    active_seconds:     int
    transcript:         str
    is_last:            bool

    @property
    def is_valid(self) -> bool:
        return (
            self.user_turns + self.assistant_turns > 0
            and bool(self.segment_started_at)
            and bool(self.ended_at)
        )


# ──────────────────────── Public API ───────────────────────────────────────────


def parse_session_segments(
    jsonl_path: Path,
    *,
    agent: Optional[str] = None,
    active_gap_cap_seconds: Optional[int] = None,
    segment_gap_seconds: Optional[int] = None,
    max_segment_seconds: Optional[int] = None,
    local_tz: Optional[tzinfo] = None,
    start_after_ts: Optional[str] = None,
) -> Tuple[SessionMeta, List[Segment]]:
    """Single pass over the JSONL; returns (overall SessionMeta, Segment list)."""
    from coding_agent_indexer import config

    if agent is None:
        agent = _infer_agent(jsonl_path)
    if active_gap_cap_seconds is None:
        active_gap_cap_seconds = config.ACTIVE_TIME_GAP_CAP_SECONDS
    if segment_gap_seconds is None:
        segment_gap_seconds = config.SEGMENT_GAP_SECONDS
    if max_segment_seconds is None:
        max_segment_seconds = config.MAX_SEGMENT_SECONDS
    if local_tz is None:
        local_tz = config.LOCAL_TZ

    start_after_dt: Optional[datetime] = None
    if start_after_ts:
        try:
            start_after_dt = _parse_iso(start_after_ts)
        except (ValueError, TypeError):
            start_after_dt = None

    session_uuid = jsonl_path.stem
    cwd: Optional[str] = None
    started_overall: Optional[str] = None
    ended_overall: Optional[str] = None
    user_turns_overall = 0
    assistant_turns_overall = 0
    total_records = 0
    prev_dt: Optional[datetime] = None          # ts of previous KEPT record (gap calc)
    seg_start_dt: Optional[datetime] = None      # start ts of the current segment (time-box)
    segments_b: List[_SegBuilder] = []
    cur: Optional[_SegBuilder] = None

    try:
        jsonl_bytes = jsonl_path.stat().st_size
    except OSError:
        jsonl_bytes = 0

    for rec in _iter_normalised(jsonl_path, agent=agent):
        total_records += 1
        ts = rec.timestamp
        if cwd is None and rec.cwd:
            cwd = rec.cwd

        cur_dt: Optional[datetime] = None
        if isinstance(ts, str):
            try:
                cur_dt = _parse_iso(ts)
            except (ValueError, TypeError):
                cur_dt = None

        # Already-sealed content is immutable history — skip it and reset the
        # gap anchor so the first kept record opens a fresh segment.
        if cur_dt is not None and start_after_dt is not None and cur_dt <= start_after_dt:
            prev_dt = None
            continue

        if cur_dt is not None:
            if started_overall is None:
                started_overall = ts
            ended_overall = ts

        # Records with no usable timestamp can't anchor a segment; attach to the
        # current one if it exists (so a body isn't lost), else drop.
        if cur_dt is None:
            if cur is not None and rec.is_turn:
                _add_turn(cur, ts, rec)
            continue

        start_new = (
            cur is None
            or prev_dt is None
            or (cur_dt - prev_dt).total_seconds() > segment_gap_seconds
            # Time-box: once a segment has run for max_segment_seconds, split — but
            # only AT THE NEXT REAL USER PROMPT, so the prior row ends on a complete
            # assistant turn and the new row opens on a user message (continuity).
            # Tool-result `user` records don't count (is_user_prompt is False).
            or (
                max_segment_seconds > 0
                and seg_start_dt is not None
                and (cur_dt - seg_start_dt).total_seconds() >= max_segment_seconds
                and rec.is_user_prompt
            )
        )
        if start_new:
            cur = _SegBuilder(
                segment_started_at=ts,
            )
            segments_b.append(cur)
            seg_start_dt = cur_dt
        else:
            gap = (cur_dt - prev_dt).total_seconds()
            if gap > 0:
                cur.active_seconds += min(gap, active_gap_cap_seconds)

        cur.ended_at = ts
        prev_dt = cur_dt

        if rec.is_turn:
            _add_turn(cur, ts, rec)
            if rec.is_user:
                user_turns_overall += 1
            else:
                assistant_turns_overall += 1

    segments: List[Segment] = []
    for i, b in enumerate(segments_b):
        segments.append(Segment(
            session_uuid       = session_uuid,
            agent              = agent,
            cwd                = cwd,
            segment_started_at = norm_iso(b.segment_started_at),
            started_at         = norm_iso(b.segment_started_at),
            ended_at           = norm_iso(b.ended_at or b.segment_started_at),
            user_turns         = b.user_turns,
            assistant_turns    = b.assistant_turns,
            active_seconds     = int(b.active_seconds),
            transcript         = _render_records(b.records),
            is_last            = (i == len(segments_b) - 1),
        ))

    meta = SessionMeta(
        session_uuid    = session_uuid,
        agent           = agent,
        cwd             = cwd,
        started_at      = norm_iso(started_overall) if started_overall else "",
        ended_at        = norm_iso(ended_overall) if ended_overall else "",
        user_turns      = user_turns_overall,
        assistant_turns = assistant_turns_overall,
        total_records   = total_records,
        jsonl_bytes     = jsonl_bytes,
        active_seconds  = sum(s.active_seconds for s in segments),
    )
    return meta, segments


# ──────────────────────── Canonical record ─────────────────────────────────────


@dataclass(frozen=True)
class _NormRecord:
    """One raw JSONL record normalised across Claude and Codex schemas."""
    timestamp:  Optional[str]
    cwd:        Optional[str]
    is_turn:    bool
    is_user:    bool
    role_label: Optional[str]
    body:       str
    # True only for a REAL human prompt (text), not a tool-result (Claude logs
    # tool outputs as type:user too). The time-box split aligns to this so a
    # segment begins on a genuine user message.
    is_user_prompt: bool = False


def _is_real_user_prompt(content) -> bool:
    """A genuine prompt (has text) vs a tool-result-only user message."""
    if isinstance(content, str):
        return bool(content.strip())
    if isinstance(content, list):
        return any(isinstance(b, dict) and b.get("type") == "text" for b in content)
    return False


@dataclass
class _SegBuilder:
    """Mutable accumulator while parsing — frozen into a Segment at the end."""
    segment_started_at: str
    ended_at:           Optional[str] = None
    user_turns:         int           = 0
    assistant_turns:    int           = 0
    active_seconds:     float         = 0.0
    records:            List[Tuple[Optional[str], _NormRecord]] = field(default_factory=list)


def _add_turn(builder: _SegBuilder, ts: Optional[str], rec: _NormRecord) -> None:
    if rec.is_user:
        builder.user_turns += 1
    else:
        builder.assistant_turns += 1
    builder.records.append((ts, rec))


def _infer_agent(jsonl_path: Path) -> str:
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
    """Yield each well-formed JSON object. Silent on partial writes / IO errors."""
    try:
        fh = path.open("rb")
    except (FileNotFoundError, PermissionError, OSError) as exc:
        log.debug("cannot open %s: %s", path, exc)
        return
    try:
        for raw in fh:
            try:
                obj = json.loads(raw)
            except Exception:
                continue
            if isinstance(obj, dict):
                yield obj
    except OSError as exc:
        log.debug("read error on %s: %s", path, exc)
        return
    finally:
        try:
            fh.close()
        except Exception:
            pass


def _iter_normalised(path: Path, *, agent: str) -> Iterator[_NormRecord]:
    """Yield canonical records for one source JSONL (agent-aware)."""
    if agent == "codex":
        yield from _iter_codex(path)
    else:
        yield from _iter_claude(path)


def _iter_claude(path: Path) -> Iterator[_NormRecord]:
    """Normalise Claude Code records. Sidechain/meta carry a timestamp (so they
    participate in gap/segment accounting) but are not conversational turns."""
    for raw in _iter_records(path):
        ts = raw.get("timestamp") if isinstance(raw.get("timestamp"), str) else None
        cwd = raw.get("cwd") if isinstance(raw.get("cwd"), str) else None
        rtype = raw.get("type")

        if raw.get("isSidechain") or raw.get("isMeta") or rtype not in ("user", "assistant"):
            yield _NormRecord(ts, cwd, is_turn=False, is_user=False, role_label=None, body="")
            continue

        msg = raw.get("message") or {}
        role_raw = msg.get("role") or rtype
        is_user = role_raw == "user"
        label = role_raw if is_user else _CLAUDE_ASSISTANT_LABEL
        content = msg.get("content", "")
        yield _NormRecord(
            ts, cwd, is_turn=True, is_user=is_user, role_label=label,
            body=_format_claude_content(content),
            is_user_prompt=is_user and _is_real_user_prompt(content),
        )


def _iter_codex(path: Path) -> Iterator[_NormRecord]:
    """Normalise Codex rollout records. Conversational turns are the
    `event_msg` `user_message` / `agent_message` events; everything else
    (session_meta, response_item, turn_context, token_count, …) is non-turn
    but still carries a timestamp for gap/segment accounting."""
    for raw in _iter_records(path):
        ts = raw.get("timestamp") if isinstance(raw.get("timestamp"), str) else None
        payload = raw.get("payload") if isinstance(raw.get("payload"), dict) else {}
        cwd = payload.get("cwd") if isinstance(payload.get("cwd"), str) else None

        if raw.get("type") != "event_msg":
            yield _NormRecord(ts, cwd, is_turn=False, is_user=False, role_label=None, body="")
            continue

        sub = payload.get("type")
        if sub == "user_message":
            yield _NormRecord(ts, cwd, is_turn=True, is_user=True, role_label="user",
                              body=_format_codex_message(payload.get("message", "")),
                              is_user_prompt=True)
        elif sub == "agent_message":
            yield _NormRecord(ts, cwd, is_turn=True, is_user=False, role_label=_CODEX_ASSISTANT_LABEL,
                              body=_format_codex_message(payload.get("message", "")))
        else:
            yield _NormRecord(ts, cwd, is_turn=False, is_user=False, role_label=None, body="")


def _parse_iso(ts: str) -> datetime:
    dt = datetime.fromisoformat(ts.replace("Z", "+00:00"))
    if dt.tzinfo is None:
        dt = dt.replace(tzinfo=timezone.utc)
    return dt


def iso_utc(dt: datetime) -> str:
    """Canonical app_sessions timestamp: ISO-8601 UTC, microseconds, '+00:00'.

    Matches the Rust ETL's chrono RFC3339 output so every started_at / ended_at /
    segment_started_at in the table shares one lexical shape. A UTC offset string
    is fixed-width, so plain string comparison stays chronological.
    """
    return dt.astimezone(timezone.utc).strftime("%Y-%m-%dT%H:%M:%S.%f+00:00")


def norm_iso(ts: str) -> str:
    """Normalise any ISO string (…Z / …±offset, any precision) to iso_utc().

    Falls back to the input unchanged if it can't be parsed, so a malformed
    source timestamp is never silently dropped.
    """
    try:
        return iso_utc(_parse_iso(ts))
    except (ValueError, TypeError):
        return ts


# ──────────────────────── Transcript rendering ─────────────────────────────────


def _render_records(records: List[Tuple[Optional[str], _NormRecord]]) -> str:
    """Flatten (timestamp, record) pairs to a timestamped transcript:
    `[<ISO ts>] [role] body` per turn, so the summariser can reason about time."""
    blocks: List[str] = []
    for ts, rec in records:
        if rec.body.strip():
            prefix = f"[{ts}] " if ts else ""
            blocks.append(f"{prefix}[{rec.role_label or 'user'}] {rec.body}")
    return "\n\n".join(blocks)


def _format_claude_content(content) -> str:
    """Render Claude `message.content` (string or typed blocks) to text."""
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
                    p.get("text", "") if isinstance(p, dict) else str(p) for p in tr
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


def _format_codex_message(content) -> str:
    """Render a Codex event_msg message payload into plain transcript text."""
    if isinstance(content, str):
        return content
    if not isinstance(content, list):
        return ""
    parts: List[str] = []
    for block in content:
        if isinstance(block, dict):
            text = block.get("text")
            if isinstance(text, str) and text:
                parts.append(text)
                continue
            nested = block.get("content")
            if isinstance(nested, str) and nested:
                parts.append(nested)
        elif isinstance(block, str):
            parts.append(block)
    return "\n".join(p for p in parts if p)
