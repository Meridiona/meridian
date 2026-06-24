"""Per-session latency for the 0.6B reranker: one-time model load vs per-session
inference (scoring all 2-4 plan candidates). Run with the services venv."""
import os, sys, time, statistics, mlx.core as mx, mlx.nn as nn
from mlx_lm import load
from plans import load as load_plans

REPO = sys.argv[1] if len(sys.argv) > 1 else "kerncore/Qwen3-Reranker-0.6B-MLX-4bit"
INSTRUCT = ("Given a software-engineering work-session summary (the Query), judge whether the work described "
            "advances the goal of the project-management ticket (the Document). Answer yes only if completing "
            "this work would make progress on that specific ticket.")
PREFIX = ("<|im_start|>system\nJudge whether the Document meets the requirements based on the Query and the "
          "Instruct provided. Note that the answer can only be \"yes\" or \"no\".<|im_end|>\n<|im_start|>user\n")
SUFFIX = "<|im_end|>\n<|im_start|>assistant\n<think>\n\n</think>\n\n"

plans, docs = load_plans()
t0 = time.time(); model, tok = load(REPO); load_s = time.time() - t0
yes_id = tok.encode("yes", add_special_tokens=False)[0]; no_id = tok.encode("no", add_special_tokens=False)[0]
def score(q, d):
    ids = tok.encode(f"{PREFIX}<Instruct>: {INSTRUCT}\n<Query>: {q}\n<Document>: {d}{SUFFIX}", add_special_tokens=False)
    lg = model(mx.array([ids]))[0, -1, :]
    return nn.softmax(mx.array([lg[no_id].item(), lg[yes_id].item()]))[1].item()

# warm up (first call pays graph-compile cost)
score(plans[0]["summary"], docs[plans[0]["candidates"][0]])

per_session = []
for p in plans:
    t = time.time()
    for k in p["candidates"]:
        score(p["summary"], docs[k])
    per_session.append((time.time() - t, len(p["candidates"]), len(p["summary"])))

times = [x[0] for x in per_session]
print(f"\n=== {REPO.split('/')[-1]} per-session latency ===")
print(f"model load (one-time, cold): {load_s:.1f}s")
print(f"per session (scores all {min(x[1] for x in per_session)}-{max(x[1] for x in per_session)} candidates):")
print(f"  mean   {statistics.mean(times)*1000:.0f} ms")
print(f"  median {statistics.median(times)*1000:.0f} ms")
print(f"  min    {min(times)*1000:.0f} ms   max {max(times)*1000:.0f} ms")
print(f"  per-candidate ~{statistics.mean(times)/statistics.mean(x[1] for x in per_session)*1000:.0f} ms")
print(f"peak mem {mx.get_peak_memory()/1e9:.2f} GB")
