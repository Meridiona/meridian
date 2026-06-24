"""Generate activity reports for labeled hours, then print for human review.

Phase 1 (this script): distil each hour → Qwen3.5-2B-think generates report → save to JSON.
Phase 2 (done by reading output): Claude labels each report as ticket or untracked.

Hours chosen to cover varied ground truth:
  - Clear single-ticket hours  (KAN-231, KAN-240, etc.)
  - Multi-ticket hours         (KAN-239+240)
  - Untracked hours            (Jun 19 tasks=empty)
  - Today's new ticket         (KAN-243)

Daily plan per hour = the actual working-set candidates for that day (3-5 tickets).
This mirrors the real production regime: user confirms 2-5 tasks each morning.
"""
from __future__ import annotations
import gc, json, sys, time
from pathlib import Path

ROOT     = Path(__file__).parent
SERVICES = ROOT.parents[2]
sys.path.insert(0, str(SERVICES))

from agents.session_distiller import distil_hour, evict_embedder   # noqa: E402

OUT = ROOT / "labeled_reports.json"

# ── Hours to generate + their daily plan (what the user had confirmed that day)
# Format: (hour, [daily_plan_tasks], expected_note)
# expected_note is just metadata — labeling happens AFTER reading the report.
HOURS = [
    # Jun 16 — KAN-239, KAN-240 active
    ("2026-06-16T03", ["KAN-239", "KAN-240", "KAN-231"],       "jun16 early"),
    ("2026-06-16T09", ["KAN-239", "KAN-240", "KAN-231"],       "jun16 mid"),
    # Jun 17 — KAN-241 heavy + KAN-239 overlap
    ("2026-06-17T07", ["KAN-241", "KAN-239", "KAN-231"],       "jun17 worklog OO"),
    # Jun 18 — KAN-231 classifier work + KAN-199
    ("2026-06-18T07", ["KAN-231", "KAN-199", "KAN-241"],       "jun18 classifier"),
    ("2026-06-18T11", ["KAN-231", "KAN-199", "KAN-241"],       "jun18 afternoon"),
    # Jun 19 — no task_key in DB (untracked candidate)
    ("2026-06-19T05", ["KAN-200", "KAN-199", "KAN-231"],       "jun19 possibly untracked"),
    # Jun 23 today — KAN-243 (benchmark) + KAN-200
    ("2026-06-23T09", ["KAN-243", "KAN-200", "KAN-239"],       "today morning"),
    ("2026-06-23T11", ["KAN-243", "KAN-200", "KAN-239"],       "today midday"),
]

TICKET_TITLES = {
    "KAN-199": "Establish baseline classifier accuracy on current Golden dataset",
    "KAN-200": "Untracked session analysis, PM matching, and draft approval flow",
    "KAN-230": "Clean candidate-set selection of PM tasks feeding the task classifier",
    "KAN-231": "Audit task-classification accuracy and improve it",
    "KAN-239": "Pass confirmed daily plan (today's tasks) to the session-task classifier",
    "KAN-240": "Emit session-task classifier logs to OpenObserve and set up proper debug tracing",
    "KAN-241": "Emit PM worklog logs to OpenObserve and set up proper debug tracing",
    "KAN-243": "Benchmark, baseline & ship the best model/reranker for session→task classification",
}

# ── Model setup ───────────────────────────────────────────────────────────────
MODEL_ID = "mlx-community/Qwen3.5-2B-OptiQ-4bit"

def generate_report(body: str) -> tuple[str, int, int, float]:
    """Returns (report_md, input_tokens, output_tokens, elapsed_s)."""
    import mlx.core as mx
    from mlx_lm import load, generate
    from mlx_lm.sample_utils import make_sampler, make_logits_processors
    from agents.prompts.activity_report import SYSTEM

    model, tok = load(MODEL_ID)
    try:
        sampler = make_sampler(temp=1.0, top_p=0.95, top_k=20)
        lp      = make_logits_processors(
            repetition_penalty=1.1, repetition_context_size=64, presence_penalty=1.5,
        )
        messages   = [{"role": "system", "content": SYSTEM},
                      {"role": "user",   "content": body}]
        prompt_ids = tok.apply_chat_template(messages, add_generation_prompt=True, enable_thinking=True)
        t0  = time.monotonic()
        raw = generate(model, tok, prompt=prompt_ids, max_tokens=32768,
                       sampler=sampler, logits_processors=lp, verbose=False)
        elapsed = round(time.monotonic() - t0, 1)
        think_chars = 0
        if "</think>" in raw:
            tpart, raw = raw.split("</think>", 1)
            think_chars = len(tpart)
        return raw.strip(), len(prompt_ids), len(tok.encode(raw)), elapsed
    finally:
        del model, tok
        gc.collect()
        mx.clear_cache()


def main() -> None:
    results = []

    for i, (hour, daily_plan, note) in enumerate(HOURS):
        print(f"\n{'═'*70}")
        print(f"[{i+1}/{len(HOURS)}] HOUR: {hour}   plan={daily_plan}   ({note})")
        print(f"{'═'*70}")

        # Distil
        t0 = time.monotonic()
        body, ds = distil_hour(hour)
        t_distil = round(time.monotonic() - t0, 1)
        if not body:
            print("  SKIP — no sessions")
            continue
        print(f"  distil: {ds.nsess} sess  {ds.raw_chars//1000}k→{ds.out_chars//1000}k chars "
              f"({ds.reduction_pct:.0f}%)  {t_distil}s")

        # Generate report
        print(f"  generating report ...", flush=True)
        report, in_tok, out_tok, elapsed = generate_report(body)
        print(f"  report: in_tok={in_tok}  out_tok={out_tok}  {elapsed}s")
        print(f"\n--- REPORT ({hour}) ---")
        print(report)
        print(f"--- END REPORT ---")

        results.append({
            "hour":        hour,
            "daily_plan":  daily_plan,
            "note":        note,
            "report":      report,
            "in_tok":      in_tok,
            "out_tok":     out_tok,
            "elapsed_s":   elapsed,
            "nsess":       ds.nsess,
            # label fields (filled in after reading)
            "label":       None,   # "KAN-xxx" or "untracked"
            "label_note":  "",
        })

        # Save after each hour so partial results survive
        OUT.write_text(json.dumps(results, indent=2))

    evict_embedder()

    print(f"\n\n{'='*70}")
    print(f"GENERATED {len(results)} reports → {OUT.name}")
    print(f"{'='*70}\n")
    print("Daily plan tickets:")
    for k, v in TICKET_TITLES.items():
        print(f"  {k}: {v}")
    print("\nNext step: read labeled_reports.json, fill in 'label' for each entry.")


if __name__ == "__main__":
    main()
