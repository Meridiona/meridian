"""Unit tests for `_fetch_pm_tasks` candidate-set policy.

Covers the plan-only candidate filtering (CLASSIFY_PLAN_ONLY_CANDIDATES) added on
top of the legacy boost-never-filter behaviour, including both safety guards:
  * no confirmed plan  → every candidate is offered (unchanged behaviour)
  * plan confirmed     → candidates narrowed to the confirmed plan, in order
  * plan tickets gone  → fall back to the full set (never zero candidates)
  * curation-excluded  → never a candidate, even if named in the plan

Run: services/.venv/bin/pytest services/tests/test_fetch_pm_tasks.py -v
"""
from __future__ import annotations

import sqlite3
import sys
from pathlib import Path

import pytest

# Make `from agents import ...` resolve (mirror tests/evals/eval_classifier.py).
_SERVICES_DIR = Path(__file__).resolve().parent.parent
if str(_SERVICES_DIR) not in sys.path:
    sys.path.insert(0, str(_SERVICES_DIR))

from agents import mlx_classifier as rtl  # noqa: E402


def _make_con(task_keys: list[str], excluded: list[str] | None = None) -> sqlite3.Connection:
    """In-memory meridian DB with the columns `_fetch_pm_tasks` selects."""
    con = sqlite3.connect(":memory:")
    con.row_factory = sqlite3.Row
    con.execute(
        "CREATE TABLE pm_tasks ("
        " task_key TEXT PRIMARY KEY, title TEXT, description_text TEXT,"
        " status_raw TEXT, is_terminal INTEGER, issue_type TEXT,"
        " parent_key TEXT, epic_title TEXT, sprint_name TEXT, tags TEXT)"
    )
    con.execute(
        "CREATE TABLE pm_task_curation (task_key TEXT PRIMARY KEY, decision TEXT)"
    )
    for k in task_keys:
        con.execute(
            "INSERT INTO pm_tasks (task_key, title, description_text, status_raw,"
            " is_terminal, issue_type, parent_key, epic_title, sprint_name, tags)"
            " VALUES (?, ?, '', 'In Progress', 0, 'Task', '', '', '', '')",
            (k, f"title {k}"),
        )
    for k in excluded or []:
        con.execute(
            "INSERT INTO pm_task_curation (task_key, decision) VALUES (?, 'excluded')",
            (k,),
        )
    con.commit()
    return con


@pytest.fixture
def plan_only(monkeypatch):
    """Force plan-only filtering on regardless of the ambient env default."""
    monkeypatch.setattr(rtl, "_PLAN_ONLY_CANDIDATES", True)


@pytest.fixture
def boost_mode(monkeypatch):
    """Force the legacy boost-never-filter behaviour."""
    monkeypatch.setattr(rtl, "_PLAN_ONLY_CANDIDATES", False)


def _keys(tasks):
    return [t["task_key"] for t in tasks]


def test_no_plan_returns_all_unmarked(plan_only):
    """No confirmed plan → every candidate offered, none marked as focus."""
    con = _make_con(["K-1", "K-2", "K-3"])
    tasks = rtl._fetch_pm_tasks(con, focus_keys=[])
    assert set(_keys(tasks)) == {"K-1", "K-2", "K-3"}
    assert all(not t.get("is_today_focus") for t in tasks)


def test_plan_only_narrows_to_plan_in_declared_order(plan_only):
    """Plan confirmed → candidates are exactly the plan, in declared order, marked."""
    con = _make_con(["K-1", "K-2", "K-3", "K-4"])
    tasks = rtl._fetch_pm_tasks(con, focus_keys=["K-3", "K-1"])
    assert _keys(tasks) == ["K-3", "K-1"]  # declared order preserved
    assert all(t["is_today_focus"] for t in tasks)


def test_plan_only_falls_back_when_plan_tickets_absent(plan_only):
    """GUARD: plan tickets not in the live pool → fall back to ALL, never empty."""
    con = _make_con(["K-1", "K-2"])
    tasks = rtl._fetch_pm_tasks(con, focus_keys=["GHOST-9"])
    assert set(_keys(tasks)) == {"K-1", "K-2"}  # full set, not empty
    assert all(not t.get("is_today_focus") for t in tasks)


def test_plan_only_drops_curation_excluded_even_if_in_plan(plan_only):
    """An excluded ticket is never a candidate, even when named in the plan."""
    con = _make_con(["K-1", "K-2"], excluded=["K-2"])
    tasks = rtl._fetch_pm_tasks(con, focus_keys=["K-2", "K-1"])
    # K-2 excluded → only K-1 survives; still a non-empty, plan-scoped set.
    assert _keys(tasks) == ["K-1"]
    assert tasks[0]["is_today_focus"]


def test_plan_only_partial_plan_keeps_only_live_plan_tickets(plan_only):
    """Plan names a live + a dead ticket → only the live one is offered."""
    con = _make_con(["K-1", "K-2", "K-3"])
    tasks = rtl._fetch_pm_tasks(con, focus_keys=["K-2", "GHOST-9"])
    assert _keys(tasks) == ["K-2"]


def test_boost_mode_keeps_all_with_plan_floated_to_top(boost_mode):
    """Flag off → legacy behaviour: plan floated to top, every candidate kept."""
    con = _make_con(["K-1", "K-2", "K-3"])
    tasks = rtl._fetch_pm_tasks(con, focus_keys=["K-3"])
    assert set(_keys(tasks)) == {"K-1", "K-2", "K-3"}  # recall untouched
    assert tasks[0]["task_key"] == "K-3"  # floated to top
    assert tasks[0]["is_today_focus"]
    assert sum(1 for t in tasks if t.get("is_today_focus")) == 1
