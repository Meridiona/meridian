"""SQLite connection helpers shared across all db submodules."""
from __future__ import annotations

import json
import sqlite3
from contextlib import contextmanager
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

from agents.config import MERIDIAN_DB


def _utc_now() -> str:
    return datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def _connect() -> sqlite3.Connection:
    path = Path(MERIDIAN_DB).expanduser()
    if not path.exists():
        raise FileNotFoundError(
            f"meridian.db not found at {path}. Is the Meridian daemon running?"
        )
    conn = sqlite3.connect(str(path), isolation_level=None, timeout=10.0)
    conn.row_factory = sqlite3.Row
    conn.execute("PRAGMA journal_mode=WAL;")
    conn.execute("PRAGMA foreign_keys=ON;")
    return conn


@contextmanager
def connection():
    conn = _connect()
    try:
        yield conn
    finally:
        conn.close()


def _json_or_none(val: Any) -> Any:
    if val in (None, "", "null"):
        return None
    if isinstance(val, (dict, list)):
        return val
    try:
        return json.loads(val)
    except (json.JSONDecodeError, TypeError):
        return None
