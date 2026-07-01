"""Tune /classify_tasks sampling settings against a captured fixture.

The classifier fails ~40% of the time by spending its entire token budget inside
the <think> block and never emitting JSON (no </think> close → empty answer). This
harness replays ONE real captured input (report + candidates) against the live MLX
server across a grid of sampling settings, so we can see — without rerunning the
slow distill + activity_report stages each time — which settings actually stop the
runaway.

Capture a fixture first (one slow run, writes report+candidates to disk):

    CLASSIFY_DUMP_DIR=services/tests/fixtures/classify \
      <restart server with that env>, then trigger one /worklog_hour

Then sweep:

    services/.venv/bin/python scripts/sweep-classify.py \
        --fixture services/tests/fixtures/classify/classify_tier1_*.json --repeats 5

Per trial we read the response's think_tokens / output_tokens / matches:
  - closed </think>  ⟺  think_tokens > 0   (the route only counts think tokens when it closes)
  - runaway failure  ⟺  output_tokens == 0  (ran out of budget mid-think, no JSON)
  - legit no-match    ⟺  output_tokens > 0 and matches == []   (model decided nothing matched)

Reported per setting: failure rate (the number we want to drive to 0), match
distribution, avg think tokens, avg latency.
"""
from __future__ import annotations

import argparse
import glob
import json
import statistics
import sys
import time
import urllib.error
import urllib.request


# The settings grid. Each entry overrides the production defaults
# (temp=0.1, thinking_budget=6144, max_tokens=10240). Edit freely.
GRID: list[dict] = [
    # thinking_budget proved advisory/ineffective (budget=2048 still ran away),
    # so the grid centres on temperature (break the deterministic ramble) and
    # total headroom (give thinking room to finish AND answer).
    {"label": "baseline temp0.1",         "temp": 0.1, "thinking_budget": 6144, "max_tokens": 10240},
    {"label": "temp 0.4",                 "temp": 0.4, "thinking_budget": 6144, "max_tokens": 10240},
    {"label": "temp 0.7",                 "temp": 0.7, "thinking_budget": 6144, "max_tokens": 10240},
    {"label": "temp 1.0",                 "temp": 1.0, "thinking_budget": 6144, "max_tokens": 10240},
    {"label": "temp 0.7 + 16k cap",       "temp": 0.7, "thinking_budget": 8192, "max_tokens": 16384},
    {"label": "temp 0.1 + 16k cap",       "temp": 0.1, "thinking_budget": 8192, "max_tokens": 16384},
]


def _post(base: str, body: dict, timeout: int) -> dict:
    req = urllib.request.Request(
        f"{base}/classify_tasks", data=json.dumps(body).encode(),
        method="POST", headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=timeout) as r:
        return json.loads(r.read())


def _classify(resp: dict) -> str:
    """Bucket one response: runaway failure / legit no-match / match."""
    out = resp.get("output_tokens", 0)
    n = len(resp.get("matches", []))
    if out == 0:
        return "FAIL"          # ran out mid-think, no JSON
    return "match" if n > 0 else "no-match"


def _one_call(base: str, fx: dict, *, temp, thinking_budget, max_tokens, timeout) -> dict:
    """Fire ONE /classify_tasks call with the given settings, print the result."""
    body = {
        "report":          fx["report"],
        "candidates":      fx["candidates"],
        "tier":            fx.get("tier", 1),
        "tier_note":       fx.get("tier_note", ""),
        "max_tokens":      max_tokens,
        "temp":            temp,
        "thinking_budget": thinking_budget,
    }
    print(f"call: temp={temp} thinking_budget={thinking_budget} max_tokens={max_tokens}")
    t0 = time.time()
    resp = _post(base, body, timeout)
    dt = time.time() - t0
    b = _classify(resp)
    closed = "yes" if resp.get("think_tokens", 0) > 0 else "NO (ran away)"
    print(f"  result : {b.upper()}")
    print(f"  closed </think>: {closed}")
    print(f"  think_tokens   : {resp.get('think_tokens', 0)}")
    print(f"  output_tokens  : {resp.get('output_tokens', 0)}")
    print(f"  matches        : {len(resp.get('matches', []))}  "
          f"{[m.get('task_key') for m in resp.get('matches', [])]}")
    print(f"  elapsed        : {dt:.0f}s")
    return resp


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--fixture", required=True, help="path/glob to a captured classify fixture JSON")
    ap.add_argument("--base-url", default="http://127.0.0.1:7823")
    ap.add_argument("--timeout", type=int, default=300)
    # Single-call mode (default): one call with these settings.
    ap.add_argument("--temp", type=float, default=0.1)
    ap.add_argument("--thinking-budget", type=int, default=6144)
    ap.add_argument("--max-tokens", type=int, default=10240)
    # Grid mode (opt-in): only when you ask for it.
    ap.add_argument("--grid", action="store_true", help="run the full GRID instead of one call")
    ap.add_argument("--repeats", type=int, default=3, help="trials per setting (grid mode only)")
    args = ap.parse_args()

    paths = sorted(glob.glob(args.fixture))
    if not paths:
        sys.exit(f"error: no fixture matched {args.fixture!r}")
    fx = json.loads(open(paths[0]).read())
    base = args.base_url.rstrip("/")
    print(f"fixture: {paths[0]}")
    print(f"  tier={fx.get('tier')} candidates={len(fx.get('candidates', []))} "
          f"report_chars={len(fx.get('report', ''))}\n")

    # Default: ONE call. Grid is opt-in via --grid.
    if not args.grid:
        _one_call(base, fx, temp=args.temp, thinking_budget=args.thinking_budget,
                  max_tokens=args.max_tokens, timeout=args.timeout)
        return

    rows = []
    for setting in GRID:
        buckets = {"FAIL": 0, "no-match": 0, "match": 0}
        think, out, secs = [], [], []
        for i in range(args.repeats):
            body = {
                "report":          fx["report"],
                "candidates":      fx["candidates"],
                "tier":            fx.get("tier", 1),
                "tier_note":       fx.get("tier_note", ""),
                "max_tokens":      setting["max_tokens"],
                "temp":            setting["temp"],
                "thinking_budget": setting["thinking_budget"],
            }
            t0 = time.time()
            try:
                resp = _post(base, body, args.timeout)
            except (urllib.error.URLError, TimeoutError) as e:
                print(f"  [{setting['label']}] trial {i+1}: request error {e}")
                buckets["FAIL"] += 1
                continue
            dt = time.time() - t0
            b = _classify(resp)
            buckets[b] += 1
            think.append(resp.get("think_tokens", 0))
            out.append(resp.get("output_tokens", 0))
            secs.append(dt)
            print(f"  [{setting['label']:<22}] trial {i+1}/{args.repeats}: "
                  f"{b:<8} think={resp.get('think_tokens',0):>5} out={resp.get('output_tokens',0):>4} "
                  f"matches={len(resp.get('matches',[]))} {dt:.0f}s")
        n = max(args.repeats, 1)
        rows.append({
            "label":     setting["label"],
            "fail_pct":  100 * buckets["FAIL"] / n,
            "match":     buckets["match"],
            "nomatch":   buckets["no-match"],
            "fail":      buckets["FAIL"],
            "avg_think": statistics.mean(think) if think else 0,
            "avg_s":     statistics.mean(secs) if secs else 0,
        })

    print("\n" + "=" * 92)
    print(f"{'setting':<24} {'fail%':>6} {'match':>6} {'nomatch':>8} {'fail':>5} "
          f"{'avgThink':>9} {'avgSec':>7}")
    print("-" * 92)
    for r in sorted(rows, key=lambda x: x["fail_pct"]):
        print(f"{r['label']:<24} {r['fail_pct']:>5.0f}% {r['match']:>6} {r['nomatch']:>8} "
              f"{r['fail']:>5} {r['avg_think']:>9.0f} {r['avg_s']:>6.0f}s")
    print("=" * 92)
    print("lower fail% is better; avgThink near the budget cap signals runaway risk.")


if __name__ == "__main__":
    main()
