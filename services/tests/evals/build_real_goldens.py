"""Build a deepeval Golden set from HAND-LABELED real sessions in meridian.db.

Reads data/labels/real_curated.json (session_id -> human ground truth) and, for
each id, pulls the REAL production row from app_sessions (the exact session_text,
window_titles, app, timing the classifier saw) plus the CURRENT open pm_tasks
board as the candidate set, then rebuilds the exact classifier prompt via the
production builder (build_user_message + _fetch_recent_ticket_activity). Output:
data/generated/goldens_real_labeled.json — the deepeval Golden shape consumed by
eval_classifier.py and test_classifier.py.

Why this and not build_dataset.py: build_dataset.py uses the MODEL's own stored
task_key/session_type as the "expected" label (circular — it can't catch the
model's mistakes). This file uses INDEPENDENT human labels, deliberately weighted
toward untracked/overhead with hard decoys, to measure the failure mode that
matters: forcing a `task` when the right answer is `untracked`.

Why not raw screenpipe frames: the only other real-session label file
(real_2026-05-28.json) anchors to screenpipe frame ids, which were renumbered on
a re-sync and no longer address matching content. app_sessions PK ids are stable,
so this set never rots.

Usage:
    MERIDIAN_DB=~/.meridian/meridian.db \\
      services/.venv/bin/python services/tests/evals/build_real_goldens.py

Options (env vars):
    MERIDIAN_DB   Path to meridian.db (default: ~/.meridian/meridian.db)
    LABELS_FILE   Curated labels JSON (default: data/labels/real_curated.json)
"""
from __future__ import annotations

import json
import os
import sqlite3
import sys
from collections import Counter
from pathlib import Path

_SERVICES_DIR = Path(__file__).parent.parent.parent
if str(_SERVICES_DIR) not in sys.path:
    sys.path.insert(0, str(_SERVICES_DIR))

from agents._prompts import build_user_message  # noqa: E402
from agents.run_task_linker_mlx import _fetch_recent_ticket_activity  # noqa: E402

_EVAL_DIR = Path(__file__).parent
MERIDIAN_DB = Path(os.environ.get("MERIDIAN_DB", Path.home() / ".meridian/meridian.db")).expanduser()
LABELS_FILE = Path(os.environ.get("LABELS_FILE", _EVAL_DIR / "data" / "labels" / "real_curated.json"))
OUTPUT = _EVAL_DIR / "data" / "generated" / "goldens_real_labeled.json"

_SESSION_COLS = (
    "id, app_name, started_at, ended_at, duration_s, session_text,"
    " session_text_source, window_titles, category, confidence,"
    " COALESCE(coding_agent_session_uuid,'') AS ca_uuid, COALESCE(session_summary,'') AS session_summary"
)


def _fetch_session(con: sqlite3.Connection, sid: int) -> dict | None:
    row = con.execute(
        f"SELECT {_SESSION_COLS} FROM app_sessions WHERE id = ?", (sid,)
    ).fetchone()
    return dict(row) if row else None


def _fetch_open_board(con: sqlite3.Connection) -> list[dict]:
    """Current open pm_tasks board — the candidate set every session is judged against."""
    rows = con.execute(
        "SELECT task_key, title, COALESCE(description_text,'') AS description_text,"
        "       COALESCE(status_raw,'') AS status_raw,"
        "       COALESCE(issue_type,'') AS issue_type, COALESCE(epic_title,'') AS epic_title,"
        "       COALESCE(sprint_name,'') AS sprint_name"
        " FROM pm_tasks WHERE COALESCE(is_terminal,0) = 0"
        " ORDER BY task_key"
    ).fetchall()
    return [dict(r) for r in rows]


def main() -> int:
    if not MERIDIAN_DB.exists():
        print(f"ERROR: meridian.db not found at {MERIDIAN_DB}", file=sys.stderr)
        return 1
    if not LABELS_FILE.exists():
        print(f"ERROR: labels file not found at {LABELS_FILE}", file=sys.stderr)
        return 1

    spec = json.loads(LABELS_FILE.read_text())
    labels = spec["labels"]

    con = sqlite3.connect(f"file:{MERIDIAN_DB}?mode=ro", uri=True)
    con.row_factory = sqlite3.Row

    board = _fetch_open_board(con)
    board_keys = [t["task_key"] for t in board]
    print(f"Candidate board: {len(board)} open tickets -> {', '.join(board_keys)}")

    goldens: list[dict] = []
    missing: list[int] = []
    bad_key: list[str] = []
    for lab in labels:
        sid = lab["session_id"]
        raw = _fetch_session(con, sid)
        if raw is None:
            missing.append(sid)
            continue

        # Production parity: coding-agent rows classify on their summary, not the
        # raw transcript (see classify_session.py / run_task_linker_mlx).
        session_text = raw.get("session_text") or ""
        if raw.get("ca_uuid") and (raw.get("session_summary") or "").strip():
            session_text = raw["session_summary"]

        session = {
            "id": sid,
            "app_name": raw.get("app_name"),
            "started_at": raw.get("started_at") or "",
            "ended_at": raw.get("ended_at") or "",
            "duration_s": raw.get("duration_s"),
            "session_text": session_text,
            "session_text_source": raw.get("session_text_source") or "unknown",
            "window_titles": json.loads(raw.get("window_titles") or "[]"),
            "category": raw.get("category"),
            "confidence": raw.get("confidence") or 0.0,
            "audio_snippets": [],
        }
        recent = _fetch_recent_ticket_activity(con, session["started_at"], board_keys)
        prompt_input = build_user_message(
            session, board, recent_activity=recent, now_iso=session["started_at"]
        )

        # Sanity: a labelled task_key must be on the current board, else the
        # expected label is unreachable (classifier can only return a candidate).
        exp_key = lab.get("task_key")
        if exp_key and exp_key not in board_keys:
            bad_key.append(f"session {sid}: expected {exp_key} not on open board")

        expected = {
            "task_key": exp_key or "none",
            "session_type": lab["session_type"],
            "reasoning": lab.get("reasoning", ""),
        }
        goldens.append({
            "input": prompt_input,
            "expected_output": json.dumps(expected, ensure_ascii=False),
            "additional_metadata": {
                "seed_id": sid,
                "session_id": sid,
                "app_name": raw.get("app_name"),
                "difficulty": lab.get("difficulty", lab["session_type"]),
                "persona": "real_labeled",
                "confidence_label": lab.get("confidence", ""),
                "design_note": lab.get("design_note", ""),
            },
        })

    con.close()

    OUTPUT.parent.mkdir(parents=True, exist_ok=True)
    OUTPUT.write_text(json.dumps(goldens, indent=2, ensure_ascii=False) + "\n")

    print(f"\nWrote {len(goldens)} goldens -> {OUTPUT.relative_to(_SERVICES_DIR.parent)}")
    if missing:
        print(f"WARNING: {len(missing)} labelled sessions not found in DB: {missing}", file=sys.stderr)
    for b in bad_key:
        print(f"WARNING: {b}", file=sys.stderr)

    dist = Counter(json.loads(g["expected_output"])["session_type"] for g in goldens)
    tier = Counter(g["additional_metadata"]["difficulty"] for g in goldens)
    print("\nsession_type distribution:")
    for k, n in sorted(dist.items(), key=lambda x: -x[1]):
        print(f"  {k:<12} {n}")
    print("difficulty tiers:")
    for k, n in sorted(tier.items()):
        print(f"  {k:<18} {n}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
