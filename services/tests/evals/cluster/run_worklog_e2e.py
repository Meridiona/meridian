"""End-to-end worklog pipeline test across many hours.

Runs distil → report → rerank → match → worklog/propose → persist for each hour
and prints a readable result. Auto-picks busy hours across recent days, or takes
explicit --hours. Runs against a DB copy so persistence is exercised safely.

  services/.venv/bin/python services/tests/evals/cluster/run_worklog_e2e.py \
      --db /tmp/.../test_meridian.db --server http://127.0.0.1:7824 --pick 8
"""
from __future__ import annotations

import argparse
import sqlite3
import sys
import time
from pathlib import Path

ROOT = Path(__file__).parent
SERVICES = ROOT.parents[2]
sys.path.insert(0, str(SERVICES))

from agents.worklog_pipeline.pipeline import run_hour  # noqa: E402


def pick_hours(db: str, n: int) -> list[str]:
    conn = sqlite3.connect(db)
    rows = conn.execute(
        """
        SELECT substr(started_at,1,13) h, COUNT(*) c
        FROM app_sessions
        WHERE app_name NOT IN ('Claude Code','Codex','GitHub Copilot','Cursor Agent')
          AND duration_s >= 15 AND session_text IS NOT NULL AND LENGTH(session_text) > 40
          AND started_at >= date('now','-9 days')
        GROUP BY h HAVING c >= 8 ORDER BY c DESC LIMIT ?
        """,
        (n * 3,),
    ).fetchall()
    conn.close()
    # spread across distinct days
    seen_days, out = set(), []
    for h, _ in rows:
        day = h[:10]
        if seen_days.count(day) if isinstance(seen_days, list) else list(seen_days).count(day):
            pass
        out.append(h)
    # simple: take first n, but prefer day diversity
    chosen, days = [], {}
    for h, _ in rows:
        d = h[:10]
        if days.get(d, 0) >= 2:
            continue
        days[d] = days.get(d, 0) + 1
        chosen.append(h)
        if len(chosen) >= n:
            break
    return chosen or [r[0] for r in rows[:n]]


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--db", required=True)
    ap.add_argument("--server", default="http://127.0.0.1:7824")
    ap.add_argument("--hours", nargs="*", help="explicit hour labels")
    ap.add_argument("--pick", type=int, default=8)
    ap.add_argument("--dry-run", action="store_true")
    args = ap.parse_args()

    hours = args.hours or pick_hours(args.db, args.pick)
    print(f"Testing {len(hours)} hours: {hours}\n")

    for hour in hours:
        t0 = time.monotonic()
        try:
            r = run_hour(hour, db_path=args.db, server_url=args.server, dry_run=args.dry_run)
        except Exception as exc:  # noqa: BLE001
            print(f"━━ {hour}  ERROR: {exc}\n")
            continue
        dt = time.monotonic() - t0
        print(f"━━━━━━━━━━ {hour}  ({r.nsess} sess, report {r.report_chars}c, {dt:.0f}s)")
        if r.note:
            print(f"   note: {r.note}\n")
            continue
        if r.matched:
            print(f"   TIER {r.tier_used} → matched {len(r.matched)} task(s):")
            for m in r.matched:
                print(f"     • {m['task_key']} @ {m['confidence']:.2f} — {m['why']}")
                if m.get("summary"):
                    print(f"       worklog: {m['summary']}")
            print(f"   persisted worklog ids: {r.worklog_ids}")
        elif r.proposed:
            print(f"   NO MATCH → proposed new task (id {r.proposed_id}):")
            print(f"     title: {r.proposed['title']}")
            print(f"     desc:  {r.proposed['description'][:160]}")
        print()


if __name__ == "__main__":
    main()
