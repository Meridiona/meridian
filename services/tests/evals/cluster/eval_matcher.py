"""Eval the worklog-pipeline tiered matcher on the 8 hand-labeled reports.

Compares the agno + schema-enforced matcher to the prior benchmark
(matcher_benchmark_results.json: 2B-think-alone 50%/4-FB, reranker 75%/0-FB).

Each labeled report's daily plan is the Tier-1 candidate set. Scoring:
  label == "untracked"  → correct iff the matcher proposes a new task (no bind)
  label == "KAN-xxx"     → correct iff that key is among the matched bindings

Run (against a server on :7824 for testing, :7823 in prod):
  services/.venv/bin/python services/tests/evals/cluster/eval_matcher.py --server http://127.0.0.1:7824
"""
from __future__ import annotations

import argparse
import json
import sys
import time
import urllib.request
from pathlib import Path

ROOT = Path(__file__).parent
SERVICES = ROOT.parents[2]
sys.path.insert(0, str(SERVICES))

from agents.worklog_pipeline.agent_io import make_match_agent_factory  # noqa: E402
from agents.worklog_pipeline.match import Candidate, match_hour        # noqa: E402
from agents.worklog_pipeline.prompts.match_tasks import SYSTEM as MATCH_SYSTEM  # noqa: E402

DATA = json.loads((ROOT / "labeled_reports.json").read_text())
TICKETS = json.loads((SERVICES / "tests/evals/rerank/data/tickets.json").read_text())


def ticket_doc(key: str) -> str:
    t = TICKETS.get(key, {})
    desc = (t.get("description_text") or "").strip().replace("\n", " ")[:300]
    return f"[{t.get('issue_type', 'Task')}] {t.get('title', key)}. {desc}".strip()


def rerank(server: str, query: str, keys: list[str]) -> dict[str, float]:
    cands = [{"task_key": k, "doc": ticket_doc(k)} for k in keys]
    body = json.dumps({"query": query[:1800], "candidates": cands}).encode()
    req = urllib.request.Request(
        f"{server}/rerank", data=body, headers={"Content-Type": "application/json"}
    )
    with urllib.request.urlopen(req, timeout=120) as r:
        ranked = json.loads(r.read())["ranked"]
    return {row["task_key"]: row["score"] for row in ranked}


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--server", default="http://127.0.0.1:7824")
    ap.add_argument("--no-rerank", action="store_true", help="skip reranker hints")
    args = ap.parse_args()

    factory = make_match_agent_factory(MATCH_SYSTEM, server_url=args.server)

    rows = []
    for r in DATA:
        daily_keys = r["daily_plan"]
        scores = {} if args.no_rerank else rerank(args.server, r["report"], daily_keys)
        daily = [
            Candidate(task_key=k, title=TICKETS.get(k, {}).get("title", k),
                      doc=ticket_doc(k), rerank_score=scores.get(k, 0.0))
            for k in daily_keys
        ]
        t0 = time.monotonic()
        outcome = match_hour(factory, r["report"], daily, backlog=[])
        elapsed = time.monotonic() - t0

        matched = [b.task_key for b in outcome.bindings]
        conf = {b.task_key: round(b.confidence, 2) for b in outcome.bindings}
        label = r["label"]
        if label == "untracked":
            ok = outcome.propose_new and not matched
        else:
            ok = label in matched
        rows.append({"hour": r["hour"], "label": label, "matched": matched, "conf": conf,
                     "propose_new": outcome.propose_new, "ok": ok, "t": round(elapsed, 1)})
        shown = [f"{k}@{conf[k]}" for k in matched] or ["∅"]
        print(f"  {'✓' if ok else '✗'} {r['hour']}  label={label:12}  "
              f"matched={shown}  propose_new={outcome.propose_new}  {elapsed:.1f}s",
              flush=True)

    n = len(rows)
    correct = sum(x["ok"] for x in rows)
    task_rows = [x for x in rows if x["label"] != "untracked"]
    untr_rows = [x for x in rows if x["label"] == "untracked"]
    false_binds = sum(1 for x in untr_rows if x["matched"])
    recall_miss = sum(1 for x in task_rows if not x["matched"])
    print(f"\n  agno+schema matcher: {correct}/{n} = {correct/n:.0%}  "
          f"task={sum(x['ok'] for x in task_rows)}/{len(task_rows)}  "
          f"untracked={sum(x['ok'] for x in untr_rows)}/{len(untr_rows)}  "
          f"false-binds={false_binds}  recall-miss={recall_miss}")
    print("  baseline: 2B-think-alone 50%/4-FB · reranker 75%/0-FB")

    (ROOT / "matcher_agno_results.json").write_text(json.dumps(rows, indent=2))


if __name__ == "__main__":
    main()
