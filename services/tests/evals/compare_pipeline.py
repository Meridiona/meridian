"""Three-layer pipeline comparison: hand-labeled ground truth vs meridian ETL + classifier.

Takes a day's hand-labeled ground truth (data/labels/real_<date>.json — natural work
blocks labeled directly from screenpipe frames, INDEPENDENT of ETL) and diffs it against
what meridian actually produced in app_sessions. Reports three layers:

  1. BOUNDARY  — how many app_sessions overlap each labeled block (fragmentation ratio).
                 1:1 = clean boundaries; 1:many = ETL over-fragmented the block.
  2. TYPE      — does the (frame-weighted) predicted session_type agree with ground truth?
  3. TASK_KEY  — does the (frame-weighted) predicted task_key agree? Exact + lenient
                 (KAN-139 ≡ KAN-141 treated as a near-equivalent pair — see labels findings).

Join key is the screenpipe frame_id: labeled blocks carry a frame_range; app_sessions carry
min_frame_id/max_frame_id. Each app_session is assigned to exactly one labeled block by its
midpoint frame, so fragments are never double-counted. Only screen-derived sessions
(coding_agent_session_uuid IS NULL) participate — coding-agent rows are a separate ingest path.

This is the measurement harness for the real-session eval (KAN-141). Re-run it after every
ETL fix to quantify the delta against a fixed ground-truth label set.

Usage:
    services/.venv/bin/python services/tests/evals/compare_pipeline.py --date 2026-05-28
    # writes results/compare_<date>.json and prints the per-block table + aggregate summary
"""
from __future__ import annotations

import argparse
import json
import os
import sqlite3
import sys
from collections import Counter, defaultdict
from pathlib import Path

_EVAL_DIR = Path(__file__).parent
_DEFAULT_MERIDIAN_DB = Path(os.environ.get("MERIDIAN_DB", "~/.meridian/meridian.db")).expanduser()

# Task keys that are genuinely entangled in the work and should not count as a hard miss
# against each other. See real_<date>.json _meta.findings.ticket_entanglement.
_EQUIV_GROUPS: list[frozenset[str]] = [frozenset({"KAN-139", "KAN-141"})]


def _norm_key(k: str | None) -> str:
    """Normalise a task_key for comparison: NULL/empty/none-ish → 'none'."""
    if not k or k.strip().lower() in {"none", "null", "n/a", ""}:
        return "none"
    return k.strip()


def _keys_match(expected: str, actual: str, lenient: bool) -> bool:
    if expected == actual:
        return True
    if lenient:
        for grp in _EQUIV_GROUPS:
            if expected in grp and actual in grp:
                return True
    return False


def _load_labels(date: str, labels_path: Path | None) -> dict:
    path = labels_path or (_EVAL_DIR / "data" / "labels" / f"real_{date}.json")
    if not path.exists():
        sys.exit(f"ERROR: labels file not found: {path}")
    return json.loads(path.read_text())


def _load_sessions(db_path: Path, date: str) -> list[dict]:
    """All screen-derived app_sessions for the date, with frame range + predictions."""
    if not db_path.exists():
        sys.exit(f"ERROR: meridian db not found: {db_path}")
    con = sqlite3.connect(f"file:{db_path}?mode=ro", uri=True)
    con.row_factory = sqlite3.Row
    rows = con.execute(
        """
        SELECT id, app_name, started_at, ended_at, duration_s, frame_count,
               min_frame_id, max_frame_id, task_key, task_session_type,
               task_confidence, task_method
        FROM app_sessions
        WHERE substr(started_at, 1, 10) = ?
          AND coding_agent_session_uuid IS NULL
        ORDER BY min_frame_id
        """,
        (date,),
    ).fetchall()
    con.close()
    return [dict(r) for r in rows]


def _assign_sessions_to_blocks(blocks: list[dict], sessions: list[dict]) -> tuple[dict, list[dict]]:
    """Assign each session to one block by midpoint frame. Returns (block_id→[sessions], unassigned)."""
    by_block: dict[int, list[dict]] = defaultdict(list)
    unassigned: list[dict] = []
    # sort blocks by frame range for a simple containment check
    ranges = [(b["block_id"], b["frame_range"][0], b["frame_range"][1]) for b in blocks]
    for s in sessions:
        mid = (s["min_frame_id"] + s["max_frame_id"]) // 2
        hit = next((bid for bid, lo, hi in ranges if lo <= mid <= hi), None)
        if hit is None:
            unassigned.append(s)
        else:
            by_block[hit].append(s)
    return by_block, unassigned


def _dominant(sessions: list[dict], field: str, normaliser) -> tuple[str, float]:
    """Frame-weighted dominant value of `field` across sessions + its coverage fraction."""
    weight: Counter = Counter()
    total = 0
    for s in sessions:
        fc = s["frame_count"] or 0
        weight[normaliser(s[field])] += fc
        total += fc
    if not weight or total == 0:
        return ("unclassified", 0.0)
    top, top_w = weight.most_common(1)[0]
    return (top, top_w / total)


def _norm_type(t: str | None) -> str:
    if not t or not t.strip():
        return "unclassified"
    return t.strip()


def compare(date: str, labels_path: Path | None, db_path: Path) -> dict:
    labels = _load_labels(date, labels_path)
    blocks = labels["blocks"]
    sessions = _load_sessions(db_path, date)
    by_block, unassigned = _assign_sessions_to_blocks(blocks, sessions)

    per_block = []
    agg = Counter()
    frag_total = 0
    for b in blocks:
        bid = b["block_id"]
        gt = b["ground_truth"]
        gt_key = _norm_key(gt["task_key"])
        gt_type = gt["session_type"]
        overlapping = by_block.get(bid, [])
        n_frag = len(overlapping)
        frag_total += n_frag

        pred_key, key_cov = _dominant(overlapping, "task_key", _norm_key)
        pred_type, type_cov = _dominant(overlapping, "task_session_type", _norm_type)

        boundary_ok = n_frag == 1
        type_ok = pred_type == gt_type
        key_exact = _keys_match(gt_key, pred_key, lenient=False)
        key_lenient = _keys_match(gt_key, pred_key, lenient=True)

        agg["blocks"] += 1
        agg["boundary_ok"] += boundary_ok
        agg["type_ok"] += type_ok
        agg["key_exact"] += key_exact
        agg["key_lenient"] += key_lenient
        agg["zero_coverage"] += (n_frag == 0)

        per_block.append({
            "block_id": bid,
            "app": b["app_name"],
            "activity": b.get("activity"),
            "span": f'{b["started_at"][11:19]}-{b["ended_at"][11:19]}',
            "frames": b["frame_count"],
            "n_app_sessions": n_frag,
            "boundary_ok": boundary_ok,
            "expected": {"task_key": gt_key, "session_type": gt_type, "confidence": gt.get("confidence")},
            "predicted": {"task_key": pred_key, "session_type": pred_type,
                          "key_coverage": round(key_cov, 2), "type_coverage": round(type_cov, 2)},
            "type_ok": type_ok,
            "key_exact_ok": key_exact,
            "key_lenient_ok": key_lenient,
        })

    n = agg["blocks"] or 1
    n_with_cov = (agg["blocks"] - agg["zero_coverage"]) or 1
    summary = {
        "date": date,
        "labeled_blocks": agg["blocks"],
        "total_app_sessions": len(sessions),
        "app_sessions_assigned": len(sessions) - len(unassigned),
        "app_sessions_unassigned": len(unassigned),  # midpoint fell in a sub-60s gap block
        "boundary": {
            "fragmentation_ratio": round(len(sessions) / n, 2),  # app_sessions per labeled block
            "assigned_fragmentation_ratio": round(frag_total / n, 2),
            "blocks_1to1": agg["boundary_ok"],
            "blocks_1to1_pct": round(agg["boundary_ok"] / n, 3),
            "blocks_zero_coverage": agg["zero_coverage"],
        },
        "session_type": {
            "agreement": agg["type_ok"],
            "agreement_pct": round(agg["type_ok"] / n, 3),
        },
        "task_key": {
            "exact_agreement": agg["key_exact"],
            "exact_pct": round(agg["key_exact"] / n, 3),
            "lenient_agreement": agg["key_lenient"],   # KAN-139 ≡ KAN-141
            "lenient_pct": round(agg["key_lenient"] / n, 3),
        },
    }
    return {"summary": summary, "per_block": per_block,
            "unassigned_sessions": [{"id": s["id"], "app": s["app_name"],
                                     "frames": s["frame_count"],
                                     "task_key": s["task_key"]} for s in unassigned]}


def _print_report(report: dict) -> None:
    s = report["summary"]
    print(f"\n{'='*92}")
    print(f"PIPELINE COMPARISON — {s['date']}   (ground truth: {s['labeled_blocks']} labeled blocks)")
    print(f"{'='*92}")
    print(f"app_sessions produced by ETL: {s['total_app_sessions']}  "
          f"(assigned to a labeled block: {s['app_sessions_assigned']}, "
          f"in sub-60s gaps: {s['app_sessions_unassigned']})")
    b, t, k = s["boundary"], s["session_type"], s["task_key"]
    print(f"\nLAYER 1 — BOUNDARY (fragmentation)")
    print(f"  fragmentation ratio:   {b['fragmentation_ratio']:>6}  app_sessions per labeled block")
    print(f"  clean 1:1 boundaries:  {b['blocks_1to1']}/{s['labeled_blocks']}  ({b['blocks_1to1_pct']*100:.1f}%)")
    print(f"  blocks with 0 coverage:{b['blocks_zero_coverage']:>3}  (no app_session mapped at all)")
    print(f"\nLAYER 2 — SESSION_TYPE")
    print(f"  agreement:             {t['agreement']}/{s['labeled_blocks']}  ({t['agreement_pct']*100:.1f}%)")
    print(f"\nLAYER 3 — TASK_KEY")
    print(f"  exact agreement:       {k['exact_agreement']}/{s['labeled_blocks']}  ({k['exact_pct']*100:.1f}%)")
    print(f"  lenient (139≡141):     {k['lenient_agreement']}/{s['labeled_blocks']}  ({k['lenient_pct']*100:.1f}%)")

    print(f"\n{'-'*92}")
    print(f"{'blk':>4} {'span':<18} {'app':<14} {'frag':>4} {'expected':<22} {'predicted':<22} {'B':>1} {'T':>1} {'K':>1}")
    print(f"{'-'*92}")
    for r in report["per_block"]:
        exp = f'{r["expected"]["task_key"]}/{r["expected"]["session_type"]}'
        prd = f'{r["predicted"]["task_key"]}/{r["predicted"]["session_type"]}'
        B = "✓" if r["boundary_ok"] else f'{r["n_app_sessions"]}'
        T = "✓" if r["type_ok"] else "✗"
        K = "✓" if r["key_exact_ok"] else ("~" if r["key_lenient_ok"] else "✗")
        print(f'{r["block_id"]:>4} {r["span"]:<18} {r["app"][:14]:<14} '
              f'{r["n_app_sessions"]:>4} {exp[:22]:<22} {prd[:22]:<22} {B:>1} {T:>1} {K:>1}')
    print(f"{'-'*92}")
    print("B: ✓=1:1 boundary, N=fragmented into N sessions  ·  T: session_type  ·  "
          "K: ✓ exact / ~ 139≡141 / ✗ miss\n")


def main() -> None:
    ap = argparse.ArgumentParser(description="Compare hand-labeled ground truth vs meridian ETL+classifier.")
    ap.add_argument("--date", required=True, help="YYYY-MM-DD (matches data/labels/real_<date>.json)")
    ap.add_argument("--labels", type=Path, default=None, help="override labels file path")
    ap.add_argument("--db", type=Path, default=_DEFAULT_MERIDIAN_DB, help="meridian.db path")
    ap.add_argument("--json-only", action="store_true", help="suppress the table, only write the JSON report")
    args = ap.parse_args()

    report = compare(args.date, args.labels, args.db)

    results_dir = _EVAL_DIR / "results"
    results_dir.mkdir(exist_ok=True)
    out = results_dir / f"compare_{args.date}.json"
    out.write_text(json.dumps(report, indent=2, ensure_ascii=False) + "\n")

    if not args.json_only:
        _print_report(report)
    print(f"Report written: {out}")


if __name__ == "__main__":
    main()
