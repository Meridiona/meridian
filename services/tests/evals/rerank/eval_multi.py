"""Multi-model reranker eval — runs models sequentially, never two at once.
Usage:
  python eval_multi.py [--week] [--models qwen06 qwen06_8bit mxbai all]

Models:
  qwen06      kerncore/Qwen3-Reranker-0.6B-MLX-4bit     (0.4 GB)
  qwen06_8bit mlx-community/Qwen3-Reranker-0.6B-mxfp8   (0.7 GB)
  mxbai       mlx-community/mxbai-rerank-large-v2        (2.9 GB, Qwen2-1.5B, true/false)
  qwen4b      vserifsaglam/Qwen3-Reranker-4B-4bit-MLX    (3.3 GB)
  all         run all four
"""
import sys, os, json, time
import mlx.core as mx
import mlx.nn as nn
from mlx_lm import load
from plans import load as load_plans, load_week, report

args = [a for a in sys.argv[1:] if not a.startswith("--")]
flags = [a for a in sys.argv[1:] if a.startswith("--")]
WEEK = "--week" in flags
plans, docs = load_week() if WEEK else load_plans()
tag = "week" if WEEK else "all"
BASE = os.path.join(os.path.dirname(os.path.abspath(__file__)), "results")

# ── model registry ─────────────────────────────────────────────────────────────
MODELS = {
    "qwen06": {
        "repo": "kerncore/Qwen3-Reranker-0.6B-MLX-4bit",
        "style": "qwen3",
        "desc": "Qwen3-0.6B 4bit",
    },
    "qwen06_8bit": {
        "repo": "mlx-community/Qwen3-Reranker-0.6B-mxfp8",
        "style": "qwen3",
        "desc": "Qwen3-0.6B 8bit",
    },
    "mxbai": {
        "repo": "mlx-community/mxbai-rerank-large-v2",
        "style": "mxbai",
        "desc": "mxbai-rerank-large-v2 (Qwen2-1.5B)",
    },
    "qwen4b": {
        "repo": "vserifsaglam/Qwen3-Reranker-4B-4bit-MLX",
        "style": "qwen3",
        "desc": "Qwen3-4B 4bit",
    },
}

# which models to run
selected_arg = [a for a in args if a in list(MODELS.keys()) + ["all"]]
if not selected_arg or "all" in selected_arg:
    to_run = list(MODELS.keys())
else:
    to_run = selected_arg

# ── Qwen3-Reranker yes/no scorer ───────────────────────────────────────────────
QWEN3_INSTRUCT = (
    "Given a software-engineering work-session summary (the Query), judge whether the work described "
    "advances the goal of the project-management ticket (the Document). Answer yes only if completing "
    "this work would make progress on that specific ticket."
)
QWEN3_PREFIX = (
    "<|im_start|>system\nJudge whether the Document meets the requirements based on the Query and the "
    "Instruct provided. Note that the answer can only be \"yes\" or \"no\".<|im_end|>\n<|im_start|>user\n"
)
QWEN3_SUFFIX = "<|im_end|>\n<|im_start|>assistant\n<think>\n\n</think>\n\n"

# ── mxbai true/false scorer ────────────────────────────────────────────────────
MXBAI_INSTRUCT = (
    "Given a software-engineering work-session summary, judge whether the work described "
    "advances the goal of the project-management ticket. Answer true only if this work "
    "makes progress on that specific ticket."
)

def _qwen3_score(tok, model, q, d, yes_id, no_id):
    ids = tok.encode(
        f"{QWEN3_PREFIX}<Instruct>: {QWEN3_INSTRUCT}\n<Query>: {q}\n<Document>: {d}{QWEN3_SUFFIX}",
        add_special_tokens=False,
    )
    lg = model(mx.array([ids]))[0, -1, :]
    return nn.softmax(mx.array([lg[no_id].item(), lg[yes_id].item()]))[1].item()


def _mxbai_score(tok, model, q, d, true_id, false_id):
    prompt = (
        f"<|im_start|>system\n{MXBAI_INSTRUCT}<|im_end|>\n"
        f"<|im_start|>user\n<query>\n{q}\n</query>\n<document>\n{d}\n</document><|im_end|>\n"
        f"<|im_start|>assistant\n"
    )
    ids = tok.encode(prompt, add_special_tokens=False)
    lg = model(mx.array([ids]))[0, -1, :]
    return nn.softmax(mx.array([lg[false_id].item(), lg[true_id].item()]))[1].item()


# ── run each model ─────────────────────────────────────────────────────────────
results_summary = {}

for key in to_run:
    cfg = MODELS[key]
    repo = cfg["repo"]
    style = cfg["style"]
    desc = cfg["desc"]
    out_name = f"scores_{key}_{tag}.json"

    print(f"\n{'='*60}", flush=True)
    print(f"MODEL: {desc}  ({repo})", flush=True)
    print(f"Loading ...", flush=True)

    model, tok = load(repo)
    mem_gb = mx.get_active_memory() / 1e9
    print(f"Active mem: {mem_gb:.2f} GB", flush=True)

    if style == "qwen3":
        yes_id = tok.encode("yes", add_special_tokens=False)[0]
        no_id  = tok.encode("no",  add_special_tokens=False)[0]
        score_fn = lambda q, d: _qwen3_score(tok, model, q, d, yes_id, no_id)
    else:  # mxbai
        true_id  = tok.encode("true",  add_special_tokens=False)[0]
        false_id = tok.encode("false", add_special_tokens=False)[0]
        score_fn = lambda q, d: _mxbai_score(tok, model, q, d, true_id, false_id)

    t0 = time.time()
    ranked_all = [
        (p, sorted(((k, score_fn(p["summary"], docs[k])) for k in p["candidates"]),
                   key=lambda x: -x[1]))
        for p in plans
    ]
    elapsed = time.time() - t0
    peak_gb = mx.get_peak_memory() / 1e9

    best = report(f"{desc} [{tag}]", ranked_all)
    json.dump({p["id"]: r for p, r in ranked_all}, open(f"{BASE}/{out_name}", "w"))
    print(f"{len(plans)} sessions in {elapsed:.0f}s  peak {peak_gb:.2f} GB")
    print(f"scores → results/{out_name}", flush=True)

    results_summary[key] = {"best_hit": best[0], "n": len(plans), "thr": best[1],
                             "fb": best[2], "elapsed": elapsed, "peak_gb": peak_gb, "desc": desc}

    # explicitly unload before next model
    del model, tok
    mx.clear_cache()
    print(f"Unloaded {desc}.", flush=True)

# ── final comparison table ─────────────────────────────────────────────────────
print(f"\n{'='*60}")
print(f"COMPARISON [{tag}] — {len(plans)} sessions")
print(f"{'Model':<35} {'Acc':>5} {'THR':>5} {'FalseBind':>10} {'Time':>7} {'RAM':>6}")
print("-"*70)
for key in to_run:
    r = results_summary[key]
    acc = f"{r['best_hit']}/{r['n']}={r['best_hit']/r['n']:.0%}"
    print(f"{r['desc']:<35} {acc:>12} {r['thr']:>5} {r['fb']:>10}  {r['elapsed']:>5.0f}s {r['peak_gb']:>5.2f}G")
