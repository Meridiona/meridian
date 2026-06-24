"""Multi-model comparison for the activity reporter pipeline.

Runs distil_hour() / distil_range() on the target window, then runs
free-form inference through each MLX model one at a time — loading,
generating, then fully unloading before the next. Prints the distilled
body and each model's report for comparison.

Usage
-----
  python eval_model_comparison.py [HOUR]               # default: last active hour
  python eval_model_comparison.py 2026-06-23T11
  python eval_model_comparison.py --range START END    # arbitrary range
  python eval_model_comparison.py --dump-body-only     # print body and exit
"""
from __future__ import annotations

import argparse
import gc
import os
import sqlite3
import sys
import time
from pathlib import Path

_REPO_SERVICES = Path(__file__).parents[3]
sys.path.insert(0, str(_REPO_SERVICES))

from agents.session_distiller import distil_hour, distil_range

_DB = os.path.expanduser("~/.meridian/meridian.db")

_W   = 72
_SEP = "═" * _W
_DIV = "─" * _W

# Exact doc-recommended settings (Qwen/Qwen3.5-2B model card):
#   non-thinking: temp=1.0, top_p=1.00, top_k=20
#   thinking:     temp=1.0, top_p=0.95, top_k=20
#
# (label, model_id, mode, temp, top_p, top_k)
# mode: "free"  = non-thinking, apply_chat_template default
#        "think" = thinking weights, enable_thinking=True, max_tokens=32768
_MODELS = [
    ("Qwen3.5-2B-nothink", "mlx-community/Qwen3.5-2B-OptiQ-4bit", "free",  1.0, 1.0,  20),
    ("Qwen3.5-2B-think",   "mlx-community/Qwen3.5-2B-OptiQ-4bit", "think", 1.0, 0.95, 20),
]

_MAX_TOKENS_FREE  = 4000   # non-thinking: answer only
_MAX_TOKENS_THINK = 32768  # thinking: must fit <think>…</think> + answer

_SYSTEM = """\
You are writing a hourly worklog update for a software development team. \
Your audience is the entire team — engineers, designers, product managers, and \
stakeholders. Most readers are not familiar with the codebase internals.

You receive a compressed log of what a developer did during a session \
(captured from their screen: editor, terminal, browser, and other tools).

TASK
Write a detailed, human-readable account of the developer's session. \
Cover everything that happened — building features, fixing bugs, researching topics, \
reading documentation, watching talks or tutorials, having design discussions, \
investigating issues, running experiments, reviewing code, and any other activity. \
Nothing should be left out just because it seems minor.

Focus on WHAT was accomplished or explored and WHY it matters. \
Do not focus on internal implementation details like variable names, function \
signatures, or file paths — those are noise for most readers. Write as if you are \
explaining to a smart colleague who was not in the room.

OUTPUT FORMAT
Write the following sections in order. Skip a section only if there is truly nothing \
to report for it — do not write placeholder text.

## Session Summary
2–4 sentences summarising the whole session in plain English. What was the developer \
focused on? What got done? What was the mood of the session (exploration, deep build, \
debugging, review)?

## What Was Worked On
One paragraph per distinct activity thread. A thread is any continuous block of \
related work, regardless of which tool was used.
For each thread:
  - Start with what the developer was trying to achieve (the goal, not the method).
  - Describe what happened — progress made, problems hit, things learned.
  - End with the current status: completed / in progress / blocked.
  - If the work directly benefits users or the team, say so in plain terms.
Include ALL activity types: coding, researching, reading docs, watching videos, \
testing, debugging, reviewing, discussing, planning, experimenting.

## Research & Learning
Anything the developer looked up, read, or watched to understand something better:
  - What question or problem triggered the research?
  - What sources were consulted (docs, articles, videos, colleagues)?
  - What was concluded or learned?

## Decisions Made
Any meaningful choice that shapes how the product or codebase will work:
  - What was the decision?
  - What alternatives were considered?
  - Why was this direction chosen?

## Tickets & Tasks
Only include this section if specific ticket keys (KAN-NNN, JIRA-NNN, etc.) or \
named tasks appear in the input. For each:
  - Plain-English goal of the ticket
  - What progress was made this session
  - What still remains

## Time Summary
One line per activity thread:
  HH:MM–HH:MM  Plain-English description                            N min

HARD RULES
- The total minutes across all threads MUST equal the "N min active" in the input \
header exactly. Do not invent or drop time.
- Write for someone who does not know the codebase. No jargon, no variable names, \
no file paths as headlines.
- Do not make up facts not present in the input.
- Do not truncate. If there is more to cover, cover it.\
"""


def _last_hour() -> str:
    con = sqlite3.connect(_DB)
    row = con.execute(
        "SELECT substr(started_at,1,13) FROM app_sessions "
        "WHERE session_text IS NOT NULL AND LENGTH(session_text)>40 "
        "ORDER BY started_at DESC LIMIT 1"
    ).fetchone()
    con.close()
    if not row:
        print("No sessions found in DB.", file=sys.stderr)
        sys.exit(1)
    return row[0]


def _snapshot_resources(label: str) -> dict:
    import psutil, mlx.core as mx
    proc = psutil.Process()
    mem  = proc.memory_info()
    return {
        "label":         label,
        "ram_rss_mb":    round(mem.rss / 1024**2, 1),
        "cpu_pct":       psutil.cpu_percent(interval=0.2),
        "metal_mb":      round(mx.get_active_memory() / 1024**2, 1),
        "metal_peak_mb": round(mx.get_peak_memory() / 1024**2, 1),
    }


def _print_resources(snap: dict) -> None:
    print(
        f"    RAM={snap['ram_rss_mb']}MB  CPU={snap['cpu_pct']}%  "
        f"Metal={snap['metal_mb']}MB (peak {snap['metal_peak_mb']}MB)",
        flush=True,
    )


def _run_model(
    name: str,
    model_id: str,
    body: str,
    mode: str = "free",
    temp: float = 1.0,
    top_p: float = 0.0,
    top_k: int = 0,
) -> tuple[str | None, float, dict]:
    """Returns (report_text, elapsed_s, resource_stats)."""
    print(f"\n  Loading {name} [mode={mode}] (temp={temp} top_p={top_p} top_k={top_k}) …", flush=True)
    t0 = time.monotonic()
    res_before = _snapshot_resources("before_load")

    try:
        import mlx.core as mx
        from mlx_lm import load, generate
        from mlx_lm.sample_utils import make_sampler

        model, tokenizer = load(model_id)
        messages = [
            {"role": "system", "content": _SYSTEM},
            {"role": "user",   "content": body},
        ]
        sampler = make_sampler(temp=temp, top_p=top_p, top_k=top_k)
        from mlx_lm.sample_utils import make_logits_processors
        logits_processors = make_logits_processors(
            repetition_penalty=1.1,
            repetition_context_size=64,
            presence_penalty=1.5,
        )
        t_load = round(time.monotonic() - t0, 1)

        res_loaded = _snapshot_resources("after_load")
        print(f"  Loaded in {t_load}s", flush=True)
        _print_resources(res_loaded)
        print("  Generating …", flush=True)

        enable_thinking = (mode == "think")
        max_tok = _MAX_TOKENS_THINK if mode == "think" else _MAX_TOKENS_FREE
        prompt = tokenizer.apply_chat_template(
            messages,
            add_generation_prompt=True,
            enable_thinking=enable_thinking,
        )
        raw = generate(model, tokenizer, prompt=prompt,
                       max_tokens=max_tok, sampler=sampler,
                       logits_processors=logits_processors, verbose=False)

        if mode == "think" and "</think>" in raw:
            think_part, answer_part = raw.split("</think>", 1)
            print(f"  <think> block: {len(think_part)} chars → stripped", flush=True)
            raw = answer_part.strip()

        t_total = round(time.monotonic() - t0, 1)
        res_after = _snapshot_resources("after_gen")
        print(f"  Done in {t_total}s", flush=True)
        _print_resources(res_after)

        resource_stats = {
            "load_metal_delta_mb": round(res_loaded["metal_mb"] - res_before["metal_mb"], 1),
            "gen_metal_peak_mb":   res_after["metal_peak_mb"],
            "after_load":          res_loaded,
            "after_gen":           res_after,
        }
        return raw.strip(), t_total, resource_stats

    except Exception as exc:  # noqa: BLE001
        print(f"  ERROR: {exc}", flush=True)
        return None, round(time.monotonic() - t0, 1), {}
    finally:
        try:
            del model, tokenizer
        except NameError:
            pass
        gc.collect()
        try:
            import mlx.core as mx
            mx.clear_cache()
        except Exception:
            pass
        res_unload = _snapshot_resources("after_unload")
        print("  Unloaded.", flush=True)
        _print_resources(res_unload)


def main() -> None:
    p = argparse.ArgumentParser(description=__doc__,
                                formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("hour", nargs="?", help="YYYY-MM-DDTHH (default: last active hour)")
    p.add_argument("--range", nargs=2, metavar=("START", "END"))
    p.add_argument("--dump-body-only", action="store_true")
    p.add_argument("--models", nargs="+", metavar="NAME",
                   help=f"subset of models to run (available: {[e[0] for e in _MODELS]})")
    args = p.parse_args()

    if args.range:
        start, end = args.range
        label = f"{start}..{end}"
        print(f"\n{_SEP}\nRANGE: {label}\n{_SEP}")
        print("\nRunning distiller …", flush=True)
        t0 = time.monotonic()
        body, stats = distil_range(start, end)
    else:
        hour = args.hour or _last_hour()
        label = hour
        print(f"\n{_SEP}\nHOUR: {hour}\n{_SEP}")
        print("\nRunning distiller …", flush=True)
        t0 = time.monotonic()
        body, stats = distil_hour(hour)

    t_distil = round(time.monotonic() - t0, 1)

    if not body:
        print("No sessions found.")
        sys.exit(0)

    print(
        f"  {stats.nsess} sessions · {stats.raw_chars//1000}k→{stats.out_chars//1000}k chars "
        f"({stats.reduction_pct:.1f}%) in {t_distil}s"
    )
    print(f"\n{_DIV}\nDISTILLED BODY (input to all models):\n{_DIV}")
    print(body)

    if args.dump_body_only:
        return

    models_to_run = _MODELS
    if args.models:
        names = set(args.models)
        models_to_run = [e for e in _MODELS if e[0] in names]
        if not models_to_run:
            print(f"No matching models. Available: {[e[0] for e in _MODELS]}")
            sys.exit(1)

    results = []
    for name, model_id, mode, temp, top_p, top_k in models_to_run:
        print(f"\n{_SEP}\nMODEL: {name}  ({model_id})\n{_SEP}")
        report, elapsed, res = _run_model(name, model_id, body,
                                          mode=mode, temp=temp,
                                          top_p=top_p, top_k=top_k)
        results.append((name, mode, report, elapsed, res))
        if report:
            print("\n  REPORT:\n")
            for line in report.splitlines():
                print(f"    {line}")

    print(f"\n\n{_SEP}\nCOMPARISON SUMMARY — {label}\n{_SEP}")
    print(f"  {'Model':24}  {'Mode':7}  {'Time':>6}  {'MetalΔ':>8}  {'Peak':>8}  Status")
    print("  " + _DIV)
    for name, mode, report, elapsed, res in results:
        metal_d = f"{res.get('load_metal_delta_mb', 0):+.0f}MB" if res else "—"
        peak    = f"{res.get('gen_metal_peak_mb', 0):.0f}MB" if res else "—"
        status  = f"OK · {len(report)} chars" if report else "FAILED"
        print(f"  {name:24}  {mode:7}  {elapsed:>5.1f}s  {metal_d:>8}  {peak:>8}  {status}")
    print(f"\n  Distilled body: {stats.out_chars} chars  ({stats.reduction_pct:.1f}% reduction)")


if __name__ == "__main__":
    main()
