"""Qwen3.5-2B (THINK mode) generative task-classifier on the week dataset.

The companion to eval_reranker_classify.py, but the matcher is the generative
Qwen3.5-2B-OptiQ-4bit (the SAME model the activity reporter uses) run with
enable_thinking=True — not a reranker. The model reads the session summary +
candidate ticket docs and directly picks ONE candidate key or NONE (its own
abstain). Identical inputs to the reranker via plans.load_week(), identical
scoring via plans.report-style accuracy/false-bind/recall-miss.

Prior gen-classifier tests (eval_gen_classify.py) ran Ollama models with
think=False; this is the first eval of the 2B in THINK mode, the configuration
the user wants productionised.

Loads exactly one model and exits — never resident alongside the reranker.

Usage:
  services/.venv/bin/python services/tests/evals/rerank/eval_llm_think_week.py
  services/.venv/bin/python services/tests/evals/rerank/eval_llm_think_week.py --multi   # allow multi-bind
  services/.venv/bin/python services/tests/evals/rerank/eval_llm_think_week.py --no-think
"""
from __future__ import annotations
import argparse, json, re, sys, time
from pathlib import Path

import mlx.core as mx
from mlx_lm import load, generate
from mlx_lm.sample_utils import make_sampler, make_logits_processors

ROOT = Path(__file__).parent
sys.path.insert(0, str(ROOT))
from plans import load_week, report  # noqa: E402

MODEL_ID = "mlx-community/Qwen3.5-2B-OptiQ-4bit"
KEY_RE   = re.compile(r"\b[A-Z]+-\d{2,4}\b")

# ── CLI ─────────────────────────────────────────────────────────────────────────
ap = argparse.ArgumentParser()
ap.add_argument("--model",   default=MODEL_ID)
ap.add_argument("--multi",   action="store_true", help="allow the model to bind multiple tickets")
ap.add_argument("--no-think", action="store_true", help="disable thinking mode")
ap.add_argument("--max-tokens", type=int, default=8192)
ap.add_argument("--limit",   type=int, default=0, help="only first N sessions (debug)")
args = ap.parse_args()
THINK = not args.no_think

# ── Prompt ────────────────────────────────────────────────────────────────────
_SINGLE = """A developer completed one work session. Decide which ONE project-management ticket (if any) this session advanced.

Session summary:
{summary}

Candidate tickets:
{cands}

Rules:
- Pick the single ticket whose goal this session most directly advanced.
- If the work does not clearly advance ANY candidate (unrelated work, admin/overhead, or real work with no matching ticket), answer NONE.
- A ticket merely being visible on screen is NOT enough — the session must actually progress its goal.
- Answer with ONLY the ticket key (e.g. {example}) or NONE.

Answer:"""

_MULTI = """A developer completed one work session. Decide which project-management ticket(s) (if any) this session advanced.

Session summary:
{summary}

Candidate tickets:
{cands}

Rules:
- List every ticket whose goal this session directly advanced — it may be one, several, or none.
- If the work does not clearly advance ANY candidate (unrelated work, admin/overhead, or real work with no matching ticket), answer NONE.
- A ticket merely being visible on screen is NOT enough — the session must actually progress its goal.
- Answer with ONLY the ticket key(s), comma-separated (e.g. {example}), or NONE.

Answer:"""


def build_prompt(summary: str, cand_keys: list[str], docs: dict) -> str:
    cand_str = "\n".join(f"- {k}: {docs.get(k,'')[:300]}" for k in cand_keys)
    tmpl = _MULTI if args.multi else _SINGLE
    return tmpl.format(summary=summary[:1800], cands=cand_str, example=cand_keys[0])


def parse_answer(text: str, cand_set: set) -> list[str]:
    """Return the list of candidate keys the model committed to (order-preserving)."""
    found = [k for k in KEY_RE.findall(text) if k in cand_set]
    seen, out = set(), []
    for k in found:
        if k not in seen:
            seen.add(k); out.append(k)
    if out:
        return out if args.multi else out[:1]
    return []  # NONE


def main() -> None:
    plans, docs = load_week()
    if args.limit:
        plans = plans[: args.limit]
    print(f"WEEK dataset: {len(plans)} sessions  |  model={args.model}  think={THINK}  multi={args.multi}")
    print(f"(reranker bench: Qwen3-Reranker-0.6B = 94%, 0 false-binds, 1.5GB)\n", flush=True)

    print(f"Loading {args.model} ...", flush=True)
    model, tok = load(args.model)
    print(f"Active mem: {mx.get_active_memory()/1e9:.2f} GB", flush=True)

    sampler = make_sampler(temp=1.0, top_p=0.95, top_k=20)
    logits_processors = make_logits_processors(
        repetition_penalty=1.1, repetition_context_size=64, presence_penalty=1.5,
    )

    ranked_all = []   # (plan, [(key, score)...]) — score 1.0 for picked, to reuse report()
    preds_raw  = []   # (plan, [keys], think_chars, gen_s)
    t0 = time.time()
    for i, p in enumerate(plans):
        prompt_str = build_prompt(p["summary"], p["candidates"], docs)
        messages = [{"role": "user", "content": prompt_str}]
        ids = tok.apply_chat_template(messages, add_generation_prompt=True, enable_thinking=THINK)
        tg = time.time()
        raw = generate(model, tok, prompt=ids, max_tokens=args.max_tokens,
                       sampler=sampler, logits_processors=logits_processors, verbose=False)
        gen_s = time.time() - tg
        think_chars = 0
        if "</think>" in raw:
            tpart, raw = raw.split("</think>", 1)
            think_chars = len(tpart)
        answer = raw.strip()
        keys = parse_answer(answer, set(p["candidates"]))
        preds_raw.append((p, keys, think_chars, gen_s))

        # For report(): emit picked key with score 1.0, else a NONE-forcing low score
        if keys:
            ranked = [(keys[0], 1.0)] + [(k, 0.0) for k in p["candidates"] if k != keys[0]]
        else:
            ranked = [(p["candidates"][0], 0.0)]  # top score 0 < any THR → NONE
        ranked_all.append((p, ranked))

        mx.clear_cache()
        pred_str = ",".join(keys) if keys else "NONE"
        ok = (set(keys) & p["acceptable"]) or (not keys and p["truth"] == "NONE")
        print(f"[{i+1:3d}/{len(plans)}] {'✓' if ok else '✗'} truth={p['truth']:9} pred={pred_str:18} "
              f"think={think_chars:5}c gen={gen_s:4.1f}s  {p['note'][:40]}", flush=True)

    dt = time.time() - t0
    # report() uses THR gate; our scores are 1.0/0.0 so any THR in (0,1) splits cleanly.
    report(f"Qwen3.5-2B think={THINK} multi={args.multi}", ranked_all, thrs=(0.5,))

    avg_think = sum(r[2] for r in preds_raw) / len(preds_raw)
    avg_gen   = sum(r[3] for r in preds_raw) / len(preds_raw)
    n_multi   = sum(1 for r in preds_raw if len(r[1]) > 1)
    print(f"\n  total {dt:.0f}s  avg gen {avg_gen:.1f}s/session  avg think {avg_think:.0f} chars  "
          f"multi-binds={n_multi}  peak mem {mx.get_peak_memory()/1e9:.2f} GB")

    out = ROOT / "results" / f"llm_think_week_{'multi' if args.multi else 'single'}_{'think' if THINK else 'nothink'}.json"
    out.write_text(json.dumps({
        "model": args.model, "think": THINK, "multi": args.multi, "n": len(plans),
        "elapsed_s": round(dt, 1), "avg_gen_s": round(avg_gen, 2),
        "per_session": [{"id": p["id"], "truth": p["truth"], "pred": keys,
                         "acceptable": sorted(p["acceptable"]), "stype": p["session_type"],
                         "think_chars": tc, "gen_s": round(gs, 2)}
                        for p, keys, tc, gs in preds_raw],
    }, indent=2))
    print(f"  results → {out.name}")


if __name__ == "__main__":
    main()
