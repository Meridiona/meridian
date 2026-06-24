"""MLX Qwen3-Reranker (yes/no logit) over the shared daily-plan eval.
Usage:
  python eval_mlx.py [repo] [--week]
  python eval_mlx.py kerncore/Qwen3-Reranker-4B-4bit-MLX --week
Flags:
  --week   use sessions_week.json + labels_week.py (real per-day candidates)
  default: sessions_all.json + labels_all.py (old random-sample dataset)
"""
import sys, os, json, time, mlx.core as mx, mlx.nn as nn
from mlx_lm import load
from plans import load as load_plans, load_week, report

args = [a for a in sys.argv[1:] if not a.startswith("--")]
flags = [a for a in sys.argv[1:] if a.startswith("--")]
WEEK = "--week" in flags
REPO = args[0] if args else "kerncore/Qwen3-Reranker-0.6B-MLX-4bit"

BASE = os.path.join(os.path.dirname(os.path.abspath(__file__)), "results")
INSTRUCT = ("Given a software-engineering work-session summary (the Query), judge whether the work described "
            "advances the goal of the project-management ticket (the Document). Answer yes only if completing "
            "this work would make progress on that specific ticket.")
PREFIX = ("<|im_start|>system\nJudge whether the Document meets the requirements based on the Query and the "
          "Instruct provided. Note that the answer can only be \"yes\" or \"no\".<|im_end|>\n<|im_start|>user\n")
SUFFIX = "<|im_end|>\n<|im_start|>assistant\n<think>\n\n</think>\n\n"

plans, docs = load_week() if WEEK else load_plans()
tag = "week" if WEEK else "all"
print(f"dataset={tag}  sessions={len(plans)}  model={REPO}", flush=True)

print(f"loading {REPO} ...", flush=True)
model, tok = load(REPO)
print(f"active mem {mx.get_active_memory()/1e9:.2f} GB", flush=True)
yes_id = tok.encode("yes", add_special_tokens=False)[0]
no_id  = tok.encode("no",  add_special_tokens=False)[0]

def score(q, d):
    ids = tok.encode(f"{PREFIX}<Instruct>: {INSTRUCT}\n<Query>: {q}\n<Document>: {d}{SUFFIX}", add_special_tokens=False)
    lg = model(mx.array([ids]))[0, -1, :]
    return nn.softmax(mx.array([lg[no_id].item(), lg[yes_id].item()]))[1].item()

t0 = time.time()
ranked_all = [(p, sorted(((k, score(p["summary"], docs[k])) for k in p["candidates"]), key=lambda x: -x[1]))
              for p in plans]

model_tag = REPO.split("/")[-1]
out_name = f"scores_{model_tag}_{tag}.json"
report(f"{model_tag} [{tag}]", ranked_all)
json.dump({p["id"]: r for p, r in ranked_all}, open(f"{BASE}/{out_name}", "w"))
print(f"\n{len(plans)} sessions in {time.time()-t0:.0f}s, peak mem {mx.get_peak_memory()/1e9:.2f} GB")
print(f"scores → results/{out_name}")
