"""Build tests/evals/.dataset.json from real labeled sessions in meridian.db.

Two modes:

  SESSION_IDS mode (preferred for curated eval sets):
    Export specific sessions by ID. Bypasses routing/confidence filters.
    SESSION_IDS=2276,2354,1961,2181,1792,1972,2514 \\
      MERIDIAN_DB=~/.meridian/meridian.db \\
      python tests/evals/build_dataset.py

  Bulk mode (general export):
    Queries sessions where task_method = 'hermes_aiagent' and routing is
    'auto' or 'pending' (hermes sessions use 'pending') with high confidence.
    MERIDIAN_DB=~/.meridian/meridian.db python tests/evals/build_dataset.py

Options (env vars):
    MERIDIAN_DB     Path to meridian.db  (default: ~/.meridian/meridian.db)
    SESSION_IDS     Comma-separated session IDs to export (overrides bulk query)
    MIN_CONFIDENCE  Minimum confidence for bulk mode (default: 0.85)
    LIMIT           Max sessions for bulk mode (default: 100)

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
SESSION_IDS: list[int] = [
    int(x.strip())
    for x in os.environ.get("SESSION_IDS", "").split(",")
    if x.strip().isdigit()
]
MIN_CONFIDENCE = float(os.environ.get("MIN_CONFIDENCE", "0.85"))
LIMIT = int(os.environ.get("LIMIT", "100"))
OUTPUT = Path(__file__).parent / ".dataset.json"

_NULL_TASK_KEYS = {"none", "null", "n/a", "nil", "undefined", "hermes_aiagent", ""}


def _normalise_task_key(raw: str | None) -> str:
    if raw is None:
        return "none"
    if raw.strip().lower() in _NULL_TASK_KEYS:
        return "none"
    return raw.strip()


_SESSION_COLS = (
    "id, app_name, started_at, ended_at, duration_s, session_text,"
    " session_text_source, window_titles, category, confidence,"
    " task_key, task_routing, task_method, task_session_type,"
    " COALESCE(task_reasoning, '') AS task_reasoning"
)


def _fetch_sessions_by_ids(con: sqlite3.Connection, ids: list[int]) -> list[dict]:
    placeholders = ",".join("?" * len(ids))
    rows = con.execute(
        f"SELECT {_SESSION_COLS}"
        f" FROM app_sessions"
        f" WHERE id IN ({placeholders})"
        f" ORDER BY id",
        ids,
    ).fetchall()
    return [dict(r) for r in rows]


def _fetch_labeled_sessions(con: sqlite3.Connection) -> list[dict]:
    # hermes_aiagent sessions use task_routing='pending'; legacy auto-classified use 'auto'
    rows = con.execute(
        f"SELECT {_SESSION_COLS}"
        " FROM app_sessions"
        " WHERE task_method IN ('hermes_aiagent', 'mlx_direct')"
        "   AND task_routing IN ('auto', 'pending')"
        "   AND task_confidence >= ?"
        " ORDER BY id DESC"
        " LIMIT ?",
        (MIN_CONFIDENCE, LIMIT),
    ).fetchall()
    return [dict(r) for r in rows]


def _fetch_pm_tasks(con: sqlite3.Connection) -> list[dict]:
    rows = con.execute(
        "SELECT task_key, title, COALESCE(description_text,'') AS description_text,"
        "       COALESCE(status_category,'') AS status_category,"
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

    if SESSION_IDS:
        print(f"SESSION_IDS mode: exporting {len(SESSION_IDS)} specific sessions: {SESSION_IDS}")
        sessions = _fetch_sessions_by_ids(con, SESSION_IDS)
        missing = set(SESSION_IDS) - {s["id"] for s in sessions}
        if missing:
            print(f"WARNING: sessions not found in DB: {sorted(missing)}", file=sys.stderr)
    else:
        print(f"Bulk mode: fetching hermes_aiagent sessions (confidence >= {MIN_CONFIDENCE}, limit {LIMIT})")
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

        expected = {
            "task_key":     _normalise_task_key(s.get("task_key")),
            "session_type": s.get("task_session_type") or "overhead",
            "reasoning":    s.get("task_reasoning") or "",
        }
        goldens.append({
            "input": prompt_input,
            "expected_output": json.dumps(expected, ensure_ascii=False),
            "additional_metadata": {
                "session_id":  s["id"],
                "app_name":    s["app_name"],
                "task_method": s.get("task_method", ""),
            },
        })

    con.close()

    OUTPUT.write_text(json.dumps(goldens, indent=2, ensure_ascii=False))
    print(f"Wrote {len(goldens)} goldens to {OUTPUT}")
    print("Review before committing: verify expected_output labels are correct.")
    print(f"\nLabel distribution:")
    from collections import Counter
    counts = Counter(g["expected_output"] for g in goldens)
    for label, n in sorted(counts.items(), key=lambda x: -x[1]):
        print(f"  {label}: {n}")


if __name__ == "__main__":
    main()
