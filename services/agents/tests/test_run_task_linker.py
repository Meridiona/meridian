# meridian — normalises screenpipe activity into structured app sessions
"""Tests for agents.run_task_linker — LLM selection, input validation, and core classification.

Protocol: Rust sends {"session_ids": [int, ...], "meridian_db": str} via stdin.
Python classifies and returns {"results": [...]} on stdout.

All tests stub hermes (AIAgent) and llm_selector so no real LLM is needed.
"""
from __future__ import annotations

import json
import sqlite3
import subprocess
import sys
import tempfile
from pathlib import Path
from unittest.mock import MagicMock, patch

import pytest

_SERVICES_DIR = Path(__file__).resolve().parent.parent.parent

# ── helpers ───────────────────────────────────────────────────────────────────

def _make_db_with_sessions(
    sessions: list[dict],
    pm_tasks: list[dict] | None = None,
) -> str:
    """Create a temp meridian.db, insert sessions and pm_tasks, return path."""
    f = tempfile.NamedTemporaryFile(suffix=".db", delete=False)
    con = sqlite3.connect(f.name)
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
            parent_key TEXT,
            epic_title TEXT,
            sprint_name TEXT
        );
    """)
    for s in sessions:
        con.execute(
            "INSERT INTO app_sessions (id, app_name, started_at, ended_at, duration_s,"
            " session_text, session_text_source, window_titles, category, confidence)"
            " VALUES (:id, :app_name, :started_at, :ended_at, :duration_s,"
            " :session_text, :session_text_source, :window_titles, :category, :confidence)",
            {
                "id":                  s.get("id", 1),
                "app_name":            s.get("app_name", ""),
                "started_at":          s.get("started_at", "2024-01-01T09:00:00"),
                "ended_at":            s.get("ended_at", "2024-01-01T09:01:00"),
                "duration_s":          s.get("duration_s", 60),
                "session_text":        s.get("session_text", ""),
                "session_text_source": s.get("session_text_source", "ocr"),
                "window_titles":       json.dumps(s.get("window_titles", [])),
                "category":            s.get("category", ""),
                "confidence":          s.get("confidence", 0.0),
            },
        )
    for t in (pm_tasks or []):
        con.execute(
            "INSERT INTO pm_tasks (task_key, title, description_text, status,"
            " status_category, issue_type, epic_title, sprint_name)"
            " VALUES (:task_key, :title, :description_text, :status,"
            " :status_category, :issue_type, :epic_title, :sprint_name)",
            {
                "task_key":         t.get("task_key", "KAN-1"),
                "title":            t.get("title", ""),
                "description_text": t.get("description_text", ""),
                "status":           t.get("status", "In Progress"),
                "status_category":  t.get("status_category", "in_progress"),
                "issue_type":       t.get("issue_type", "Task"),
                "epic_title":       t.get("epic_title", ""),
                "sprint_name":      t.get("sprint_name", ""),
            },
        )
    con.commit()
    con.close()
    return f.name


def _fake_agent_response(task_key: str = "KAN-1", confidence: float = 0.9) -> MagicMock:
    raw = json.dumps({
        "task_key":     task_key,
        "confidence":   confidence,
        "session_type": "active",
        "reasoning":    "stub reasoning",
        "dimensions":   {"activity": ["coding"]},
    })
    mock = MagicMock()
    mock.run_conversation.return_value = {"final_response": raw}
    return mock


# ── _resolve_llm: local endpoint selected ─────────────────────────────────────

def test_resolve_llm_uses_local_when_prefer_local_and_endpoint_found():
    from agents.llm_selector import LocalModelEndpoint
    ep = LocalModelEndpoint(model="gemma3-12b", base_url="http://127.0.0.1:11434/v1",
                            api_key="local", runtime="ollama")
    with patch("agents.run_task_linker.LLM_PREFER_LOCAL", True), \
         patch("agents.run_task_linker.select_model_for_hermes", return_value=ep):
        from agents.run_task_linker import _resolve_llm
        model, base_url, api_key = _resolve_llm()
    assert model == "gemma3-12b"
    assert base_url == "http://127.0.0.1:11434/v1"
    assert api_key == "local"


def test_resolve_llm_falls_back_to_cloud_when_no_local_endpoint():
    with patch("agents.run_task_linker.LLM_PREFER_LOCAL", True), \
         patch("agents.run_task_linker.select_model_for_hermes", return_value=None), \
         patch("agents.run_task_linker.MODEL", "gemma4:31b-cloud"), \
         patch("agents.run_task_linker.BASE_URL", "https://ollama.com/v1"), \
         patch("agents.run_task_linker.API_KEY", "test-key"):
        from agents.run_task_linker import _resolve_llm
        model, base_url, api_key = _resolve_llm()
    assert model == "gemma4:31b-cloud"
    assert base_url == "https://ollama.com/v1"
    assert api_key == "test-key"


def test_resolve_llm_falls_back_when_selector_raises():
    with patch("agents.run_task_linker.LLM_PREFER_LOCAL", True), \
         patch("agents.run_task_linker.select_model_for_hermes",
               side_effect=RuntimeError("probe failed")), \
         patch("agents.run_task_linker.MODEL", "gemma4:31b-cloud"), \
         patch("agents.run_task_linker.BASE_URL", "https://ollama.com/v1"), \
         patch("agents.run_task_linker.API_KEY", "key"):
        from agents.run_task_linker import _resolve_llm
        model, base_url, api_key = _resolve_llm()
    assert model == "gemma4:31b-cloud"


def test_resolve_llm_skips_selector_when_prefer_local_disabled():
    with patch("agents.run_task_linker.LLM_PREFER_LOCAL", False), \
         patch("agents.run_task_linker.select_model_for_hermes") as mock_sel, \
         patch("agents.run_task_linker.MODEL", "cloud-model"), \
         patch("agents.run_task_linker.BASE_URL", "https://cloud.example/v1"), \
         patch("agents.run_task_linker.API_KEY", ""):
        from agents.run_task_linker import _resolve_llm
        model, base_url, api_key = _resolve_llm()
    mock_sel.assert_not_called()
    assert model == "cloud-model"


# ── _classify_one: agent is injected, not constructed internally ──────────────

def test_classify_one_uses_provided_agent():
    """_classify_one calls agent.run_conversation and returns the parsed result."""
    db_path = _make_db_with_sessions(
        [{"id": 10, "app_name": "VSCode", "duration_s": 90}],
        pm_tasks=[{"task_key": "KAN-42", "title": "Feature work"}],
    )
    con = sqlite3.connect(db_path)
    con.row_factory = sqlite3.Row
    fake_agent = _fake_agent_response("KAN-42", 0.9)

    from agents.run_task_linker import _classify_one
    result = _classify_one(
        10, db_path, con,
        agent=fake_agent,
        llm_model="gemma3-12b",
        llm_base_url="http://127.0.0.1:11434/v1",
    )

    con.close()
    fake_agent.run_conversation.assert_called_once()
    assert result["task_key"] == "KAN-42"
    assert result["session_id"] == 10


def test_classify_one_returns_expected_fields():
    db_path = _make_db_with_sessions(
        [{"id": 20, "app_name": "Xcode", "duration_s": 120}],
        pm_tasks=[{"task_key": "KAN-99", "title": "iOS feature"}],
    )
    con = sqlite3.connect(db_path)
    con.row_factory = sqlite3.Row
    fake_agent = _fake_agent_response("KAN-99", 0.88)

    from agents.run_task_linker import _classify_one
    result = _classify_one(
        20, db_path, con,
        agent=fake_agent,
        llm_model="m",
        llm_base_url="http://x",
    )

    con.close()
    for field in ("session_id", "task_key", "confidence", "session_type",
                  "reasoning", "method", "dimensions", "elapsed_s"):
        assert field in result, f"missing field: {field}"
    assert result["method"] == "hermes_aiagent"
    assert isinstance(result["elapsed_s"], float)


def test_classify_one_session_not_found_returns_error_shape():
    """_classify_one returns an error shape without calling the agent when the session row is absent."""
    db_path = _make_db_with_sessions([])
    con = sqlite3.connect(db_path)
    con.row_factory = sqlite3.Row
    stub_agent = MagicMock()

    from agents.run_task_linker import _classify_one
    result = _classify_one(
        999, db_path, con,
        agent=stub_agent,
        llm_model="m",
        llm_base_url="http://x",
    )

    con.close()
    stub_agent.run_conversation.assert_not_called()
    assert result["session_id"] == 999
    assert result["task_key"] is None
    assert result["method"] == "llm_error"


def test_classify_one_agent_exception_returns_llm_error():
    db_path = _make_db_with_sessions([{"id": 30, "app_name": "Safari", "duration_s": 60}])
    con = sqlite3.connect(db_path)
    con.row_factory = sqlite3.Row

    boom = MagicMock()
    boom.run_conversation.side_effect = RuntimeError("connection refused")

    from agents.run_task_linker import _classify_one
    result = _classify_one(
        30, db_path, con,
        agent=boom,
        llm_model="m",
        llm_base_url="http://x",
    )

    con.close()
    assert result["session_id"] == 30
    assert result["task_key"] is None
    assert result["method"] == "llm_error"
    assert "connection refused" in result["reasoning"]


# ── main(): agent construction ─────────────────────────────────────────────────

def test_main_constructs_agent_once_for_batch():
    """AIAgent must be instantiated once per batch, not once per session."""
    db_path = _make_db_with_sessions(
        [
            {"id": 1, "app_name": "VSCode", "duration_s": 90},
            {"id": 2, "app_name": "Terminal", "duration_s": 60},
        ],
        pm_tasks=[{"task_key": "KAN-1", "title": "Feature"}],
    )
    fake_agent = _fake_agent_response("KAN-1", 0.9)

    with patch("agents.run_task_linker._resolve_llm", return_value=("m", "http://x", "k")), \
         patch("agents.run_task_linker.AIAgent", return_value=fake_agent) as MockAgent:
        import io
        from agents.run_task_linker import main
        captured = io.StringIO()
        payload = json.dumps({"session_ids": [1, 2], "meridian_db": db_path})
        with patch("sys.stdin", io.StringIO(payload)), patch("sys.stdout", captured):
            main()

    MockAgent.assert_called_once()
    out = json.loads(captured.getvalue().strip())
    assert len(out["results"]) == 2


def test_main_constructs_agent_with_correct_params():
    """main() must build the agent with skip_memory=True, tool_delay=0, and max_tokens set."""
    db_path = _make_db_with_sessions(
        [{"id": 1, "app_name": "VSCode", "duration_s": 90}],
        pm_tasks=[{"task_key": "KAN-1", "title": "Feature"}],
    )
    fake_agent = _fake_agent_response("KAN-1", 0.9)

    with patch("agents.run_task_linker._resolve_llm", return_value=("mymodel", "http://local/v1", "mykey")), \
         patch("agents.run_task_linker.AIAgent", return_value=fake_agent) as MockAgent:
        import io
        from agents.run_task_linker import main
        captured = io.StringIO()
        payload = json.dumps({"session_ids": [1], "meridian_db": db_path})
        with patch("sys.stdin", io.StringIO(payload)), patch("sys.stdout", captured):
            main()

    kw = MockAgent.call_args.kwargs
    assert kw["model"] == "mymodel"
    assert kw["base_url"] == "http://local/v1"
    assert kw["api_key"] == "mykey"
    assert kw.get("skip_memory") is True
    assert kw.get("tool_delay") == 0.0
    assert kw.get("max_tokens") is not None


# ── main(): input validation ───────────────────────────────────────────────────

def test_main_empty_session_ids_exits_cleanly():
    db_path = _make_db_with_sessions([])
    payload = {"session_ids": [], "meridian_db": db_path}
    with patch("agents.run_task_linker._resolve_llm", return_value=("m", "http://x", "k")):
        from agents.run_task_linker import main
        import io
        captured = io.StringIO()
        with patch("sys.stdin", io.StringIO(json.dumps(payload))), \
             patch("sys.stdout", captured):
            main()
    out = json.loads(captured.getvalue().strip())
    assert out == {"results": []}


def test_main_missing_meridian_db_exits_nonzero():
    payload = {"session_ids": [1], "meridian_db": ""}
    proc = subprocess.run(
        [sys.executable, "-m", "agents.run_task_linker"],
        input=json.dumps(payload),
        capture_output=True, text=True, timeout=10,
        env={**__import__("os").environ, "PYTHONPATH": str(_SERVICES_DIR)},
    )
    assert proc.returncode != 0


def test_main_malformed_stdin_exits_nonzero():
    proc = subprocess.run(
        [sys.executable, "-m", "agents.run_task_linker"],
        input="not json",
        capture_output=True, text=True, timeout=10,
        env={**__import__("os").environ, "PYTHONPATH": str(_SERVICES_DIR)},
    )
    assert proc.returncode != 0


def test_main_nonexistent_db_path_exits_nonzero():
    payload = {"session_ids": [1], "meridian_db": "/tmp/does_not_exist_meridian.db"}
    proc = subprocess.run(
        [sys.executable, "-m", "agents.run_task_linker"],
        input=json.dumps(payload),
        capture_output=True, text=True, timeout=10,
        env={**__import__("os").environ, "PYTHONPATH": str(_SERVICES_DIR)},
    )
    assert proc.returncode != 0
