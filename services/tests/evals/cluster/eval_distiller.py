"""Eval harness for services/agents/session_distiller.py.

Measures compression quality (fact recall, reduction %, entity rescue) across
many hours. Optionally baselines against the eval-version hour_text.py.

Usage
-----
  python eval_distiller.py [hour1 hour2 ...]        specific hours
  python eval_distiller.py --hours N                auto-pick N peak hours (default 6)
  python eval_distiller.py --range START END        arbitrary time window
  python eval_distiller.py ... --baseline           also score hour_text.py for comparison
  python eval_distiller.py ... --save               write body to distil_<label>.txt
  python eval_distiller.py ... --preview N          body preview lines (default 8)

Examples
--------
  python eval_distiller.py --hours 6 --baseline
  python eval_distiller.py 2026-06-16T09 2026-06-16T10 --save
  python eval_distiller.py --range 2026-06-16T09 2026-06-16T11
"""
from __future__ import annotations

import argparse
import os
import sqlite3
import sys
import time
from pathlib import Path

# Make agents/ importable regardless of CWD
_REPO_SERVICES = Path(__file__).parents[3]   # …/meridian/services
sys.path.insert(0, str(_REPO_SERVICES))

import clib
from audit_facts import extract_facts, fact_present, gold_for_hour

from agents.session_distiller import distil_hour, distil_range, evict_embedder, DistilStats

_DIR = Path(__file__).parent
_W   = 72
_SEP = "═" * _W


# ── Helpers ────────────────────────────────────────────────────────────────────

def _peak_hours(n: int) -> list[str]:
    """Return up to n hour strings for the busiest recent hours with gold summaries."""
    from audit_facts import EXCLUDE, MIN_DUR
    con = sqlite3.connect(clib.DB)
    rows = con.execute(
        f"""SELECT substr(started_at,1,13) h, COUNT(*) c,
                   SUM(CASE WHEN session_summary IS NOT NULL
                              AND LENGTH(session_summary)>5 THEN 1 ELSE 0 END) g
              FROM app_sessions
             WHERE app_name NOT IN ({','.join('?'*len(EXCLUDE))})
               AND duration_s >= ?
               AND session_text IS NOT NULL AND LENGTH(session_text)>40
               AND started_at >= date('now','-14 days')
             GROUP BY h HAVING g >= 3 ORDER BY c DESC LIMIT ?""",
        (*EXCLUDE, MIN_DUR, n),
    ).fetchall()
    con.close()
    return [r[0] for r in rows]


def _gold_for_range(start: str, end: str) -> list[dict]:
    """Gold sessions within an arbitrary time window."""
    from audit_facts import EXCLUDE, MIN_DUR
    con = sqlite3.connect(clib.DB)
    rows = con.execute(
        f"""SELECT id, app_name, task_key, task_session_type,
                   COALESCE(NULLIF(session_summary,''), task_reasoning, '')
              FROM app_sessions
             WHERE app_name NOT IN ({','.join('?'*len(EXCLUDE))})
               AND duration_s >= ?
               AND session_text IS NOT NULL AND LENGTH(session_text)>40
               AND started_at >= ? AND started_at < ?
             ORDER BY started_at""",
        (*EXCLUDE, MIN_DUR, start, end),
    ).fetchall()
    con.close()
    return [{"sid": r[0], "app": r[1], "tk": r[2], "type": r[3], "gold": r[4]}
            for r in rows if r[4] and len(r[4]) > 10]


def _score(body: str, golds: list[dict]) -> dict:
    """Deterministic fact-recall of body vs gold session summaries."""
    total = found = no_facts = 0
    missing_lines: list[str] = []
    for g in golds:
        facts = extract_facts(g["gold"])
        if not facts:
            no_facts += 1
            continue
        total += len(facts)
        pres = [fact_present(f, body) for f in facts]
        found += sum(pres)
        lost = [f for f, ok in zip(facts, pres) if not ok]
        if lost:
            tag = f'{g["app"]}{"/"+g["tk"] if g["tk"] else ""}|{g["type"] or "?"}'
            missing_lines.append(f'    [{tag}]  ' + ", ".join(lost[:6]))
    return {
        "total": total, "found": found,
        "recall": found / total if total else 1.0,
        "no_facts": no_facts, "missing": missing_lines,
    }


def _save_body(label: str, body: str) -> None:
    safe = label.replace(":", "").replace(" ", "_").replace("..", "_")
    out = _DIR / f"distil_{safe}.txt"
    out.write_text(body)
    print(f"  saved → {out}")


# ── Per-window evaluation ──────────────────────────────────────────────────────

def _eval_hour(hour: str, baseline: bool, save: bool, preview_lines: int) -> dict | None:
    """Run distil_hour, score, optionally baseline. Returns result dict."""
    print(f"\n{_SEP}")
    print(f"HOUR  {hour}")
    print(_SEP)

    body, st = distil_hour(hour)
    if not body:
        print("  (no sessions — skipped)")
        return None

    golds = gold_for_hour(hour)
    sc = _score(body, golds)
    _print_result(st, sc, preview_lines, body)

    base_sc: dict | None = None
    if baseline:
        try:
            from hour_text import build_hour_text
            base_body, base_st = build_hour_text(hour)
            base_sc = _score(base_body, golds)
            delta = sc["recall"] - base_sc["recall"]
            sign  = "+" if delta >= 0 else ""
            print(f"\n  vs hour_text.py :  {base_sc['recall']:5.1%}   Δ={sign}{delta:.1%}")
        except Exception as e:
            print(f"\n  (baseline error: {e})")

    if save:
        _save_body(hour, body)

    return {"label": hour, "stats": st, "score": sc, "base_score": base_sc}


def _eval_range(start: str, end: str, baseline: bool, save: bool, preview_lines: int) -> dict | None:
    label = f"{start}..{end}"
    print(f"\n{_SEP}")
    print(f"RANGE  {label}")
    print(_SEP)

    body, st = distil_range(start, end)
    if not body:
        print("  (no sessions — skipped)")
        return None

    golds = _gold_for_range(start, end)
    sc = _score(body, golds)
    _print_result(st, sc, preview_lines, body)

    if baseline:
        print("  (--baseline not supported for --range mode)")

    if save:
        _save_body(label, body)

    return {"label": label, "stats": st, "score": sc, "base_score": None}


def _print_result(st: DistilStats, sc: dict, preview_lines: int, body: str) -> None:
    print(
        f"\n  sessions : {st.nsess:3}    "
        f"raw : {st.raw_chars//1000:4} k chars    "
        f"out : {st.out_chars//1000:3} k chars    "
        f"reduction : {st.reduction_pct:.1f}%"
    )
    print(
        f"  funnel   : spans {st.n_after_junk} → df {st.n_after_df} "
        f"→ lex {st.n_after_lex} → sem {st.n_after_sem} → selected {st.n_selected}"
    )
    print(
        f"  rescue   : entity_rescued={st.n_entity_rescued}  "
        f"session_rescued={st.n_session_rescued}   elapsed={st.elapsed_s:.1f}s"
    )
    if sc["total"] > 0:
        print(
            f"\n  RECALL vs gold :  {sc['recall']:5.1%}   "
            f"(facts {sc['found']}/{sc['total']}  no-facts {sc['no_facts']})"
        )
    else:
        print("\n  RECALL vs gold :  (no gold sessions with extractable facts)")

    if sc["missing"]:
        print(f"\n  missing facts ({min(len(sc['missing']), 5)} shown):")
        for line in sc["missing"][:5]:
            print(line)

    if preview_lines > 0:
        lines = body.splitlines()[:preview_lines + 2]
        print(f"\n  body preview ({preview_lines} lines):")
        for ln in lines[:preview_lines + 2]:
            print(f"    {ln}")
        if len(body.splitlines()) > preview_lines + 2:
            print("    ...")


# ── Summary table ──────────────────────────────────────────────────────────────

def _grand_summary(results: list[dict], baseline: bool) -> None:
    print(f"\n\n{_SEP}")
    print(f"GRAND SUMMARY  ({len(results)} windows)")
    print(_SEP)

    hdr = f"  {'Label':22} {'Sess':>4}  {'Raw':>5}  {'Out':>4}  {'Reduc%':>6}  {'Recall':>7}  {'Rescued':>7}"
    if baseline:
        hdr += f"  {'Base':>7}  {'Δ':>5}"
    print(hdr)
    print("  " + "-" * (_W - 2))

    sum_reduction = sum_recall = sum_base = 0.0
    sum_rescued = 0
    n_with_base = 0

    for r in results:
        st: DistilStats = r["stats"]
        sc = r["score"]
        lbl = r["label"][:22]
        row = (
            f"  {lbl:22} {st.nsess:4}  "
            f"{st.raw_chars//1000:4}k  {st.out_chars//1000:3}k  "
            f"{st.reduction_pct:5.1f}%  "
            f"{sc['recall']:6.1%}  {st.n_entity_rescued:7}"
        )
        if baseline and r["base_score"]:
            bs = r["base_score"]
            delta = sc["recall"] - bs["recall"]
            sign = "+" if delta >= 0 else ""
            row += f"  {bs['recall']:6.1%}  {sign}{delta:.1%}"
            sum_base += bs["recall"]
            n_with_base += 1
        print(row)

        sum_reduction += st.reduction_pct
        sum_recall    += sc["recall"]
        sum_rescued   += st.n_entity_rescued

    n = len(results)
    print("  " + "-" * (_W - 2))
    avg_row = (
        f"  {'AVERAGE':22} {'':4}  {'':5}  {'':4}  "
        f"{sum_reduction/n:5.1f}%  {sum_recall/n:6.1%}  {sum_rescued//n:7}"
    )
    if baseline and n_with_base:
        avg_delta = (sum_recall / n) - (sum_base / n_with_base)
        sign = "+" if avg_delta >= 0 else ""
        avg_row += f"  {sum_base/n_with_base:6.1%}  {sign}{avg_delta:.1%}"
    print(avg_row)


# ── CLI ────────────────────────────────────────────────────────────────────────

def _parse() -> argparse.Namespace:
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("hours", nargs="*", help="specific hours (YYYY-MM-DDTHH)")
    p.add_argument("--hours", dest="n_hours", type=int, default=6, metavar="N",
                   help="auto-pick N peak hours (default 6)")
    p.add_argument("--range", nargs=2, metavar=("START", "END"),
                   help="compress an arbitrary time window instead of whole hours")
    p.add_argument("--baseline", action="store_true",
                   help="also run hour_text.py and compare recall")
    p.add_argument("--save", action="store_true",
                   help="write distilled body to distil_<label>.txt")
    p.add_argument("--preview", type=int, default=8, metavar="N",
                   help="body preview lines per hour (0 to suppress, default 8)")
    return p.parse_args()


def main() -> None:
    args = _parse()

    if args.range:
        start, end = args.range
        result = _eval_range(start, end, args.baseline, args.save, args.preview)
        results = [result] if result else []
    else:
        hour_list = args.hours or _peak_hours(args.n_hours)
        if not hour_list:
            print("No hours found. Try specifying hours explicitly or check the DB.")
            sys.exit(1)
        print(f"Hours ({len(hour_list)}): {hour_list}")
        results = []
        for hour in hour_list:
            r = _eval_hour(hour, args.baseline, args.save, args.preview)
            if r:
                results.append(r)

    evict_embedder()

    if len(results) > 1:
        _grand_summary(results, args.baseline)
    elif results:
        print()


if __name__ == "__main__":
    main()
