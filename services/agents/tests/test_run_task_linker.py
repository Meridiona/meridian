# meridian — normalises screenpipe activity into structured app sessions
"""Tests for agents.run_task_linker — input validation and error handling.

These tests invoke the module via subprocess so that stdin/stdout/exit-code
behaviour is validated at the process boundary (the same interface Rust uses).
"""
from __future__ import annotations

import json
import sqlite3
import subprocess
import sys
import tempfile
from pathlib import Path

import pytest

# Path to the services directory so we can invoke the module
_SERVICES_DIR = Path(__file__).resolve().parent.parent.parent


def _run(payload: dict, *, timeout: int = 15) -> subprocess.CompletedProcess:
    """Run python -m agents.run_task_linker with payload on stdin."""
    return subprocess.run(
        [sys.executable, "-m", "agents.run_task_linker"],
        input=json.dumps(payload),
        capture_output=True,
        text=True,
        cwd=str(_SERVICES_DIR),
        timeout=timeout,
    )


def _make_minimal_db() -> str:
    """Create a temp SQLite file with the minimal schema run_task_linker needs."""
    tmp = tempfile.NamedTemporaryFile(suffix=".db", delete=False)
    tmp.close()
    con = sqlite3.connect(tmp.name)
    con.executescript("""
        CREATE TABLE IF NOT EXISTS app_sessions (
            id INTEGER PRIMARY KEY,
            app_name TEXT,
            started_at TEXT,
            ended_at TEXT,
            duration_s REAL,
            session_text TEXT,
            session_text_source TEXT,
            window_titles TEXT,
            category TEXT,
            confidence REAL,
            task_key TEXT,
            task_routing TEXT
        );
        CREATE TABLE IF NOT EXISTS pm_tasks (
            task_key TEXT PRIMARY KEY,
            title TEXT,
            description_text TEXT,
            status TEXT,
            status_category TEXT,
            issue_type TEXT,
            epic_title TEXT,
            sprint_name TEXT
        );
    """)
    con.close()
    return tmp.name


# ── empty session_ids ──────────────────────────────────────────────────────────

def test_empty_session_ids_exits_cleanly():
    """Empty session_ids list must write {"results": []} to stdout and exit 0."""
    db = _make_minimal_db()
    try:
        proc = _run({"session_ids": [], "meridian_db": db})
        assert proc.returncode == 0, f"stderr: {proc.stderr[:500]}"
        out = json.loads(proc.stdout.strip())
        assert out == {"results": []}
    finally:
        Path(db).unlink(missing_ok=True)


# ── missing / empty db_path ────────────────────────────────────────────────────

def test_missing_db_path_exits_with_error():
    """An empty meridian_db string must cause a non-zero exit."""
    proc = _run({"session_ids": [1], "meridian_db": ""})
    assert proc.returncode != 0, "expected non-zero exit when db_path is empty"


def test_none_db_path_exits_with_error():
    """A missing meridian_db key must cause a non-zero exit."""
    proc = _run({"session_ids": [1]})
    assert proc.returncode != 0, "expected non-zero exit when meridian_db key is absent"


# ── nonexistent DB file ────────────────────────────────────────────────────────

def test_nonexistent_db_exits_with_error():
    """A db path that does not exist on disk must cause a non-zero exit."""
    proc = _run({"session_ids": [1], "meridian_db": "/nonexistent/path/meridian.db"})
    assert proc.returncode != 0, "expected non-zero exit for nonexistent db file"


# ── session not found in DB ────────────────────────────────────────────────────

def test_session_not_in_db_returns_llm_error():
    """When the session_id is absent from app_sessions, the result method is llm_error.

    This exercises the _classify_one path where session_raw is None without
    actually invoking the hermes AIAgent — the DB open succeeds but the row
    lookup returns nothing.
    """
    db = _make_minimal_db()
    try:
        # We call _classify_one directly (no hermes) by importing the module.
        # sys.path must include services/ — conftest ensures that.
        import importlib
        import os
        # Ensure HERMES_HOME is set to avoid FileNotFoundError in hermes on import
        os.environ.setdefault("HERMES_HOME", str(_SERVICES_DIR / ".hermes"))

        from agents.run_task_linker import _classify_one
        import sqlite3 as sl

        con = sl.connect(db)
        con.row_factory = sl.Row
        try:
            result = _classify_one(session_id=9999, db_path=db, con=con)
        finally:
            con.close()

        assert result["session_id"] == 9999
        assert result["method"] == "llm_error"
        assert result["task_key"] is None
        assert "not found" in result["reasoning"]
    finally:
        Path(db).unlink(missing_ok=True)


# ── malformed stdin ────────────────────────────────────────────────────────────

def test_malformed_stdin_exits_with_error():
    """Non-JSON stdin must cause a non-zero exit."""
    proc = subprocess.run(
        [sys.executable, "-m", "agents.run_task_linker"],
        input="this is not json",
        capture_output=True,
        text=True,
        cwd=str(_SERVICES_DIR),
        timeout=15,
    )
    assert proc.returncode != 0
