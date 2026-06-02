"""Build deepeval Goldens from hand-labeled real sessions (data/labels/real_<date>.json).

Distinct from build_dataset.py: that exporter labels each app_session with the CLASSIFIER's
own output (a consistency/regression export). This one is the real GOLDEN builder —
expected_output is the human ground-truth label, and the input is a natural work block with
its fragments MERGED back into one session (so the classifier sees a correctly-bounded
session, not the 3-frame slivers ETL currently produces).

Pipeline per labeled block:
  1. Find the app_sessions overlapping the block's frame_range (the ETL fragments).
  2. Merge them: union window_titles (sum counts, re-sort), take the richest session_text,
     use the block's true start/end/duration/frame_count.
  3. Candidate tickets = the day's candidate set recorded in the labels _meta (so a block
     labeled KAN-140 actually has KAN-140 as a candidate, even after it left pm_tasks).
  4. Recent-work context = the prior 5 scoreable labeled blocks (ground-truth recent context).
  5. expected_output = the block's ground_truth (task_key, session_type, reasoning).

Blocks with label_confidence == "low" are exported with scoreable=false (kept for context /
inspection but excluded from accuracy scoring — the human label itself is uncertain there).

Usage:
    services/.venv/bin/python services/tests/evals/build_goldens_from_labels.py --date 2026-05-28
    # writes data/generated/goldens_real_<date>.json
"""
from __future__ import annotations

import argparse
import json
import os
import sqlite3
import sys
from collections import Counter
from pathlib import Path

_EVAL_DIR = Path(__file__).parent
_SERVICES_DIR = _EVAL_DIR.parent.parent
if str(_SERVICES_DIR) not in sys.path:
    sys.path.insert(0, str(_SERVICES_DIR))

from agents._prompts import build_user_message  # noqa: E402

_DEFAULT_DB = Path(os.environ.get("MERIDIAN_DB", "~/.meridian/meridian.db")).expanduser()
_NULL = {"none", "null", "n/a", "nil", "", None}


def _norm_key(k: str | None) -> str:
    if k is None or k.strip().lower() in _NULL:
        return "none"
    return k.strip()


def _title_name(t) -> tuple[str, int]:
    if isinstance(t, dict):
        return (t.get("window_name") or t.get("title") or "", int(t.get("count", 1)))
    if isinstance(t, (list, tuple)) and t:
        return (str(t[0]), int(t[1]) if len(t) > 1 else 1)
    return (str(t), 1)


def _merge_fragments(rows: list[dict], block: dict) -> dict:
    """Merge the ETL fragments overlapping a block into one session dict for the prompt."""
    title_counts: Counter = Counter()
    best_text, best_text_len, best_source = "", -1, "unknown"
    dom = max(rows, key=lambda r: r["frame_count"] or 0, default=None) if rows else None
    for r in rows:
        for t in json.loads(r["window_titles"] or "[]"):
            name, cnt = _title_name(t)
            if name:
                title_counts[name] += cnt
        txt = (r["session_text"] or "").strip()
        if len(txt) > best_text_len:
            best_text, best_text_len, best_source = txt, len(txt), (r["session_text_source"] or "unknown")
    merged_titles = [{"window_name": n, "count": c} for n, c in title_counts.most_common(10)]
    return {
        "app_name":            block["app_name"],
        "started_at":          block["started_at"],
        "ended_at":            block["ended_at"],
        "duration_s":          block["duration_s"],
        "category":            (dom or {}).get("category") or "",
        "confidence":          (dom or {}).get("confidence") or 0.0,
        "window_titles":       merged_titles,
        "session_text":        best_text,
        "session_text_source": best_source,
        "audio_snippets":      [],
    }


def _candidates_from_meta(meta: dict) -> list[dict]:
    out = []
    for key, title in (meta.get("candidate_tickets") or {}).items():
        out.append({
            "task_key": key, "title": title, "description_text": "",
            "status_category": "", "issue_type": "", "epic_title": "", "sprint_name": "",
        })
    return out


def _overlapping(con: sqlite3.Connection, lo: int, hi: int) -> list[dict]:
    rows = con.execute(
        "SELECT id, frame_count, window_titles, session_text, session_text_source,"
        "       category, confidence"
        " FROM app_sessions"
        " WHERE claude_session_uuid IS NULL AND min_frame_id <= ? AND max_frame_id >= ?"
        " ORDER BY min_frame_id",
        (hi, lo),
    ).fetchall()
    return [dict(r) for r in rows]


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--date", required=True)
    ap.add_argument("--labels", type=Path, default=None)
    ap.add_argument("--db", type=Path, default=_DEFAULT_DB)
    args = ap.parse_args()

    labels_path = args.labels or (_EVAL_DIR / "data" / "labels" / f"real_{args.date}.json")
    doc = json.loads(labels_path.read_text())
    meta, blocks = doc["_meta"], doc["blocks"]
    candidates = _candidates_from_meta(meta)

    con = sqlite3.connect(f"file:{args.db}?mode=ro", uri=True)
    con.row_factory = sqlite3.Row

    goldens, recent_history = [], []
    skipped_low = 0
    for b in blocks:
        gt = b["ground_truth"]
        lo, hi = b["frame_range"]
        frags = _overlapping(con, lo, hi)
        session = _merge_fragments(frags, b)
        prompt = build_user_message(session, candidates, recent_sessions=recent_history[-5:])

        scoreable = gt.get("confidence") != "low"
        expected = {
            "task_key": _norm_key(gt["task_key"]),
            "session_type": gt["session_type"],
            "reasoning": gt["reasoning"],
        }
        goldens.append({
            "input": prompt,
            "expected_output": json.dumps(expected, ensure_ascii=False),
            "additional_metadata": {
                "source": "real-labeled",
                "persona": f"real_{args.date}",
                "seed_id": b["block_id"],          # block_id doubles as the span seed_id
                "difficulty": gt.get("confidence"),  # tier = label confidence (high/medium/low)
                "date": args.date,
                "block_id": b["block_id"],
                "app_name": b["app_name"],
                "activity": b.get("activity"),
                "label_confidence": gt.get("confidence"),
                "etl_fragments": len(frags),
                "scoreable": scoreable,
            },
        })
        if not scoreable:
            skipped_low += 1
        # recent-context history is built only from scoreable, task-bearing blocks
        if scoreable:
            recent_history.append({
                "app_name": b["app_name"], "started_at": b["started_at"],
                "duration_s": b["duration_s"], "task_key": _norm_key(gt["task_key"]),
                "task_routing": "ground_truth", "category": b.get("activity") or "",
            })
    con.close()

    out = _EVAL_DIR / "data" / "generated" / f"goldens_real_{args.date}.json"
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(goldens, indent=2, ensure_ascii=False) + "\n")

    print(f"Wrote {len(goldens)} goldens → {out}")
    print(f"  scoreable: {len(goldens) - skipped_low}   context-only (low-confidence): {skipped_low}")
    dist = Counter(g["expected_output"] for g in goldens if g["additional_metadata"]["scoreable"])
    print("\nScoreable label distribution:")
    for label, n in sorted(dist.items(), key=lambda x: -x[1]):
        print(f"  {n:>2}  {label}")


if __name__ == "__main__":
    main()
