"""BERT-style cross-encoder reranker (single relevance logit) over the shared
daily-plan eval. Usage: python eval_ce.py <repo>. One model per process (mem frees on exit)."""
import sys, os, json, time, math
import torch
from sentence_transformers import CrossEncoder
from plans import load as load_plans, report

BASE = os.path.join(os.path.dirname(os.path.abspath(__file__)), "results")
REPO = sys.argv[1]
plans, docs = load_plans()
dev = "mps" if torch.backends.mps.is_available() else "cpu"
print(f"loading {REPO} on {dev} ...", flush=True)
ce = CrossEncoder(REPO, device=dev, trust_remote_code=True, max_length=1024)

def sig(x):
    x = float(x)
    return x if 0.0 <= x <= 1.0 else 1/(1+math.exp(-x))   # normalize logits to 0-1

t0 = time.time()
ranked_all = []
for p in plans:
    pairs = [[p["summary"][:2000], docs[k]] for k in p["candidates"]]
    sc = ce.predict(pairs, convert_to_numpy=True, show_progress_bar=False)
    ranked = sorted(((k, sig(s)) for k, s in zip(p["candidates"], sc)), key=lambda x: -x[1])
    ranked_all.append((p, ranked))
dt = time.time() - t0
peak = torch.mps.driver_allocated_memory()/1e9 if dev == "mps" else 0.0
report(REPO.split("/")[-1], ranked_all)
json.dump({p["id"]: r for p, r in ranked_all}, open(f"{BASE}/scores_{REPO.split('/')[-1]}.json", "w"))
print(f"\n{len(plans)} sessions in {dt:.0f}s, ~{peak:.2f}GB mps allocated")
