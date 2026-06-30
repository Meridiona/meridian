"""Accuracy eval + temperature tuning for /classify_tasks (FSM endpoint).

Runs the labeled cases in services/tests/fixtures/classify/eval_cases.json against
the live endpoint across a temperature grid, and scores each call by EXACT SET MATCH
of returned task_keys vs the expected set. The case set is balanced (some expect a
match, some expect abstention) so neither an always-match nor an always-abstain
policy can score well — this is what guards against an over- or under-fit prompt.

/classify_tasks now decodes with outlines FSM (grammar-constrained JSON), thinking
OFF — so the JSON is always structurally valid (``fail`` should be 0) and the only
sampling knob that matters is ``temp``. (The endpoint still accepts ``thinking_budget``
/ ``enable_thinking`` for backward-compat, but they are no-ops; this harness no longer
sends or sweeps them.)

Metrics per setting:
  - acc        : fraction of (case, repeat) trials whose returned key-set == expected
  - FP         : trials that returned a key that should NOT have matched (over-match)
  - FN         : trials that missed a key that SHOULD have matched (under-match)
  - fail       : trials with empty output (should be 0 under FSM — a non-zero value
                 means the endpoint is NOT on the FSM path)
  - per-case   : exact-match rate for each labelled case

Usage:
    services/.venv/bin/python scripts/eval-classify.py --repeats 3
    services/.venv/bin/python scripts/eval-classify.py --temp 0.1            # single temp
"""
from __future__ import annotations

import argparse
import json
import sys
import time
import urllib.error
import urllib.request
from pathlib import Path

CASES_PATH = Path("services/tests/fixtures/classify/eval_cases.json")
FIXTURE_PATH = Path("services/tests/fixtures/classify/classify_tier1_today_s_confirmed_plan.json")

# Temperatures to compare (FSM is always on; temp is the only live knob).
GRID = [
    {"temp": 0.1},   # production default
    {"temp": 0.4},
    {"temp": 0.7},
]


def _post(base: str, body: dict, timeout: int) -> dict:
    req = urllib.request.Request(
        f"{base}/classify_tasks", data=json.dumps(body).encode(),
        method="POST", headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=timeout) as r:
        return json.loads(r.read())


def _load_cases() -> tuple[dict, str]:
    data = json.loads(CASES_PATH.read_text())
    fixture_report = json.loads(FIXTURE_PATH.read_text())["report"]
    return data, fixture_report


def _candidates(data: dict, keys: list[str]) -> list[dict]:
    out = []
    for k in keys:
        t = data["tickets"][k]
        out.append({"task_key": k, "title": t["title"], "doc": t.get("doc", ""), "rerank_score": 0.0})
    return out


def _resolve_report(data: dict, fixture_report: str, ref: str) -> str:
    return fixture_report if ref == "FIXTURE" else data["reports"][ref]


def run_setting(base, data, fixture_report, setting, repeats, timeout):
    cases = data["cases"]
    trials = 0
    exact = 0
    fp = fn = fail = 0
    per_case = {}
    for case in cases:
        report = _resolve_report(data, fixture_report, case["report"])
        cands = _candidates(data, case["candidates"])
        expected = set(case["expected"])
        ok = 0
        for _ in range(repeats):
            body = {
                "report": report, "candidates": cands, "tier": 1,
                "tier_note": "eval", "max_tokens": 4096,
                "temp": setting["temp"],
            }
            try:
                resp = _post(base, body, timeout)
            except (urllib.error.URLError, TimeoutError) as e:
                print(f"    {case['id']}: request error {e}")
                fail += 1; trials += 1
                continue
            trials += 1
            got = {m.get("task_key") for m in resp.get("matches", []) if isinstance(m, dict)}
            if resp.get("output_tokens", 0) == 0:
                fail += 1
            if got == expected:
                exact += 1; ok += 1
            fp += len(got - expected)
            fn += len(expected - got)
            mark = "✓" if got == expected else "✗"
            print(f"    {case['id']:<22} exp={sorted(expected) or '∅'} got={sorted(got) or '∅'} {mark}")
        per_case[case["id"]] = ok / max(repeats, 1)
    return {
        "setting": setting, "trials": trials,
        "acc": exact / max(trials, 1), "fp": fp, "fn": fn, "fail": fail,
        "per_case": per_case,
    }


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--base-url", default="http://127.0.0.1:7823")
    ap.add_argument("--timeout", type=int, default=200)
    ap.add_argument("--repeats", type=int, default=2)
    ap.add_argument("--temp", type=float, help="single-setting mode: run only this temperature")
    args = ap.parse_args()

    data, fixture_report = _load_cases()
    base = args.base_url.rstrip("/")
    grid = [{"temp": args.temp}] if args.temp is not None else GRID

    print(f"{len(data['cases'])} cases × {args.repeats} repeats × {len(grid)} settings "
          f"= {len(data['cases'])*args.repeats*len(grid)} calls\n")

    results = []
    for s in grid:
        print(f"=== setting decode=fsm temp={s['temp']} ===")
        t0 = time.time()
        r = run_setting(base, data, fixture_report, s, args.repeats, args.timeout)
        r["secs"] = time.time() - t0
        results.append(r)
        print(f"  -> acc={r['acc']:.0%}  FP={r['fp']} FN={r['fn']} fail={r['fail']}  ({r['secs']:.0f}s)\n")

    print("=" * 64)
    print(f"{'decode':>8} {'temp':>5} {'acc':>6} {'FP':>4} {'FN':>4} {'fail':>5} {'avgSec':>7}")
    print("-" * 64)
    for r in sorted(results, key=lambda x: (-x["acc"], x["fp"] + x["fn"])):
        s = r["setting"]
        avg = r["secs"] / max(r["trials"], 1)
        print(f"{'fsm':>8} {s['temp']:>5} {r['acc']:>5.0%} "
              f"{r['fp']:>4} {r['fn']:>4} {r['fail']:>5} {avg:>6.1f}s")
    print("=" * 64)
    # Per-case breakdown for the best setting.
    best = max(results, key=lambda x: x["acc"])
    print(f"per-case exact-match (best: temp={best['setting']['temp']}):")
    for cid, rate in best["per_case"].items():
        print(f"  {cid:<24} {rate:.0%}")


if __name__ == "__main__":
    main()
