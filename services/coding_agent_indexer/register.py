"""The single entry point both the hook and the daemon use.

`register_ended_session(jsonl_path)` parses the JSONL into idle-gap
segments, resolves the host terminal/IDE once, and UPSERTs one
`app_sessions` row per segment — sealing the settled ones. Idempotent at
the DB layer: duplicate calls refresh live rows in place and no-op on
sealed rows, never raise.

Seal decision per segment:
  * Every segment EXCEPT the last is sealed — a later segment exists, so
    the >1h gap that ended it already happened.
  * The LAST segment is sealed iff the caller says the session ended
    (`session_ended=True`, the SessionEnd-hook path) OR its last message
    is already older than `config.SEGMENT_GAP_SECONDS` (settled by idle).
    Otherwise it stays LIVE and is refreshed on the next poll.

Before parsing, we fetch the session's sealed high-water mark and pass it
as `start_after_ts`, so already-sealed content is excluded and any newer
record opens a fresh segment — the invariant that keeps a session safe to
resume shortly after its SessionEnd hook fired.

Timestamps are UTC (ISO-8601 `...Z`), matching what the JSONLs store.

This module knows nothing about how it was invoked: the hook passes a path
from Claude Code's SessionEnd payload (session_ended defaults True); the
poller passes a path it scanned (session_ended=False).
"""
from __future__ import annotations

import logging
from dataclasses import dataclass, field
from datetime import datetime, timezone
from enum import Enum
from pathlib import Path
from typing import List, Optional

from coding_agent_indexer import config, db, host_app
from coding_agent_indexer.jsonl_meta import (
    Segment,
    SessionMeta,
    iso_utc,
    parse_session_segments,
)

log = logging.getLogger(__name__)


class RegisterOutcome(str, Enum):
    INSERTED      = "inserted"        # at least one segment written (INSERT or UPDATE)
    SKIPPED_EMPTY = "skipped_empty"   # no turns / no timestamps / no segments / all sealed
    SKIPPED_FORK  = "skipped_fork"    # this is one of our summariser forks
    FAILED        = "failed"          # unexpected exception


@dataclass(frozen=True)
class RegisterResult:
    outcome:      RegisterOutcome
    session_uuid: str
    host_app:     str
    meta:         Optional[SessionMeta]                    # overall session metadata
    row_ids:      List[int] = field(default_factory=list)  # one id per segment written
    sealed_ids:   List[int] = field(default_factory=list)  # subset of row_ids that were sealed
    error:        Optional[str] = None

    @property
    def row_id(self) -> Optional[int]:
        """First written row id, for backward compatibility with single-row callers."""
        return self.row_ids[0] if self.row_ids else None


def register_ended_session(
    jsonl_path: Path,
    *,
    session_ended: bool = True,
    fork_skip_list: Optional[set[str]] = None,
    now: Optional[datetime] = None,
) -> RegisterResult:
    """Parse → resolve host → UPSERT one row per segment, sealing settled ones.

    Idempotent. Always returns a result; never raises.

    Args:
        jsonl_path: Path to the session's JSONL file.
        session_ended: True when the caller knows the session ended (the
            SessionEnd-hook path; default) → the last segment seals
            immediately. The poller passes False so an actively-growing
            last segment stays live until it idles out.
        fork_skip_list: session_uuids known to be summariser forks — skip.
        now: reference time for the idle/seal check (default: utcnow).

    Returns:
        `RegisterResult`; `row_ids` = every segment row written/refreshed,
        `sealed_ids` = the subset that are now sealed.
    """
    session_uuid = jsonl_path.stem

    if fork_skip_list and session_uuid in fork_skip_list:
        return _result(RegisterOutcome.SKIPPED_FORK, session_uuid, "", None)

    now_dt = now or datetime.now(timezone.utc)
    now_iso = _utc_iso(now_dt)

    try:
        start_after = db.sealed_high_water(session_uuid)
        meta, segments = parse_session_segments(jsonl_path, start_after_ts=start_after)
    except Exception as exc:                                 # noqa: BLE001
        log.exception("parse failed for %s", jsonl_path)
        return _result(RegisterOutcome.FAILED, session_uuid, "", None, error=str(exc))

    valid = [s for s in segments if s.is_valid]
    if not valid:
        # Empty / metadata-only / fully-sealed-already JSONLs are normal.
        # DEBUG keeps the daemon log high-signal; per-tick counts still show.
        log.debug(
            "skip empty session: uuid=%s user=%d asst=%d segments=%d started=%r ended=%r",
            session_uuid, meta.user_turns, meta.assistant_turns,
            len(segments), meta.started_at, meta.ended_at,
        )
        return _result(RegisterOutcome.SKIPPED_EMPTY, session_uuid, "", meta)

    resolved_host = host_app.detect_host_app()

    # One UPSERT per segment. Per-segment failures don't abort the rest.
    row_ids: List[int] = []
    sealed_ids: List[int] = []
    for seg in valid:
        sealed = _should_seal(seg, session_ended=session_ended, now_dt=now_dt)
        try:
            row_id = db.upsert_segment(
                seg, sealed=sealed, sealed_at=now_iso if sealed else None,
            )
        except Exception:                                    # noqa: BLE001
            log.exception("upsert failed for %s seg=%s", session_uuid, seg.segment_started_at)
            continue
        if row_id is None:
            continue
        row_ids.append(row_id)
        if sealed:
            sealed_ids.append(row_id)
        log.info(
            "registered: agent=%s uuid=%s seg=%s host=%s ended=%s "
            "active=%ds turns=%d/%d bytes=%d sealed=%s row_id=%d",
            seg.agent, seg.session_uuid, seg.segment_started_at,
            resolved_host, seg.ended_at, seg.active_seconds,
            seg.user_turns, seg.assistant_turns, len(seg.transcript),
            sealed, row_id,
        )

    if not row_ids:
        # Everything we tried was a no-op (e.g. all keys hit already-sealed rows).
        return _result(RegisterOutcome.SKIPPED_EMPTY, session_uuid, resolved_host, meta)

    return _result(
        RegisterOutcome.INSERTED, session_uuid, resolved_host, meta,
        row_ids=row_ids, sealed_ids=sealed_ids,
    )


# ──────────────────────── Internals ────────────────────────────────────────────


def _should_seal(seg: Segment, *, session_ended: bool, now_dt: datetime) -> bool:
    """A non-last segment is always sealed; the last seals on end-or-idle."""
    if not seg.is_last:
        return True
    if session_ended:
        return True
    try:
        ended = datetime.fromisoformat(seg.ended_at.replace("Z", "+00:00"))
        if ended.tzinfo is None:
            ended = ended.replace(tzinfo=timezone.utc)
    except (ValueError, TypeError):
        return False                                         # can't determine idleness → keep live
    idle = (now_dt - ended).total_seconds()
    return idle > config.SEGMENT_GAP_SECONDS


def _utc_iso(dt: datetime) -> str:
    """Canonical UTC timestamp (microseconds + '+00:00'); see jsonl_meta.iso_utc."""
    return iso_utc(dt)


def _result(
    outcome:        RegisterOutcome,
    session_uuid:   str,
    host_app_name:  str,
    meta:           Optional[SessionMeta],
    *,
    row_ids:        Optional[List[int]] = None,
    sealed_ids:     Optional[List[int]] = None,
    error:          Optional[str] = None,
) -> RegisterResult:
    return RegisterResult(
        outcome      = outcome,
        session_uuid = session_uuid,
        host_app     = host_app_name,
        meta         = meta,
        row_ids      = list(row_ids) if row_ids else [],
        sealed_ids   = list(sealed_ids) if sealed_ids else [],
        error        = error,
    )
