"""Shared daily-plan construction so every reranker sees identical inputs.

Two modes:
  load()       — old sessions_all.json + labels_all.py, randomly-sampled 3-candidate plans
  load_week()  — sessions_week.json + labels_week.py, real per-day working-set candidates
"""
import json, random, os, sys

ROOT = os.path.dirname(os.path.abspath(__file__))
BASE = os.path.join(ROOT, "data")
sys.path.insert(0, BASE)


def _make_docs(tickets):
    """Build ticket doc strings from a tickets dict {key: {...}} or list [{...}]."""
    if isinstance(tickets, list):
        tickets = {t["task_key"]: t for t in tickets}
    return {k: f"[{t['issue_type']}] {t['title']}. Epic: {t.get('epic_title') or ''}. "
               f"{(t.get('description_text') or '').strip().replace(chr(10),' ')[:600]}".strip()
            for k, t in tickets.items()}


def load(size=3, mode="full", seed=7):
    """Original dataset: sessions_all.json + labels_all.py, randomly-sampled candidates."""
    from labels_all import L as _L
    tasks = {t["task_key"]: t for t in json.load(open(f"{BASE}/tasks.json"))}
    ALL = list(tasks.keys())
    DEV = ["KAN-109", "KAN-199", "KAN-200", "KAN-220", "KAN-231", "KAN-239", "KAN-240", "KAN-241"]
    pool = DEV if mode == "dev" else ALL
    sessions = json.load(open(f"{BASE}/sessions_all.json"))
    docs = _make_docs(tasks)
    rng = random.Random(seed); plans = []
    for s in sessions:
        primary, okset, unc, note = _L[s["id"]]; acc = set(okset)
        if primary == "NONE":
            cand = rng.sample(pool, size); truth = "NONE"
        else:
            p = [k for k in pool if k not in acc]
            cand = rng.sample(p, min(size - 1, len(p))) + [primary]; truth = primary
        rng.shuffle(cand)
        plans.append({"id": s["id"], "summary": s["session_summary"], "candidates": cand,
                      "acceptable": acc, "truth": truth, "uncertain": unc, "note": note,
                      "session_type": "task" if primary != "NONE" else "untracked"})
    return plans, docs


def load_week():
    """New dataset: sessions_week.json + labels_week.py, real per-day working-set candidates.
    Each session already carries its candidate list — no random sampling needed."""
    from labels_week import L as _L
    tickets = json.load(open(f"{BASE}/tickets.json"))
    sessions = json.load(open(f"{BASE}/sessions_week.json"))
    docs = _make_docs(tickets)
    plans = []
    for s in sessions:
        sid = s["id"]
        primary, okset, unc, stype, note = _L[sid]
        acc = set(okset)
        cand = s["candidates"]          # pre-computed per-day working set
        truth = primary                 # "NONE" or a KAN-xxx
        plans.append({"id": sid, "summary": s["session_summary"], "candidates": cand,
                      "acceptable": acc, "truth": truth, "uncertain": unc, "note": note,
                      "session_type": stype, "day": s["day"]})
    return plans, docs

def report(name, ranked_all, thrs=(0.08, 0.1, 0.12, 0.15, 0.2, 0.3, 0.4, 0.5)):
    """ranked_all: list of (plan, [(key,score)... desc]). Prints THR sweep + per-type breakdown."""
    n = len(ranked_all); best = None
    print(f"\n=== {name} | {n} sessions ===")
    for THR in thrs:
        hit = fb = rmiss = uh = ut = 0
        for p, ranked in ranked_all:
            k, sc = ranked[0]; pred = k if sc >= THR else "NONE"
            ok = pred in (p["acceptable"] | ({"NONE"} if p["truth"] == "NONE" else set())); hit += ok
            if p["uncertain"]: ut += 1; uh += ok
            if pred != "NONE" and p["truth"] == "NONE": fb += 1
            if not ok and pred == "NONE" and p["truth"] != "NONE": rmiss += 1
        print(f"  THR={THR}: {hit}/{n}={hit/n:.0%}  conf {hit-uh}/{n-ut}={(hit-uh)/(n-ut):.0%}  "
              f"false-binds={fb}  recall-misses={rmiss}")
        if best is None or hit > best[0]: best = (hit, THR, fb)
    print(f"  BEST: {best[0]}/{n}={best[0]/n:.0%} @ THR={best[1]} (false-binds={best[2]})")

    # Per session_type breakdown at best threshold
    THR = best[1]
    by_type = {}
    misses = []
    for p, ranked in ranked_all:
        k, sc = ranked[0]; pred = k if sc >= THR else "NONE"
        ok = pred in (p["acceptable"] | ({"NONE"} if p["truth"] == "NONE" else set()))
        stype = p.get("session_type", "?")
        by_type.setdefault(stype, [0, 0])
        by_type[stype][1] += 1
        if ok: by_type[stype][0] += 1
        if not ok:
            misses.append((p["id"], stype, p["truth"], pred, round(sc, 3), p.get("note","")[:55]))
    print(f"\n  Per-type @ THR={THR}:")
    for t, (h, tot) in sorted(by_type.items()):
        print(f"    {t:10s}: {h}/{tot} = {h/tot:.0%}")
    if misses:
        print(f"\n  Misses ({len(misses)}):")
        for sid, st, truth, pred, sc, note in misses:
            print(f"    {sid} [{st}] truth={truth} pred={pred} sc={sc}  {note}")
    return best
