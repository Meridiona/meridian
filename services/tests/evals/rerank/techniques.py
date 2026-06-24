"""Push the 0.6B Qwen3-Reranker toward ~100% with principled, sub-2GB techniques.
Loads the model once; for each (session, candidate) computes scores under several
document representations and instructions, then evaluates:
  - doc representation: title-only / desc-only / combined / MAXPOOL over reps
  - instruction variant
  - score-margin gate (top1 - top2)
All stay at ~1.5GB (single 0.6B model). Dumps per-candidate scores for ensembling."""
import os, json, time, mlx.core as mx, mlx.nn as nn
from mlx_lm import load
from plans import load as load_plans, report

ROOT = os.path.dirname(os.path.abspath(__file__))
DATA = os.path.join(ROOT, "data")
BASE = os.path.join(ROOT, "results")
REPO = "kerncore/Qwen3-Reranker-0.6B-MLX-4bit"
PREFIX = ("<|im_start|>system\nJudge whether the Document meets the requirements based on the Query and the "
          "Instruct provided. Note that the answer can only be \"yes\" or \"no\".<|im_end|>\n<|im_start|>user\n")
SUFFIX = "<|im_end|>\n<|im_start|>assistant\n<think>\n\n</think>\n\n"

INSTR = {
 "base": ("Given a software-engineering work-session summary (the Query), judge whether the work described "
          "advances the goal of the project-management ticket (the Document). Answer yes only if completing "
          "this work would make progress on that specific ticket."),
 "entity": ("The Query is a developer work-session summary; the Document is a Jira ticket. Answer yes only if "
            "the session's primary objective is the SAME feature/component this ticket is about (e.g. classifier "
            "vs worklog vs eval-framework vs landing-page are different objectives). Reject keyword overlap without "
            "shared goal."),
}

tasks = {t["task_key"]: t for t in json.load(open(f"{DATA}/tasks.json"))}
def rep(t, kind):
    title = f"[{t['issue_type']}] {t['title']}"
    epic = f" Epic: {t.get('epic_title') or ''}."
    desc = (t.get("description_text") or "").strip().replace("\n", " ")[:600]
    if kind == "title": return f"{title}.{epic}"
    if kind == "desc":  return f"{title}. {desc}".strip()
    return f"{title}.{epic} {desc}".strip()   # combined

plans, _ = load_plans()
print(f"loading {REPO} ...", flush=True)
model, tok = load(REPO)
print(f"active mem {mx.get_active_memory()/1e9:.2f} GB", flush=True)
yes_id = tok.encode("yes", add_special_tokens=False)[0]; no_id = tok.encode("no", add_special_tokens=False)[0]
_cache = {}
def score(q, d, instr):
    key = (q, d, instr)            # full strings — truncating collides reps that share a prefix
    if key in _cache: return _cache[key]
    ids = tok.encode(f"{PREFIX}<Instruct>: {INSTR[instr]}\n<Query>: {q}\n<Document>: {d}{SUFFIX}", add_special_tokens=False)
    lg = model(mx.array([ids]))[0, -1, :]
    v = nn.softmax(mx.array([lg[no_id].item(), lg[yes_id].item()]))[1].item()
    _cache[key] = v; return v

t0 = time.time()
# precompute scores: per plan, per candidate -> {(instr,repkind): score}
raw = []
for p in plans:
    row = {}
    for k in p["candidates"]:
        t = tasks[k]
        row[k] = {(instr, kind): score(p["summary"], rep(t, kind), instr)
                  for instr in INSTR for kind in ("title", "desc", "combined")}
    raw.append((p, row))
print(f"scored in {time.time()-t0:.0f}s, peak mem {mx.get_peak_memory()/1e9:.2f} GB")

def ranked_for(agg):
    out = []
    for p, row in raw:
        out.append((p, sorted(((k, agg(sc)) for k, sc in row.items()), key=lambda x: -x[1])))
    return out

# --- evaluate techniques ---
configs = {
 "base/combined  (current)": lambda sc: sc[("base", "combined")],
 "base/title-only":          lambda sc: sc[("base", "title")],
 "base/MAXPOOL(t,d,c)":      lambda sc: max(sc[("base","title")], sc[("base","desc")], sc[("base","combined")]),
 "entity/combined":          lambda sc: sc[("entity", "combined")],
 "entity/MAXPOOL":           lambda sc: max(sc[("entity","title")], sc[("entity","desc")], sc[("entity","combined")]),
 "INSTR-ens MAXPOOL(all 6)": lambda sc: max(sc.values()),
 "INSTR-ens MEAN(base+ent comb)": lambda sc: (sc[("base","combined")]+sc[("entity","combined")])/2,
}
best_overall = []
for name, agg in configs.items():
    b = report(name, ranked_for(agg))
    best_overall.append((name, b))

# dump the strongest single-rep scores for cross-model ensembling
json.dump({p["id"]: [(k, max(row[k][("base","title")], row[k][("base","desc")], row[k][("base","combined")]))
                     for k in p["candidates"]] for p, row in raw},
          open(f"{BASE}/scores_06b_maxpool.json", "w"))
print("\nSUMMARY (best acc @ best THR, 0 unless noted):")
for name, b in best_overall:
    print(f"  {name:32s} {b[0]}/85={b[0]/85:.0%} @THR={b[1]} fb={b[2]}")
