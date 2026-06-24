"""Standalone experiment: match an activity report to PM tasks.

Three approaches tested side-by-side on real data:
  A) Qwen3.5-2B think — read the report, pick a task or NONE (no reranker)
  B) Reranker 0.6B — score all tasks, take argmax if score >= threshold
  C) Tiered: reranker shortlists top-3 → 2B-think confirms/abstains

Tier logic tested:
  Tier 1: match against DAILY tasks (2-5 confirmed tasks for today)
  Tier 2: if no match, match against ALL tasks (~25)
  Tier 3: if still no match → output "CREATE NEW TASK"

One model loaded at a time — never two in memory simultaneously.
Run: services/.venv/bin/python services/tests/evals/cluster/test_task_matcher.py
"""
from __future__ import annotations
import json, re, sys, time
from pathlib import Path

ROOT     = Path(__file__).parent
SERVICES = ROOT.parents[2]
sys.path.insert(0, str(SERVICES))

TASKS_FILE   = SERVICES / "tests/evals/rerank/data/tickets.json"
REPORT_FILE  = ROOT / "2b_think_report.txt"

# ── Load task pool ─────────────────────────────────────────────────────────────
raw_tickets = json.loads(TASKS_FILE.read_text())
if isinstance(raw_tickets, list):
    TICKETS = {t["task_key"]: t for t in raw_tickets}
else:
    TICKETS = raw_tickets

def ticket_doc(key: str) -> str:
    t = TICKETS[key]
    desc  = (t.get("description_text") or "").strip().replace("\n", " ")[:500]
    epic  = t.get("epic_title") or ""
    itype = t.get("issue_type", "Task")
    return f"[{itype}] {t['title']}. Epic: {epic}. {desc}".strip()

ALL_KEYS = list(TICKETS.keys())

# Simulate a daily plan (2-5 tasks the user confirmed today).
# In production this comes from the /plan endpoint.
DAILY_PLAN = ["KAN-64", "KAN-231", "KAN-200", "KAN-239", "KAN-241"]

# The activity report text to classify
REPORT_TEXT = REPORT_FILE.read_text()
# Strip trailing eval noise — real output is up to the === line
if "\n═" in REPORT_TEXT:
    REPORT_TEXT = REPORT_TEXT[:REPORT_TEXT.index("\n═")].strip()

print(f"Report: {len(REPORT_TEXT)} chars")
print(f"Daily plan: {DAILY_PLAN}")
print(f"All tasks:  {len(ALL_KEYS)}")


# ─────────────────────────────────────────────────────────────────────────────
# APPROACH A: Qwen3.5-2B think-mode generative classifier
# ─────────────────────────────────────────────────────────────────────────────
PROMPT_DAILY = """A developer wrote this worklog for the last hour of work:

{report}

Today's confirmed tasks:
{tasks}

Which ONE of today's tasks did this work most directly advance?
- If it clearly advances one of today's tasks, answer with ONLY the task key (e.g. KAN-64).
- If the work is admin/overhead or doesn't match any task above, answer NONE.

Answer:"""

PROMPT_ALL = """A developer wrote this worklog for the last hour of work:

{report}

All open project tasks:
{tasks}

Which ONE task did this work most directly advance? If it doesn't match any task, answer NONE.
Answer with ONLY the task key (e.g. KAN-64) or NONE.

Answer:"""

KEY_RE = re.compile(r"\b[A-Z]+-\d{2,4}\b")

def _format_tasks(keys: list[str]) -> str:
    return "\n".join(f"- {k}: {TICKETS[k]['title']}" for k in keys if k in TICKETS)

def _parse_key(text: str, valid: set) -> str | None:
    for k in KEY_RE.findall(text):
        if k in valid:
            return k
    return None


def run_approach_a_llm_think() -> None:
    """Qwen3.5-2B think mode — tiered daily → all → create-new."""
    import mlx.core as mx
    from mlx_lm import load, generate
    from mlx_lm.sample_utils import make_sampler, make_logits_processors

    MODEL = "mlx-community/Qwen3.5-2B-OptiQ-4bit"
    print(f"\n{'='*70}")
    print(f"APPROACH A: Qwen3.5-2B THINK — tiered daily→all→create")
    print(f"{'='*70}")

    print(f"Loading {MODEL} ...", flush=True)
    model, tok = load(MODEL)
    print(f"Active mem: {mx.get_active_memory()/1e9:.2f} GB", flush=True)

    sampler   = make_sampler(temp=1.0, top_p=0.95, top_k=20)
    lp        = make_logits_processors(repetition_penalty=1.1,
                                       repetition_context_size=64, presence_penalty=1.5)

    def ask(prompt: str) -> tuple[str, int, float]:
        messages = [{"role": "user", "content": prompt}]
        ids = tok.apply_chat_template(messages, add_generation_prompt=True, enable_thinking=True)
        t0  = time.monotonic()
        raw = generate(model, tok, prompt=ids, max_tokens=4096,
                       sampler=sampler, logits_processors=lp, verbose=False)
        elapsed = time.monotonic() - t0
        think_chars = 0
        if "</think>" in raw:
            think_part, raw = raw.split("</think>", 1)
            think_chars = len(think_part)
            raw = raw.strip()
        mx.clear_cache()
        return raw.strip(), think_chars, elapsed

    # TIER 1: daily plan
    prompt1 = PROMPT_DAILY.format(
        report=REPORT_TEXT[:3000],
        tasks=_format_tasks(DAILY_PLAN),
    )
    print("\nTIER 1: matching against daily plan ...", flush=True)
    answer1, tc1, t1 = ask(prompt1)
    matched1 = _parse_key(answer1, set(DAILY_PLAN))
    print(f"  raw answer : {answer1[:120]}")
    print(f"  think chars: {tc1}   elapsed: {t1:.1f}s")
    print(f"  → matched  : {matched1 or 'NONE'}")

    if matched1:
        print(f"\n  RESULT: BOUND to daily task {matched1} ({TICKETS[matched1]['title']})")
        del model, tok; return

    # TIER 2: all tasks
    prompt2 = PROMPT_ALL.format(
        report=REPORT_TEXT[:3000],
        tasks=_format_tasks(ALL_KEYS),
    )
    print("\nTIER 2: no daily match — matching against all tasks ...", flush=True)
    answer2, tc2, t2 = ask(prompt2)
    matched2 = _parse_key(answer2, set(ALL_KEYS))
    print(f"  raw answer : {answer2[:120]}")
    print(f"  think chars: {tc2}   elapsed: {t2:.1f}s")
    print(f"  → matched  : {matched2 or 'NONE'}")

    if matched2:
        print(f"\n  RESULT: BOUND to existing task {matched2} ({TICKETS[matched2]['title']})")
    else:
        print(f"\n  RESULT: CREATE NEW TASK (no match in {len(ALL_KEYS)} tasks)")

    del model, tok


# ─────────────────────────────────────────────────────────────────────────────
# APPROACH B: Reranker 0.6B — score all candidates, pick argmax ≥ threshold
# ─────────────────────────────────────────────────────────────────────────────

def run_approach_b_reranker() -> None:
    """Reranker-only tiered approach — fast but no reasoning for admin cases."""
    import mlx.core as mx
    import mlx.nn as nn
    from mlx_lm import load

    MODEL = "kerncore/Qwen3-Reranker-0.6B-MLX-4bit"
    INSTR = ("Given a developer worklog (the Query), judge whether the work described "
             "advances the goal of the project-management ticket (the Document). "
             "Answer yes only if completing this work would make progress on that specific ticket.")
    PREFIX = ("<|im_start|>system\nJudge whether the Document meets the requirements based on "
              "the Query and the Instruct provided. Note that the answer can only be \"yes\" or "
              "\"no\".<|im_end|>\n<|im_start|>user\n")
    SUFFIX = "<|im_end|>\n<|im_start|>assistant\n<think>\n\n</think>\n\n"
    THR    = 0.10  # bind threshold (tuned on week dataset)

    print(f"\n{'='*70}")
    print(f"APPROACH B: Reranker 0.6B — tiered daily→all→create")
    print(f"{'='*70}")

    print(f"Loading {MODEL} ...", flush=True)
    model, tok = load(MODEL)
    print(f"Active mem: {mx.get_active_memory()/1e9:.2f} GB", flush=True)

    yes_id = tok.encode("yes", add_special_tokens=False)[0]
    no_id  = tok.encode("no",  add_special_tokens=False)[0]

    def score_one(query: str, doc: str) -> float:
        ids = tok.encode(
            f"{PREFIX}<Instruct>: {INSTR}\n<Query>: {query}\n<Document>: {doc}{SUFFIX}",
            add_special_tokens=False,
        )
        lg = model(mx.array([ids]))[0, -1, :]
        p  = nn.softmax(mx.array([lg[no_id].item(), lg[yes_id].item()]))[1].item()
        mx.clear_cache()
        return p

    query = REPORT_TEXT[:2000]

    # TIER 1: daily plan
    print(f"\nTIER 1: scoring {len(DAILY_PLAN)} daily-plan tasks ...", flush=True)
    t0 = time.monotonic()
    daily_scores = {k: score_one(query, ticket_doc(k)) for k in DAILY_PLAN}
    t1 = time.monotonic() - t0
    best1_key = max(daily_scores, key=daily_scores.get)
    best1_sc  = daily_scores[best1_key]
    print(f"  scores ({t1:.1f}s):")
    for k, s in sorted(daily_scores.items(), key=lambda x: -x[1]):
        print(f"    {k}: {s:.3f}  {TICKETS[k]['title'][:50]}")
    print(f"  → best: {best1_key} @ {best1_sc:.3f}  (THR={THR})")
    if best1_sc >= THR:
        print(f"\n  RESULT: BOUND to daily task {best1_key} ({TICKETS[best1_key]['title']})")
        del model, tok; return

    # TIER 2: all tasks
    print(f"\nTIER 2: no daily match — scoring all {len(ALL_KEYS)} tasks ...", flush=True)
    t0 = time.monotonic()
    all_scores = {k: score_one(query, ticket_doc(k)) for k in ALL_KEYS}
    t2 = time.monotonic() - t0
    ranked = sorted(all_scores.items(), key=lambda x: -x[1])
    print(f"  top 5 ({t2:.1f}s):")
    for k, s in ranked[:5]:
        print(f"    {k}: {s:.3f}  {TICKETS[k]['title'][:50]}")
    top_key, top_sc = ranked[0]
    if top_sc >= THR:
        print(f"\n  RESULT: BOUND to {top_key} ({TICKETS[top_key]['title']})")
    else:
        print(f"\n  RESULT: CREATE NEW TASK (top score {top_sc:.3f} < {THR})")

    del model, tok


# ─────────────────────────────────────────────────────────────────────────────
# APPROACH C: Reranker shortlists top-3 → 2B-think confirms/abstains
# ─────────────────────────────────────────────────────────────────────────────

def run_approach_c_combo() -> None:
    """Reranker shortlists → 2B-think reasons. Best of both worlds."""
    import gc
    import mlx.core as mx
    import mlx.nn as nn
    from mlx_lm import load, generate
    from mlx_lm.sample_utils import make_sampler, make_logits_processors

    RERANKER = "kerncore/Qwen3-Reranker-0.6B-MLX-4bit"
    LLM      = "mlx-community/Qwen3.5-2B-OptiQ-4bit"
    INSTR    = ("Given a developer worklog (the Query), judge whether the work described "
                "advances the goal of the project-management ticket (the Document). "
                "Answer yes only if completing this work would make progress on that specific ticket.")
    PREFIX   = ("<|im_start|>system\nJudge whether the Document meets the requirements based on "
                "the Query and the Instruct provided. Note that the answer can only be \"yes\" or "
                "\"no\".<|im_end|>\n<|im_start|>user\n")
    SUFFIX   = "<|im_end|>\n<|im_start|>assistant\n<think>\n\n</think>\n\n"
    SHORTLIST_N = 3  # reranker picks top-N for LLM to reason about
    SHORTLIST_MIN_SC = 0.05  # only hand to LLM if score > this

    print(f"\n{'='*70}")
    print(f"APPROACH C: Reranker shortlist → 2B-think confirm")
    print(f"{'='*70}")

    # Step 1: reranker (fast, cheap)
    print(f"\nStep 1: Load reranker {RERANKER} ...", flush=True)
    rmodel, rtok = load(RERANKER)
    print(f"  Active mem: {mx.get_active_memory()/1e9:.2f} GB", flush=True)

    yes_id = rtok.encode("yes", add_special_tokens=False)[0]
    no_id  = rtok.encode("no",  add_special_tokens=False)[0]

    def rscore(query: str, doc: str) -> float:
        ids = rtok.encode(
            f"{PREFIX}<Instruct>: {INSTR}\n<Query>: {query}\n<Document>: {doc}{SUFFIX}",
            add_special_tokens=False,
        )
        lg = rmodel(mx.array([ids]))[0, -1, :]
        p  = nn.softmax(mx.array([lg[no_id].item(), lg[yes_id].item()]))[1].item()
        mx.clear_cache()
        return p

    query = REPORT_TEXT[:2000]

    # TIER 1: score daily plan
    print(f"\n  TIER 1: scoring {len(DAILY_PLAN)} daily tasks ...", flush=True)
    t0 = time.monotonic()
    daily_scores = sorted(
        {k: rscore(query, ticket_doc(k)) for k in DAILY_PLAN}.items(), key=lambda x: -x[1]
    )
    print(f"  daily scores ({time.monotonic()-t0:.1f}s):")
    for k, s in daily_scores:
        print(f"    {k}: {s:.3f}  {TICKETS[k]['title'][:50]}")
    shortlist_daily = [(k, s) for k, s in daily_scores if s >= SHORTLIST_MIN_SC][:SHORTLIST_N]

    # TIER 2: if no daily candidates, score all
    if not shortlist_daily:
        print(f"\n  TIER 2: no daily candidates — scoring all {len(ALL_KEYS)} tasks ...", flush=True)
        t0 = time.monotonic()
        all_scores = sorted(
            {k: rscore(query, ticket_doc(k)) for k in ALL_KEYS}.items(), key=lambda x: -x[1]
        )
        print(f"  top-5 ({time.monotonic()-t0:.1f}s):")
        for k, s in all_scores[:5]:
            print(f"    {k}: {s:.3f}  {TICKETS[k]['title'][:50]}")
        shortlist = [(k, s) for k, s in all_scores if s >= SHORTLIST_MIN_SC][:SHORTLIST_N]
        shortlist_pool = ALL_KEYS
    else:
        shortlist = shortlist_daily
        shortlist_pool = DAILY_PLAN

    # Unload reranker before loading LLM
    print(f"\n  Unloading reranker ...", flush=True)
    del rmodel, rtok; gc.collect(); mx.clear_cache()
    print(f"  Mem after unload: {mx.get_active_memory()/1e9:.2f} GB", flush=True)

    if not shortlist:
        print(f"\n  RESULT: CREATE NEW TASK (reranker found no candidates above {SHORTLIST_MIN_SC})")
        return

    # Step 2: LLM reasoning on the shortlist
    shortlist_keys = [k for k, _ in shortlist]
    print(f"\nStep 2: Load LLM {LLM} to reason over {shortlist_keys} ...", flush=True)
    lmodel, ltok = load(LLM)
    print(f"  Active mem: {mx.get_active_memory()/1e9:.2f} GB", flush=True)

    sampler = make_sampler(temp=1.0, top_p=0.95, top_k=20)
    lp      = make_logits_processors(repetition_penalty=1.1,
                                     repetition_context_size=64, presence_penalty=1.5)

    cand_str = "\n".join(
        f"- {k} (reranker score {s:.2f}): {TICKETS[k]['title']}\n  {(TICKETS[k].get('description_text') or '')[:200]}"
        for k, s in shortlist
    )
    prompt = f"""A developer wrote this worklog for the last hour:

{REPORT_TEXT[:3000]}

The reranker suggests these candidate tasks:
{cand_str}

Does this worklog represent actual progress on one of these tasks?
- If YES, answer with ONLY the task key (e.g. {shortlist_keys[0]}).
- If the work is admin/overhead/unrelated (just mentioning a ticket is NOT enough — work must advance its goal), answer NONE.

Answer:"""

    messages = [{"role": "user", "content": prompt}]
    ids = ltok.apply_chat_template(messages, add_generation_prompt=True, enable_thinking=True)
    t0  = time.monotonic()
    raw = generate(lmodel, ltok, prompt=ids, max_tokens=4096,
                   sampler=sampler, logits_processors=lp, verbose=False)
    elapsed = time.monotonic() - t0
    think_chars = 0
    if "</think>" in raw:
        tpart, raw = raw.split("</think>", 1)
        think_chars = len(tpart)
        raw = raw.strip()
    mx.clear_cache()

    print(f"  raw answer : {raw[:120]}")
    print(f"  think chars: {think_chars}   elapsed: {elapsed:.1f}s")

    matched = _parse_key(raw, set(shortlist_keys))
    if matched:
        print(f"\n  RESULT: BOUND to {matched} ({TICKETS[matched]['title']})")
    else:
        if shortlist_pool is DAILY_PLAN:
            # Try all tasks before giving up
            print(f"\n  LLM rejected daily shortlist — would escalate to all-tasks reranker pass next")
            print(f"  (skipping in this test to avoid re-loading reranker — production would do it)")
        else:
            print(f"\n  RESULT: CREATE NEW TASK (LLM rejected all reranker candidates)")

    del lmodel, ltok


# ─────────────────────────────────────────────────────────────────────────────
# Main
# ─────────────────────────────────────────────────────────────────────────────
if __name__ == "__main__":
    import argparse
    p = argparse.ArgumentParser()
    p.add_argument("approach", nargs="?", default="all",
                   choices=["a", "b", "c", "all"],
                   help="a=LLM-think  b=reranker-only  c=reranker+LLM  all=run all sequentially")
    args = p.parse_args()

    runs = {"a": run_approach_a_llm_think,
            "b": run_approach_b_reranker,
            "c": run_approach_c_combo}

    if args.approach == "all":
        for fn in runs.values():
            fn()
    else:
        runs[args.approach]()
