"""The single entry point both the hook and the daemon use.

`register_ended_session(jsonl_path)` parses the JSONL into per-day
slices, resolves the host terminal/IDE once, and UPSERTs one
`app_sessions` row per slice. Idempotent at the DB layer — duplicate
calls update mutable fields in place, never raise.

A long-running session that spans multiple calendar days produces
multiple rows here, one per local day, so PM-update windowing
attributes time to the day the work actually happened.

This module deliberately knows nothing about how it was invoked: the
hook process passes a path it got from Claude Code's SessionEnd payload;
the poller passes a path it found by scanning. Both go through here.
"""
from __future__ import annotations

import logging
from dataclasses import dataclass, field
from enum import Enum
from pathlib import Path
from typing import List, Optional

from coding_agent_indexer import db, host_app
from coding_agent_indexer.jsonl_meta import (
    SessionMeta,
    parse_session_slices,
)

log = logging.getLogger(__name__)


class RegisterOutcome(str, Enum):
    INSERTED      = "inserted"        # at least one slice written (INSERT or UPDATE)
    SKIPPED_EMPTY = "skipped_empty"   # session has zero turns / no timestamps / no slices
    SKIPPED_FORK  = "skipped_fork"    # this is one of our summariser forks
    FAILED        = "failed"          # unexpected exception


@dataclass(frozen=True)
class RegisterResult:
    outcome:      RegisterOutcome
    session_uuid: str
    host_app:     str
    meta:         Optional[SessionMeta]                    # overall session metadata
    row_ids:      List[int] = field(default_factory=list)  # one id per day slice written
    error:        Optional[str] = None

    @property
    def row_id(self) -> Optional[int]:
        """First written row id, for backward compatibility with single-row callers."""
        return self.row_ids[0] if self.row_ids else None


def register_ended_session(
    jsonl_path: Path,
    *,
    fork_skip_list: Optional[set[str]] = None,
) -> RegisterResult:
    """Parse → resolve host → UPSERT one row per day slice.

    Idempotent. Always returns a result; never raises.

    Args:
        jsonl_path: Path to the session's JSONL file.
        fork_skip_list: Optional set of session_uuids the caller already
            knows are summariser forks (don't re-register them as user
            sessions). The daemon passes its in-memory skip-list here.

    Returns:
        `RegisterResult` describing the outcome. `row_ids` contains the
        id of every (uuid, day) row written or refreshed.
    """
    session_uuid = jsonl_path.stem

    if fork_skip_list and session_uuid in fork_skip_list:
        return _result(RegisterOutcome.SKIPPED_FORK, session_uuid, "", None)

    try:
        meta, slices = parse_session_slices(jsonl_path)
    except Exception as exc:                                 # noqa: BLE001
        log.exception("parse failed for %s", jsonl_path)
        return _result(RegisterOutcome.FAILED, session_uuid, "", None, error=str(exc))

    if not meta.is_valid or not slices:
        # Empty / metadata-only JSONLs are normal, especially for Codex
        # rollout artifacts. Keep this at DEBUG so the daemon log stays
        # high-signal; operators still get per-tick skipped counts.
        log.debug(
            "skip empty session: uuid=%s user_turns=%d asst_turns=%d slices=%d started=%r ended=%r",
            session_uuid, meta.user_turns, meta.assistant_turns,
            len(slices), meta.started_at, meta.ended_at,
        )
        return _result(RegisterOutcome.SKIPPED_EMPTY, session_uuid, "", meta)

    resolved_host = host_app.detect_host_app()

    # One UPSERT per day slice. Per-slice failures don't abort the rest —
    # we want partial progress on a session even if one day's slice is
    # malformed. Aggregate written ids for the result.
    row_ids: List[int] = []
    for slice_ in slices:
        try:
            row_id = db.upsert_session_day_slice(slice_)
        except Exception:                                    # noqa: BLE001
            log.exception("upsert failed for %s day=%s", session_uuid, slice_.day_utc)
            continue
        if row_id is not None:
            row_ids.append(row_id)
            log.info(
                "registered: agent=%s uuid=%s day=%s host=%s started=%s ended=%s "
                "active=%ds turns=%d/%d transcript_bytes=%d row_id=%d",
                slice_.agent, slice_.session_uuid, slice_.day_utc, resolved_host,
                slice_.started_at, slice_.ended_at, slice_.active_seconds,
                slice_.user_turns, slice_.assistant_turns,
                len(slice_.transcript), row_id,
            )

    if not row_ids:
        # All slices invalid OR all upserts failed — treat as no-op.
        return _result(RegisterOutcome.SKIPPED_EMPTY, session_uuid, resolved_host, meta)

    return _result(
        RegisterOutcome.INSERTED, session_uuid, resolved_host, meta, row_ids=row_ids,
    )


def _result(
    outcome:        RegisterOutcome,
    session_uuid:   str,
    host_app_name:  str,
    meta:           Optional[SessionMeta],
    *,
    row_ids:        Optional[List[int]] = None,
    error:          Optional[str] = None,
) -> RegisterResult:
    return RegisterResult(
        outcome      = outcome,
        session_uuid = session_uuid,
        host_app     = host_app_name,
        meta         = meta,
        row_ids      = list(row_ids) if row_ids else [],
        error        = error,
    )
