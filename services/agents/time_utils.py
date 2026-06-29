"""
Local-timezone helpers for the Meridian agent services.

All time-zone conversion in the Python services should go through this module so
that the logic lives in one place and is easy to update (e.g. if we ever need to
support a user-configurable timezone rather than the machine's local tz).

Public API
----------
LOCAL_TZ                   — the machine's current local timezone
local_hour_utc_bounds(h)   — local hour label → (utc_start, utc_end) ISO strings
utc_to_local_hhmm(s)       — UTC ISO datetime string → local "HH:MM"
utc_to_local_dt(s)         — UTC ISO datetime string → aware local datetime
"""
from __future__ import annotations

from datetime import datetime, timedelta, timezone

# Resolved once at import time; cheap and safe for a long-running process.
LOCAL_TZ = datetime.now().astimezone().tzinfo


def local_hour_utc_bounds(hour: str) -> tuple[str, str]:
    """Convert a local-time hour label (``YYYY-MM-DDTHH``) to UTC [start, end) ISO strings.

    The DB stores ``started_at`` in UTC; callers use these bounds for range queries
    so that the pipeline operates on the user's local clock hour, not UTC.

    Example (IST = UTC+5:30)::

        local_hour_utc_bounds("2026-06-28T10")
        # → ("2026-06-28T04:30:00", "2026-06-28T05:30:00")
    """
    local_start = datetime.strptime(hour, "%Y-%m-%dT%H").replace(tzinfo=LOCAL_TZ)
    utc_start = local_start.astimezone(timezone.utc)
    utc_end = (local_start + timedelta(hours=1)).astimezone(timezone.utc)
    return utc_start.strftime("%Y-%m-%dT%H:%M:%S"), utc_end.strftime("%Y-%m-%dT%H:%M:%S")


def utc_to_local_hhmm(utc_iso: str) -> str:
    """Convert a UTC ISO datetime string to local ``HH:MM``.

    Accepts strings with or without a trailing ``Z`` or ``+00:00`` suffix.
    Falls back to the raw ``[11:16]`` slice on any parse error.
    """
    try:
        s = utc_iso.rstrip("Z").replace("+00:00", "")
        return datetime.fromisoformat(s).replace(tzinfo=timezone.utc).astimezone(LOCAL_TZ).strftime("%H:%M")
    except Exception:
        return utc_iso[11:16]


def utc_to_local_dt(utc_iso: str) -> datetime:
    """Convert a UTC ISO datetime string to an aware local :class:`datetime`."""
    s = utc_iso.rstrip("Z").replace("+00:00", "")
    return datetime.fromisoformat(s).replace(tzinfo=timezone.utc).astimezone(LOCAL_TZ)
