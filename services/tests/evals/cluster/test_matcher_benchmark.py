"""Benchmark 3 matching approaches on 8 labeled activity reports.

Approaches (one model at a time — never two in memory):
  A: Qwen3.5-2B think-mode — reads full report, picks ticket or NONE
  B: Reranker 0.6B only    — scores daily plan, bind if score >= THR
  C: Reranker shortlist (top-2) → 2B-think confirm/reject

Scoring: correct if prediction matches label.
  label = "KAN-xxx" → prediction must be that key
  label = "untracked" → prediction must be NONE

Run: services/.venv/bin/python services/tests/evals/cluster/test_matcher_benchmark.py
"""
from __future__ import annotations
import gc, json, re, sys, time
from pathlib import Path

ROOT     = Path(__file__).parent
SERVICES = ROOT.parents[2]
sys.path.insert(0, str(SERVICES))

DATA    = json.loads((ROOT / "labeled_reports.json").read_text())
TICKETS = json.loads((SERVICES / "tests/evals/rerank/data/tickets.json").read_text())

KEY_RE = re.compile(r"\b[A-Z]+-\d{2,4}\b")

def ticket_doc(key: str) -> str:
    t = TICKETS.get(key, {})
    desc = (t.get("description_text") or "").strip().replace("\n"," ")[:400]
    return f"[{t.get('issue_type','Task')}] {t.get('title',key)}. {desc}".strip()

def parse_key(text: str, valid: set) -> str | None:
    for k in KEY_RE.findall(text):
        if k in valid: return k
    return None

def score_pred(pred: str | None, label: str) -> bool:
    if label == "untracked":
        return pred is None
    return pred == label

W = 70

# ─────────────────────────────────────────────────────────────────────────────
# APPROACH A — 2B-think only
# ─────────────────────────────────────────────────────────────────────────────
PROMPT_A = """A developer wrote this worklog for the last hour:

{report}

Today's tasks:
{tasks}

Which ONE task did this work directly advance? Answer ONLY with the task key (e.g. {example}) or NONE.
Rules: the work must actively progress the task goal — merely mentioning a ticket is not enough.

Answer:"""

def run_A() -> list[dict]:
    import mlx.core as mx
    from mlx_lm import load, generate
    from mlx_lm.sample_utils import make_sampler, make_logits_processors

    print(f"\n{'='*W}")
    print("APPROACH A: Qwen3.5-2B THINK MODE")
    print(f"{'='*W}")
    model, tok = load("mlx-community/Qwen3.5-2B-OptiQ-4bit")
    print(f"Active mem: {mx.get_active_memory()/1e9:.2f} GB", flush=True)
    sampler = make_sampler(temp=1.0, top_p=0.95, top_k=20)
    lp = make_logits_processors(repetition_penalty=1.1, repetition_context_size=64, presence_penalty=1.5)

    rows = []
    for r in DATA:
        daily = r["daily_plan"]
        tasks_str = "\n".join(f"- {k}: {TICKETS.get(k,{}).get('title','?')}" for k in daily)
        prompt = PROMPT_A.format(report=r["report"][:2500], tasks=tasks_str, example=daily[0])
        msgs = [{"role": "user", "content": prompt}]
        ids  = tok.apply_chat_template(msgs, add_generation_prompt=True, enable_thinking=True)
        t0   = time.monotonic()
        raw  = generate(model, tok, prompt=ids, max_tokens=4096, sampler=sampler,
                        logits_processors=lp, verbose=False)
        elapsed = time.monotonic() - t0
        think_chars = 0
        answer = raw
        if "</think>" in raw:
            tp, answer = raw.split("</think>", 1)
            think_chars = len(tp)
            answer = answer.strip()
        mx.clear_cache()

        pred  = parse_key(answer, set(daily))
        ok    = score_pred(pred, r["label"])
        rows.append({"hour": r["hour"], "label": r["label"], "pred": pred,
                     "ok": ok, "think": think_chars, "t": round(elapsed,1), "raw": answer[:80]})
        print(f"  {'✓' if ok else '✗'} {r['hour']}  label={r['label']:12}  pred={str(pred):12}  "
              f"think={think_chars:5}c  {elapsed:.1f}s", flush=True)

    del model, tok; gc.collect(); mx.clear_cache()
    return rows


# ─────────────────────────────────────────────────────────────────────────────
# APPROACH B — Reranker 0.6B only
# ─────────────────────────────────────────────────────────────────────────────
INSTR   = ("Given a developer worklog (the Query), judge whether the work described advances "
           "the goal of the project-management ticket (the Document). Answer yes only if "
           "completing this work would make progress on that specific ticket.")
PREFIX  = ("<|im_start|>system\nJudge whether the Document meets the requirements based on "
           "the Query and the Instruct provided. Note that the answer can only be \"yes\" or "
           "\"no\".<|im_end|>\n<|im_start|>user\n")
SUFFIX  = "<|im_end|>\n<|im_start|>assistant\n<think>\n\n</think>\n\n"
THR     = 0.10

def run_B() -> list[dict]:
    import mlx.core as mx
    import mlx.nn as nn
    from mlx_lm import load

    print(f"\n{'='*W}")
    print(f"APPROACH B: Reranker 0.6B only  (THR={THR})")
    print(f"{'='*W}")
    model, tok = load("kerncore/Qwen3-Reranker-0.6B-MLX-4bit")
    print(f"Active mem: {mx.get_active_memory()/1e9:.2f} GB", flush=True)
    yes_id = tok.encode("yes", add_special_tokens=False)[0]
    no_id  = tok.encode("no",  add_special_tokens=False)[0]

    def rscore(q: str, doc: str) -> float:
        ids = tok.encode(f"{PREFIX}<Instruct>: {INSTR}\n<Query>: {q}\n<Document>: {doc}{SUFFIX}",
                         add_special_tokens=False)
        lg = model(mx.array([ids]))[0, -1, :]
        p  = nn.softmax(mx.array([lg[no_id].item(), lg[yes_id].item()]))[1].item()
        mx.clear_cache()
        return p

    rows = []
    for r in DATA:
        daily = r["daily_plan"]
        query = r["report"][:1800]
        t0    = time.monotonic()
        scored = sorted(((k, rscore(query, ticket_doc(k))) for k in daily), key=lambda x: -x[1])
        elapsed = time.monotonic() - t0
        best_k, best_sc = scored[0]
        pred = best_k if best_sc >= THR else None
        ok   = score_pred(pred, r["label"])
        rows.append({"hour": r["hour"], "label": r["label"], "pred": pred,
                     "ok": ok, "top_sc": round(best_sc,3), "t": round(elapsed,1)})
        scores_str = "  ".join(f"{k}={s:.3f}" for k,s in scored)
        print(f"  {'✓' if ok else '✗'} {r['hour']}  label={r['label']:12}  pred={str(pred):12}  "
              f"scores=[{scores_str}]  {elapsed:.1f}s", flush=True)

    del model, tok; gc.collect(); mx.clear_cache()
    return rows


# ─────────────────────────────────────────────────────────────────────────────
# APPROACH C — Reranker shortlist → 2B-think confirm
# ─────────────────────────────────────────────────────────────────────────────
PROMPT_C = """A developer wrote this worklog for the last hour:

{report}

A pre-filter identified these candidate tasks (with relevance scores):
{cands}

Does this worklog represent ACTIVE PROGRESS on one of these tasks?
- Admin work (creating tickets, commits, code review of others' work) does NOT count as progress.
- Research/exploration DOES count if it's aimed at solving the task goal.
- Answer ONLY with the task key (e.g. {example}) or NONE.

Answer:"""

def run_C() -> list[dict]:
    import mlx.core as mx
    import mlx.nn as nn
    from mlx_lm import load, generate
    from mlx_lm.sample_utils import make_sampler, make_logits_processors

    print(f"\n{'='*W}")
    print("APPROACH C: Reranker shortlist → 2B-think confirm")
    print(f"{'='*W}")

    # Step 1: reranker scores all daily tasks
    print("Step 1: Load reranker ...", flush=True)
    rmodel, rtok = load("kerncore/Qwen3-Reranker-0.6B-MLX-4bit")
    print(f"  Active mem: {mx.get_active_memory()/1e9:.2f} GB", flush=True)
    yes_id = rtok.encode("yes", add_special_tokens=False)[0]
    no_id  = rtok.encode("no",  add_special_tokens=False)[0]

    def rscore(q: str, doc: str) -> float:
        ids = rtok.encode(f"{PREFIX}<Instruct>: {INSTR}\n<Query>: {q}\n<Document>: {doc}{SUFFIX}",
                          add_special_tokens=False)
        lg = rmodel(mx.array([ids]))[0, -1, :]
        p  = nn.softmax(mx.array([lg[no_id].item(), lg[yes_id].item()]))[1].item()
        mx.clear_cache()
        return p

    preranked = []
    for r in DATA:
        daily  = r["daily_plan"]
        query  = r["report"][:1800]
        scored = sorted(((k, rscore(query, ticket_doc(k))) for k in daily), key=lambda x: -x[1])
        # shortlist: top-2 with score > 0.03 (pass anything plausible to LLM)
        shortlist = [(k,s) for k,s in scored if s > 0.03][:2]
        preranked.append(shortlist)
        print(f"  {r['hour']}  shortlist={[(k,round(s,3)) for k,s in shortlist]}", flush=True)

    del rmodel, rtok; gc.collect(); mx.clear_cache()
    print(f"  Reranker unloaded. Mem: {mx.get_active_memory()/1e9:.2f} GB")

    # Step 2: LLM reasons over shortlist
    print("\nStep 2: Load 2B-think ...", flush=True)
    lmodel, ltok = load("mlx-community/Qwen3.5-2B-OptiQ-4bit")
    print(f"  Active mem: {mx.get_active_memory()/1e9:.2f} GB", flush=True)
    sampler = make_sampler(temp=1.0, top_p=0.95, top_k=20)
    lp = make_logits_processors(repetition_penalty=1.1, repetition_context_size=64, presence_penalty=1.5)

    rows = []
    for r, shortlist in zip(DATA, preranked):
        if not shortlist:
            # nothing passed reranker gate → NONE without LLM call
            pred, think_chars, elapsed = None, 0, 0.0
            print(f"  skip LLM (empty shortlist)  {r['hour']}", flush=True)
        else:
            keys  = [k for k,_ in shortlist]
            cands = "\n".join(f"- {k} (score {s:.3f}): {TICKETS.get(k,{}).get('title','?')}"
                              for k,s in shortlist)
            prompt = PROMPT_C.format(report=r["report"][:2500], cands=cands, example=keys[0])
            msgs   = [{"role": "user", "content": prompt}]
            ids    = ltok.apply_chat_template(msgs, add_generation_prompt=True, enable_thinking=True)
            t0     = time.monotonic()
            raw    = generate(lmodel, ltok, prompt=ids, max_tokens=4096, sampler=sampler,
                              logits_processors=lp, verbose=False)
            elapsed = time.monotonic() - t0
            think_chars = 0
            answer = raw
            if "</think>" in raw:
                tp, answer = raw.split("</think>", 1)
                think_chars = len(tp)
                answer = answer.strip()
            mx.clear_cache()
            pred = parse_key(answer, set(keys))

        ok = score_pred(pred, r["label"])
        rows.append({"hour": r["hour"], "label": r["label"], "pred": pred,
                     "ok": ok, "shortlist": shortlist, "think": think_chars, "t": round(elapsed,1)})
        print(f"  {'✓' if ok else '✗'} {r['hour']}  label={r['label']:12}  pred={str(pred):12}  "
              f"think={think_chars:5}c  {elapsed:.1f}s", flush=True)

    del lmodel, ltok; gc.collect(); mx.clear_cache()
    return rows


# ─────────────────────────────────────────────────────────────────────────────
# Summary table
# ─────────────────────────────────────────────────────────────────────────────
def summarise(rows_a, rows_b, rows_c):
    print(f"\n\n{'='*W}")
    print("RESULTS SUMMARY")
    print(f"{'='*W}")
    print(f"  {'Hour':16}  {'Label':12}  {'A-think':12}  {'B-rerank':12}  {'C-combo':12}")
    print("  " + "-"*66)
    for a, b, c in zip(rows_a, rows_b, rows_c):
        def fmt(r):
            p = str(r["pred"]) if r["pred"] else "NONE"
            return f"{'✓' if r['ok'] else '✗'}{p}"
        print(f"  {a['hour']:16}  {a['label']:12}  {fmt(a):12}  {fmt(b):12}  {fmt(c):12}")

    print(f"\n  {'Metric':20}  {'A-think':>8}  {'B-rerank':>8}  {'C-combo':>8}")
    print("  " + "-"*48)
    n = len(rows_a)
    for name, rows in [("Accuracy", rows_a), ("", rows_b), ("", rows_c)]:
        pass

    for label, rows in [("A-think", rows_a), ("B-rerank", rows_b), ("C-combo", rows_c)]:
        correct     = sum(r["ok"] for r in rows)
        task_rows   = [r for r in rows if r["label"] != "untracked"]
        untr_rows   = [r for r in rows if r["label"] == "untracked"]
        task_ok     = sum(r["ok"] for r in task_rows)
        untr_ok     = sum(r["ok"] for r in untr_rows)
        false_binds = sum(1 for r in untr_rows if r["pred"] is not None)
        recall_miss = sum(1 for r in task_rows if r["pred"] is None)
        avg_t       = sum(r["t"] for r in rows) / n
        print(f"  {label:20}  {correct}/{n} = {correct/n:.0%}  "
              f"task={task_ok}/{len(task_rows)}  untracked={untr_ok}/{len(untr_rows)}  "
              f"false-binds={false_binds}  recall-miss={recall_miss}  avg={avg_t:.1f}s")


if __name__ == "__main__":
    rows_a = run_A()
    rows_b = run_B()
    rows_c = run_C()
    summarise(rows_a, rows_b, rows_c)

    out = ROOT / "matcher_benchmark_results.json"
    out.write_text(json.dumps({"A": rows_a, "B": rows_b, "C": rows_c}, indent=2))
    print(f"\nResults → {out.name}")
