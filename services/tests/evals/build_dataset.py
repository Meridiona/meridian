"""Build tests/evals/.dataset.json from real labeled sessions in meridian.db.

Queries sessions where task_routing = 'auto' (high-confidence auto-classified)
as ground truth, builds the full formatted prompt for each, and writes them
as DeepEval Goldens to .dataset.json.

Usage:
    cd services
    MERIDIAN_DB=~/.meridian/meridian.db python tests/evals/build_dataset.py

Options (env vars):
    MERIDIAN_DB     Path to meridian.db  (default: ~/.meridian/meridian.db)
    MIN_CONFIDENCE  Minimum confidence to include a session (default: 0.80)
    LIMIT           Max number of goldens to export (default: 100)

The output overwrites tests/evals/.dataset.json.
Review and spot-check the exported goldens before committing them.
"""
from __future__ import annotations

import json
import os
import sqlite3
import sys
from pathlib import Path

_SERVICES_DIR = Path(__file__).parent.parent.parent
if str(_SERVICES_DIR) not in sys.path:
    sys.path.insert(0, str(_SERVICES_DIR))

from agents._prompts import build_user_message

MERIDIAN_DB = Path(os.environ.get("MERIDIAN_DB", Path.home() / ".meridian/meridian.db"))
MIN_CONFIDENCE = float(os.environ.get("MIN_CONFIDENCE", "0.80"))
LIMIT = int(os.environ.get("LIMIT", "100"))
OUTPUT = Path(__file__).parent / ".dataset.json"


def _fetch_labeled_sessions(con: sqlite3.Connection) -> list[dict]:
    rows = con.execute(
        "SELECT id, app_name, started_at, ended_at, duration_s, session_text,"
        "       session_text_source, window_titles, category, confidence,"
        "       task_key, task_routing"
        " FROM app_sessions"
        " WHERE task_key IS NOT NULL"
        "   AND task_routing = 'auto'"
        "   AND confidence >= ?"
        " ORDER BY id DESC"
        " LIMIT ?",
        (MIN_CONFIDENCE, LIMIT),
    ).fetchall()
    return [dict(r) for r in rows]


def _fetch_pm_tasks(con: sqlite3.Connection) -> list[dict]:
    rows = con.execute(
        "SELECT task_key, title, COALESCE(description_text,'') AS description_text,"
        "       COALESCE(status,'') AS status, COALESCE(status_category,'') AS status_category,"
        "       COALESCE(issue_type,'') AS issue_type, COALESCE(epic_title,'') AS epic_title,"
        "       COALESCE(sprint_name,'') AS sprint_name"
        " FROM pm_tasks WHERE LOWER(status_category) != 'done'"
    ).fetchall()
    return [dict(r) for r in rows]


def _fetch_recent(con: sqlite3.Connection, before_id: int) -> list[dict]:
    rows = con.execute(
        "SELECT app_name, started_at, duration_s, task_key, task_routing, category"
        " FROM app_sessions"
        " WHERE id < ? AND duration_s > 1 AND COALESCE(session_text,'') != ''"
        " ORDER BY id DESC LIMIT 5",
        (before_id,),
    ).fetchall()
    result = [dict(r) for r in rows]
    result.reverse()
    return result


def main() -> None:
    if not MERIDIAN_DB.exists():
        print(f"ERROR: meridian.db not found at {MERIDIAN_DB}", file=sys.stderr)
        print("Set MERIDIAN_DB env var to the correct path.", file=sys.stderr)
        sys.exit(1)

    con = sqlite3.connect(MERIDIAN_DB)
    con.row_factory = sqlite3.Row

    sessions = _fetch_labeled_sessions(con)
    pm_tasks = _fetch_pm_tasks(con)

    goldens: list[dict] = []
    for s in sessions:
        session = {
            "id":                  s["id"],
            "app_name":            s["app_name"],
            "started_at":          s["started_at"] or "",
            "ended_at":            s["ended_at"] or "",
            "duration_s":          s["duration_s"],
            "session_text":        s["session_text"] or "",
            "session_text_source": s["session_text_source"] or "unknown",
            "window_titles":       json.loads(s["window_titles"] or "[]"),
            "category":            s["category"],
            "confidence":          s["confidence"] or 0.0,
            "audio_snippets":      [],
        }
        recent = _fetch_recent(con, s["id"])
        prompt_input = build_user_message(session, pm_tasks, recent_sessions=recent)

        goldens.append({
            "input": prompt_input,
            "expected_output": s["task_key"],
            "additional_metadata": {
                "session_id": s["id"],
                "app_name":   s["app_name"],
                "confidence": s["confidence"],
            },
        })

    con.close()

    OUTPUT.write_text(json.dumps(goldens, indent=2, ensure_ascii=False))
    print(f"Wrote {len(goldens)} goldens to {OUTPUT}")
    print(f"Review before committing: spot-check at least 10-20 entries for correctness.")


if __name__ == "__main__":
    main()
