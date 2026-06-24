"""Ensemble 0.6B + 4B reranker scores (offline) and characterize residual misses
at the 0.6B operating point (THR=0.08). Determines whether the ceiling is model
error or contestable labels."""
import os, json
from plans import load as load_plans

BASE = os.path.join(os.path.dirname(os.path.abspath(__file__)), "results")
plans, _ = load_plans()
pm = {p["id"]: p for p in plans}
s06 = {int(k): dict(v) for k, v in json.load(open(f"{BASE}/scores_Qwen3-Reranker-0.6B-MLX-4bit.json")).items()}
s4b = {int(k): dict(v) for k, v in json.load(open(f"{BASE}/scores_Qwen3-Reranker-4B-4bit-MLX.json")).items()}

def evaluate(name, scorefn, thr):
    hit = fb = 0; miss = []
    for sid, p in pm.items():
        sc = scorefn(sid, p["candidates"])
        k, v = max(sc.items(), key=lambda x: x[1]); pred = k if v >= thr else "NONE"
        ok = pred in (p["acceptable"] | ({"NONE"} if p["truth"] == "NONE" else set())); hit += ok
        if pred != "NONE" and p["truth"] == "NONE": fb += 1
        if not ok: miss.append((sid, pred, round(v, 2), p["truth"], p["uncertain"], p["note"][:44]))
    n = len(pm)
    print(f"{name:34s} {hit}/{n}={hit/n:.0%}  fb={fb}  misses={len(miss)}")
    return miss

for thr in (0.08, 0.1):
    evaluate(f"0.6B only THR={thr}", lambda sid, c: s06[sid], thr)
    evaluate(f"4B only THR={thr}", lambda sid, c: s4b[sid], thr)
    evaluate(f"ens MEAN THR={thr}", lambda sid, c: {k: (s06[sid][k]+s4b[sid][k])/2 for k in c}, thr)
    evaluate(f"ens MAX THR={thr}", lambda sid, c: {k: max(s06[sid][k], s4b[sid][k]) for k in c}, thr)
    print()

print("=== residual misses of ens-MAX(0.6B,4B) @ THR=0.08  ([U]=contestable label) ===")
miss = evaluate("ensMAX @0.08", lambda sid, c: {k: max(s06[sid][k], s4b[sid][k]) for k in c}, 0.08)
for sid, pred, v, truth, unc, note in miss:
    tag = "[U]" if unc else "   "
    print(f"  {tag} s{sid} pred={pred:8s}({v}) truth={truth:8s} | {note}")
