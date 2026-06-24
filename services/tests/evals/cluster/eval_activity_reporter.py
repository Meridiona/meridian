"""Eval harness for the activity reporter pipeline (distil → /activity_report).

Runs distil_hour() / distil_range() then POSTs to /activity_report and prints
the full worklog report for each window. Measures entity recall (are known ticket
keys from session_summary present in the report?) as a deterministic quality signal.

Usage
-----
  python eval_activity_reporter.py [hour1 hour2 ...]
  python eval_activity_reporter.py --hours N            # auto-pick N peak hours (default 3)
  python eval_activity_reporter.py --range START END    # arbitrary time window
  python eval_activity_reporter.py --save               # write report_<label>.txt

Examples
--------
  python eval_activity_reporter.py --hours 3
  python eval_activity_reporter.py 2026-06-23T10 2026-06-23T13
  python eval_activity_reporter.py --range 2026-06-23T09 2026-06-23T12 --save
"""
from __future__ import annotations

import argparse
import sqlite3
import sys
from pathlib import Path

_REPO_SERVICES = Path(__file__).parents[3]
sys.path.insert(0, str(_REPO_SERVICES))

from agents.session_distiller import distil_hour, distil_range, evict_embedder
from agents.activity_reporter  import report_activity, ActivityReport

_DIR    = Path(__file__).parent
_DB     = _DIR.parents[2] / ".." / ".." / ".." / ".meridian" / "meridian.db"
_W      = 72
_SEP    = "═" * _W
_DIV    = "─" * _W

# Apps excluded from eval hours (same filter as audit_facts)
_EXCLUDE = ("Claude Code", "Codex", "GitHub Copilot", "Cursor Agent")
_MIN_DUR = 15


# ── DB helpers ─────────────────────────────────────────────────────────────────

def _db() -> sqlite3.Connection:
    import os
    path = os.path.expanduser("~/.meridian/meridian.db")
    return sqlite3.connect(path)


def _peak_hours(n: int) -> list[str]:
    con = _db()
    rows = con.execute(
        f"""SELECT substr(started_at,1,13) h, COUNT(*) c,
                   SUM(CASE WHEN session_summary IS NOT NULL
                              AND LENGTH(session_summary)>5 THEN 1 ELSE 0 END) g
              FROM app_sessions
             WHERE app_name NOT IN ({','.join('?'*len(_EXCLUDE))})
               AND duration_s >= ?
               AND session_text IS NOT NULL AND LENGTH(session_text)>40
               AND started_at >= date('now','-14 days')
             GROUP BY h HAVING g >= 3 ORDER BY c DESC LIMIT ?""",
        (*_EXCLUDE, _MIN_DUR, n),
    ).fetchall()
    con.close()
    return [r[0] for r in rows]


def _gold_tickets_for_hour(hour: str) -> set[str]:
    """Ticket keys known to have been worked on in this hour (from classifier output)."""
    con = _db()
    rows = con.execute(
        f"""SELECT DISTINCT task_key FROM app_sessions
             WHERE app_name NOT IN ({','.join('?'*len(_EXCLUDE))})
               AND duration_s >= ?
               AND session_text IS NOT NULL AND LENGTH(session_text)>40
               AND started_at >= ? AND started_at < ?
               AND task_key IS NOT NULL AND task_key != ''""",
        (*_EXCLUDE, _MIN_DUR, f"{hour}:00:00", f"{hour[:10]}T{int(hour[11:13])+1:02d}:00:00"),
    ).fetchall()
    con.close()
    return {r[0].lower() for r in rows}


def _gold_tickets_for_range(start: str, end: str) -> set[str]:
    con = _db()
    rows = con.execute(
        f"""SELECT DISTINCT task_key FROM app_sessions
             WHERE app_name NOT IN ({','.join('?'*len(_EXCLUDE))})
               AND duration_s >= ?
               AND session_text IS NOT NULL AND LENGTH(session_text)>40
               AND started_at >= ? AND started_at < ?
               AND task_key IS NOT NULL AND task_key != ''""",
        (*_EXCLUDE, _MIN_DUR, start, end),
    ).fetchall()
    con.close()
    return {r[0].lower() for r in rows}


# ── Metrics ────────────────────────────────────────────────────────────────────

def _entity_recall(report: ActivityReport, gold_keys: set[str]) -> dict:
    """Fraction of known ticket keys that appear anywhere in the report."""
    text   = report.report.lower()
    found  = {k for k in gold_keys if k in text}
    return {
        "total":   len(gold_keys),
        "found":   len(found),
        "recall":  len(found) / len(gold_keys) if gold_keys else 1.0,
        "missing": sorted(gold_keys - found),
    }


# ── Eval runners ───────────────────────────────────────────────────────────────

def _run_hour(hour: str, save: bool, preview: bool) -> dict | None:
    print(f"\n{_SEP}")
    print(f"HOUR  {hour}")
    print(_SEP)

    body, ds = distil_hour(hour)
    if not body:
        print("  (no sessions — skipped)")
        return None

    print(
        f"  distil : {ds.nsess} sessions  "
        f"{ds.raw_chars//1000}k → {ds.out_chars//1000}k chars  "
        f"({ds.reduction_pct:.1f}%  {ds.elapsed_s:.1f}s)",
    )

    report = report_activity(body, hour)
    gold   = _gold_tickets_for_hour(hour)
    recall = _entity_recall(report, gold)

    _print_metrics(report, recall)

    if preview:
        print(f"\n  WORKLOG REPORT:\n")
        for line in report.report.splitlines():
            print(f"    {line}")

    if save:
        _save(hour, report)

    return {"label": hour, "distil": ds, "report": report, "recall": recall}


def _run_range(start: str, end: str, save: bool, preview: bool) -> dict | None:
    label = f"{start}..{end}"
    print(f"\n{_SEP}")
    print(f"RANGE  {label}")
    print(_SEP)

    body, ds = distil_range(start, end)
    if not body:
        print("  (no sessions — skipped)")
        return None

    print(
        f"  distil : {ds.nsess} sessions  "
        f"{ds.raw_chars//1000}k → {ds.out_chars//1000}k chars  "
        f"({ds.reduction_pct:.1f}%  {ds.elapsed_s:.1f}s)",
    )

    report = report_activity(body, label)
    gold   = _gold_tickets_for_range(start, end)
    recall = _entity_recall(report, gold)

    _print_metrics(report, recall)

    if preview:
        print(f"\n  WORKLOG REPORT:\n")
        for line in report.report.splitlines():
            print(f"    {line}")

    if save:
        _save(label, report)

    return {"label": label, "distil": ds, "report": report, "recall": recall}


def _print_metrics(report: ActivityReport, recall: dict) -> None:
    print(
        f"  report : in_tok={report.input_tokens}  out_tok={report.output_tokens}  "
        f"think_tok={report.think_tokens}  {report.output_chars if hasattr(report, 'output_chars') else len(report.report)} chars  "
        f"elapsed={report.elapsed_s:.1f}s",
    )
    if recall["total"] > 0:
        print(
            f"  recall : {recall['recall']:.0%}  "
            f"({recall['found']}/{recall['total']} ticket keys)",
        )
        if recall["missing"]:
            print(f"  missing: {', '.join(recall['missing'])}")
    else:
        print("  recall : (no classified tickets in this window)")


def _save(label: str, report: ActivityReport) -> None:
    safe = label.replace(":", "").replace(" ", "_").replace("..", "_")
    out  = _DIR / f"report_{safe}.txt"
    out.write_text(report.report)
    print(f"  saved  → {out}")


# ── Grand summary ──────────────────────────────────────────────────────────────

def _grand_summary(results: list[dict]) -> None:
    print(f"\n\n{_SEP}")
    print(f"GRAND SUMMARY  ({len(results)} windows)")
    print(_SEP)
    print(f"  {'Label':22}  {'Sess':>4}  {'Distil%':>7}  {'InTok':>6}  {'OutTok':>6}  {'ThinkTok':>9}  {'Elapsed':>8}  {'Recall':>7}")
    print("  " + _DIV)

    totals = dict(distil=0.0, recall=0.0, in_tok=0, out_tok=0, think_tok=0, elapsed=0.0)
    for r in results:
        ds  = r["distil"]
        rp: ActivityReport = r["report"]
        rc  = r["recall"]
        print(
            f"  {r['label'][:22]:22}  {ds.nsess:4}  "
            f"{ds.reduction_pct:6.1f}%  "
            f"{rp.input_tokens:6}  {rp.output_tokens:6}  {rp.think_tokens:9}  "
            f"{rp.elapsed_s:7.1f}s  {rc['recall']:6.0%}",
        )
        totals["distil"]    += ds.reduction_pct
        totals["recall"]    += rc["recall"]
        totals["in_tok"]    += rp.input_tokens
        totals["out_tok"]   += rp.output_tokens
        totals["think_tok"] += rp.think_tokens
        totals["elapsed"]   += rp.elapsed_s

    n = len(results)
    print("  " + _DIV)
    print(
        f"  {'AVERAGE':22}  {'':4}  "
        f"{totals['distil']/n:6.1f}%  "
        f"{totals['in_tok']//n:6}  {totals['out_tok']//n:6}  {totals['think_tok']//n:9}  "
        f"{totals['elapsed']/n:7.1f}s  {totals['recall']/n:6.0%}",
    )


# ── CLI ────────────────────────────────────────────────────────────────────────

def main() -> None:
    p = argparse.ArgumentParser(
        description=__doc__,
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    p.add_argument("hours", nargs="*", help="specific hours (YYYY-MM-DDTHH)")
    p.add_argument("--hours", dest="n_hours", type=int, default=3, metavar="N",
                   help="auto-pick N peak hours (default 3)")
    p.add_argument("--range", nargs=2, metavar=("START", "END"))
    p.add_argument("--save",    action="store_true", help="write report to report_<label>.txt")
    p.add_argument("--preview", action="store_true", default=True,
                   help="print full report to stdout (default on)")
    p.add_argument("--no-preview", dest="preview", action="store_false")
    args = p.parse_args()

    results = []

    if args.range:
        start, end = args.range
        r = _run_range(start, end, args.save, args.preview)
        results = [r] if r else []
    else:
        hour_list = args.hours or _peak_hours(args.n_hours)
        if not hour_list:
            print("No hours found. Specify hours explicitly or check the DB.")
            sys.exit(1)
        print(f"Hours ({len(hour_list)}): {hour_list}")
        for hour in hour_list:
            r = _run_hour(hour, args.save, args.preview)
            if r:
                results.append(r)

    evict_embedder()

    if len(results) > 1:
        _grand_summary(results)
    elif results:
        print()


if __name__ == "__main__":
    main()
