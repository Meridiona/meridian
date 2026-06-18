"""Dry-run the task classifier for one or more session ids — read-only.

Drives the SAME code path the daemon uses: POSTs the session id(s) to the
running MLX server's /classify_sessions endpoint, which runs `_classify_one`
(fetch session + recent context + pm_tasks → build prompt → model → parse).
The Python endpoint only RETURNS the result; the DB write is done separately by
the Rust daemon, so calling this never mutates app_sessions — you see exactly
what the classifier WOULD output right now, with the current code and prompt.

Usage:
    services/.venv/bin/python services/tests/evals/classify_session.py 20128
    services/.venv/bin/python services/tests/evals/classify_session.py 20128 20127 --show-prompt
    MLX_SERVER_URL=http://127.0.0.1:7823 ... classify_session.py 20128

The MLX server must be running (meridian status / port 7823). It uses the
already-loaded model, so this is fast — no in-process model load.
"""

from __future__ import annotations

import argparse
import json
import os
import sqlite3
import sys
import urllib.request
from pathlib import Path

_DEFAULT_URL = os.environ.get("MLX_SERVER_URL", "http://127.0.0.1:7823").rstrip("/")
_DEFAULT_DB = os.path.expanduser(
    os.environ.get("MERIDIAN_DB", "~/.meridian/meridian.db")
)


def _reconstruct_prompt(db_path: str, session_id: int) -> str | None:
    """Rebuild the exact prompt via the production builder (read-only)."""
    # Import lazily so the common path (no --show-prompt) needs no agents deps.
    sys.path.insert(0, str(Path(__file__).resolve().parents[2]))  # services/
    from agents._prompts import build_user_message
    from agents.run_task_linker_mlx import (
        _fetch_pm_tasks,
        _fetch_recent_ticket_activity,
        _fetch_session,
    )

    con = sqlite3.connect(f"file:{db_path}?mode=ro", uri=True)
    con.row_factory = sqlite3.Row
    raw = _fetch_session(con, session_id)
    if raw is None:
        return None
    pm_tasks = _fetch_pm_tasks(con)
    recent = _fetch_recent_ticket_activity(
        con, raw.get("started_at") or "", [t["task_key"] for t in pm_tasks]
    )
    session_text = raw.get("session_text") or ""
    if raw.get("coding_agent_session_uuid") and (raw.get("session_summary") or "").strip():
        session_text = raw["session_summary"]
    session = {
        "id": session_id,
        "app_name": raw.get("app_name"),
        "started_at": raw.get("started_at", ""),
        "ended_at": raw.get("ended_at", ""),
        "duration_s": raw.get("duration_s"),
        "session_text": session_text,
        "session_text_source": raw.get("session_text_source", "unknown"),
        "window_titles": json.loads(raw.get("window_titles") or "[]"),
        "category": raw.get("category"),
        "confidence": raw.get("confidence", 0.0),
        "audio_snippets": [],
    }
    return build_user_message(
        session, pm_tasks, recent_activity=recent, now_iso=raw.get("started_at")
    )


def _classify(url: str, db_path: str, session_ids: list[int]) -> list[dict]:
    payload = json.dumps({"session_ids": session_ids, "meridian_db": db_path}).encode()
    req = urllib.request.Request(
        f"{url}/classify_sessions",
        data=payload,
        headers={"Content-Type": "application/json"},
    )
    with urllib.request.urlopen(req, timeout=600) as resp:
        return json.loads(resp.read()).get("results", [])


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("session_ids", nargs="+", type=int, help="session id(s) to classify")
    ap.add_argument("--show-prompt", action="store_true", help="also print the exact prompt sent")
    ap.add_argument("--url", default=_DEFAULT_URL, help=f"MLX server (default {_DEFAULT_URL})")
    ap.add_argument("--db", default=_DEFAULT_DB, help=f"meridian.db (default {_DEFAULT_DB})")
    ap.add_argument("--json", action="store_true", help="print raw JSON results")
    args = ap.parse_args()

    if args.show_prompt:
        for sid in args.session_ids:
            prompt = _reconstruct_prompt(args.db, sid)
            print(f"\n{'='*30} PROMPT for session {sid} {'='*30}")
            print(prompt if prompt is not None else f"(session {sid} not found)")

    results = _classify(args.url, args.db, args.session_ids)

    if args.json:
        print(json.dumps(results, indent=2))
        return 0

    for r in results:
        print(f"\n{'='*30} RESULT for session {r.get('session_id')} {'='*30}")
        for k in (
            "task_key",
            "session_type",
            "confidence",
            "category",
            "category_confidence",
            "method",
        ):
            print(f"  {k:20} = {r.get(k)}")
        reasoning = (r.get("reasoning") or "").strip()
        if reasoning:
            print(f"  reasoning            = {reasoning}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
