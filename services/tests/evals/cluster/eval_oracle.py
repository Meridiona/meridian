"""Oracle comparison: gold session summaries → LLM hour worklog vs our pipeline output.

For each peak hour:
  A. GOLD ORACLE TEXT  = all session_summary values for that hour concatenated in order.
     This is the BEST POSSIBLE input — what a perfect per-session classifier would hand us.
  B. REDUCED HOUR TEXT = output of our noise-reduction pipeline (hour_text.py).

Feed both to the same LLM with the same prompt, score both outputs against the same
ground-truth facts (extracted from the session_summaries themselves).

This isolates exactly two questions:
  1. How much does our noise-reduction lose vs perfect session-level summaries? (A vs B input)
  2. How much does the LLM compress away? (output recall vs input baseline)

Usage: python eval_oracle.py [model] [N_hours]
  model    default: qwen3.5:4b
  N_hours  default: 12
"""
import sys, os, re, json, time, subprocess, sqlite3, urllib.request
from collections import defaultdict
sys.path.insert(0, os.path.dirname(__file__))
import clib
from hour_text import build_hour_text, EXCLUDE, MIN_DUR
from audit_facts import extract_facts, fact_present, gold_for_hour

OLLAMA = "http://127.0.0.1:11434"
MODEL  = sys.argv[1] if len(sys.argv) > 1 else "qwen3.5:4b"
N_HOURS = int(sys.argv[2]) if len(sys.argv) > 2 else 12


WORKLOG_PROMPT = """You are writing a developer's PM work-log entry for exactly ONE HOUR of work.

The text below is time-ordered session information. Write a FACTUAL, DENSE work-log for a project manager.
Requirements:
- Cover EVERY distinct piece of work (do not omit anything)
- For each thread: 1-2 sentences with EXACT specifics — file names, ticket keys (KAN-NNN), PR numbers (#NNN), function names, feature names, decisions
- Do NOT invent. Copy ticket keys and PR numbers exactly as they appear.
- End with: "Time split:" and a one-line breakdown by ticket/app
Output only the work-log.

=== HOUR ACTIVITY ({hour}) ===
{body}

=== WORK-LOG ==="""


def stop_all():
    out = subprocess.check_output(["ollama", "list"], text=True)
    for line in out.splitlines()[1:]:
        parts = line.split()
        if parts:
            subprocess.run(["ollama", "stop", parts[0]], capture_output=True)


def generate(model: str, prompt: str) -> tuple[str, dict]:
    opts = {"temperature": 0.1, "num_ctx": 16384, "num_predict": 900}
    body = {"model": model, "prompt": prompt, "stream": False,
            "options": opts, "keep_alive": "90s"}
    if "qwen3" in model:
        body["think"] = False
    req = urllib.request.Request(
        OLLAMA + "/api/generate",
        data=json.dumps(body).encode(),
        headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=600) as r:
        out = json.loads(r.read())
    text = out.get("response", "").strip()
    meta = {"in_tok": out.get("prompt_eval_count"), "out_tok": out.get("eval_count")}
    return text, meta


def score_summary(summary: str, golds: list[dict]) -> dict:
    total = found = no_facts = 0
    missing = []
    for g in golds:
        facts = extract_facts(g["gold"])
        if not facts:
            no_facts += 1
            continue
        total += len(facts)
        pres = [fact_present(f, summary) for f in facts]
        found += sum(pres)
        lost = [f for f, ok in zip(facts, pres) if not ok]
        if lost:
            tag = f'{g["app"]}{"/"+g["tk"] if g["tk"] else ""}|{g["type"]}'
            missing.append(f'  [{tag}] {", ".join(lost[:4])}')
    recall = found / total if total else 1.0
    return {"total": total, "found": found, "recall": recall,
            "no_facts": no_facts, "missing": missing}


def build_oracle_text(hour: str, golds: list[dict]) -> str:
    """Concatenate all session_summary values in time order — perfect input."""
    lines = [f"=== HOUR {hour[11:]}:00 · {len(golds)} sessions (gold summaries) ==="]
    for g in golds:
        tag = f'{g["app"]}{" / "+g["tk"] if g["tk"] else ""}'
        lines.append(f'\n[{tag} | {g["type"] or "?"}]')
        lines.append(f'  {g["gold"][:400]}')
    return "\n".join(lines)


def peak_hours(n: int) -> list[str]:
    con = sqlite3.connect(clib.DB)
    rows = con.execute(
        f"""SELECT substr(started_at,1,13) h, COUNT(*) c,
                   SUM(CASE WHEN session_summary IS NOT NULL AND LENGTH(session_summary)>5 THEN 1 ELSE 0 END) g
            FROM app_sessions
            WHERE app_name NOT IN ({','.join('?'*len(EXCLUDE))})
              AND duration_s >= ? AND session_text IS NOT NULL AND LENGTH(session_text)>40
              AND started_at >= date('now','-7 days')
            GROUP BY h HAVING g >= 5 ORDER BY c DESC LIMIT ?""",
        (*EXCLUDE, MIN_DUR, n)).fetchall()
    con.close()
    return [r[0] for r in rows]


def main():
    hours = peak_hours(N_HOURS)
    print(f"Oracle eval  model={MODEL}  hours={len(hours)}")
    print(f"Hours: {hours}\n")

    stop_all(); time.sleep(1)

    rows_out = []  # for final table

    for hour in hours:
        print(f"{'='*72}")
        print(f"HOUR: {hour}")

        golds = gold_for_hour(hour)
        if not golds:
            print("  no gold — skip"); continue

        # Build both texts
        reduced_body, st = build_hour_text(hour)
        oracle_body = build_oracle_text(hour, golds)

        # Baseline: fact recall of the raw reduced text
        base_reduced = score_summary(reduced_body, golds)
        base_oracle  = score_summary(oracle_body, golds)

        reduced_tok = len(reduced_body) // 4
        oracle_tok  = len(oracle_body) // 4

        print(f"  sessions={st['nsess']}  gold={len(golds)}")
        print(f"  reduced text: {st['raw_chars']//1000}k→{st['out_chars']//1000}k chars "
              f"(~{reduced_tok}tok)  baseline={base_reduced['recall']:.0%}")
        print(f"  oracle  text: {len(oracle_body)//1000}k chars "
              f"(~{oracle_tok}tok)  baseline={base_oracle['recall']:.0%}")

        # Generate worklog from REDUCED text
        t0 = time.time()
        summ_reduced, meta_r = generate(MODEL, WORKLOG_PROMPT.format(hour=hour, body=reduced_body))
        dt_r = time.time() - t0
        sc_r = score_summary(summ_reduced, golds)

        # Generate worklog from ORACLE (gold summaries)
        t0 = time.time()
        summ_oracle, meta_o = generate(MODEL, WORKLOG_PROMPT.format(hour=hour, body=oracle_body))
        dt_o = time.time() - t0
        sc_o = score_summary(summ_oracle, golds)

        tps_r = round((meta_r["out_tok"] or 0) / max(dt_r, 1), 1)
        tps_o = round((meta_o["out_tok"] or 0) / max(dt_o, 1), 1)

        print(f"\n  FROM REDUCED:  recall={sc_r['recall']:.0%}  "
              f"(baseline={base_reduced['recall']:.0%})  "
              f"out={meta_r['out_tok']}tok  {tps_r}tok/s")
        print(f"  FROM ORACLE:   recall={sc_o['recall']:.0%}  "
              f"(baseline={base_oracle['recall']:.0%})  "
              f"out={meta_o['out_tok']}tok  {tps_o}tok/s")

        print(f"\n  --- REDUCED worklog ---")
        print(summ_reduced[:1200])
        print(f"\n  --- ORACLE worklog ---")
        print(summ_oracle[:1200])

        rows_out.append({
            "hour": hour, "nsess": st["nsess"], "ngold": len(golds),
            "base_reduced": base_reduced["recall"],
            "base_oracle":  base_oracle["recall"],
            "recall_reduced": sc_r["recall"],
            "recall_oracle":  sc_o["recall"],
        })

    # ── Grand summary table ────────────────────────────────────────────────────
    print(f"\n\n{'='*72}")
    print(f"GRAND SUMMARY  model={MODEL}")
    print(f"{'='*72}")
    print(f"  {'Hour':18} {'Sess':>4} {'BaseRed':>8} {'BaseOrc':>8} "
          f"{'LLM(Red)':>9} {'LLM(Orc)':>9}  Gap")
    tot_br = tot_bo = tot_lr = tot_lo = 0
    n = 0
    for r in rows_out:
        gap = r["recall_oracle"] - r["recall_reduced"]
        print(f"  {r['hour']}  {r['nsess']:3}  "
              f"{r['base_reduced']:>7.0%}  {r['base_oracle']:>7.0%}  "
              f"{r['recall_reduced']:>8.0%}  {r['recall_oracle']:>8.0%}  {gap:+.0%}")
        tot_br += r["base_reduced"]; tot_bo += r["base_oracle"]
        tot_lr += r["recall_reduced"]; tot_lo += r["recall_oracle"]
        n += 1

    if n:
        print(f"  {'AVERAGE':18}       "
              f"{tot_br/n:>7.0%}  {tot_bo/n:>7.0%}  "
              f"{tot_lr/n:>8.0%}  {tot_lo/n:>8.0%}  {(tot_lo-tot_lr)/n:+.0%}")

    print(f"\nKey:")
    print(f"  BaseRed  = facts present in the reduced hour text (pre-LLM)")
    print(f"  BaseOrc  = facts present in the concatenated gold summaries (pre-LLM)")
    print(f"  LLM(Red) = facts in LLM worklog generated FROM reduced text")
    print(f"  LLM(Orc) = facts in LLM worklog generated FROM oracle (gold summaries)")
    print(f"  Gap      = what the LLM gains from perfect vs noisy input")

    stop_all()


if __name__ == "__main__":
    main()
