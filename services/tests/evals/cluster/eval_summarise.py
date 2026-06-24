"""End-to-end summarisation eval: reduced hour text → LLM worklog → fact recall vs gold.

For a given hour:
  1. Build the reduced hour text (already proven at 78% fact recall)
  2. Feed it to each available small LLM with a PM worklog prompt
  3. Score the LLM's output against the gold session summaries using the same
     deterministic fact-extraction + substring check as audit_facts.py

Load/unload each model one at a time — never two resident simultaneously.

Usage: python eval_summarise.py [YYYY-MM-DDTHH] [hour2] ...
       (defaults to the best 2 peak hours if no args given)
"""
import sys, os, re, json, time, subprocess, sqlite3, urllib.request
from collections import defaultdict
sys.path.insert(0, os.path.dirname(__file__))
import clib
from hour_text import build_hour_text, EXCLUDE, MIN_DUR
from audit_facts import extract_facts, fact_present, gold_for_hour

OLLAMA = "http://127.0.0.1:11434"

# ── prompt designed for PM worklog from noisy reduced screen-capture text ──────
WORKLOG_PROMPT = """You are writing a developer's PM work-log entry for exactly ONE HOUR of work, from compressed screen-capture excerpts.

Each block is labelled [HH:MM · App · Window]. Text is OCR/accessibility output — expect garbled words but real content. Infer through noise.

Your task: write a FACTUAL, DENSE work-log that a project manager can read to understand what was accomplished. Requirements:
- Cover EVERY distinct piece of work visible in the text (do not omit small tasks)
- For each work thread: 1–2 sentences on what was done, with the EXACT specifics from the text — file names, ticket keys (KAN-NNN), PR numbers (#NNN), function names, feature names, error messages, decisions reached
- Do NOT invent details. If a ticket key appears in the text, copy it exactly (e.g. KAN-241). Same for PR numbers and file paths.
- If a session shows debugging/investigation: name the error or symptom
- If a session shows browser research: name the specific topic or page
- Keep idle / tool-loading / overhead short (one line each)
- End with: "Time split:" followed by a one-line breakdown by app or ticket (e.g. "Code/KAN-241 40% · Chrome 30% · Terminal 15% · DBeaver 15%")

Do not summarise the prompt instructions. Output only the work-log.

=== HOUR ACTIVITY ({hour}) ===
{body}

=== WORK-LOG ==="""


def list_models() -> list[str]:
    """Return model tags from `ollama list`, ordered smallest→largest."""
    try:
        out = subprocess.check_output(["ollama", "list"], text=True)
    except Exception:
        return []
    lines = out.strip().splitlines()[1:]  # skip header
    models = []
    for ln in lines:
        parts = ln.split()
        if parts:
            models.append(parts[0])
    # heuristic size order: put qwen3:0.6b / smollm first, big ones last
    def size_key(tag):
        for key, val in [("0.6b", 0), ("1.7b", 1), ("3b", 2), ("2b", 3),
                         ("mini", 4), ("3.3", 5), ("4b", 6), ("3.5:latest", 99)]:
            if key in tag.lower():
                return val
        return 50
    return sorted(models, key=size_key)


def stop_all():
    for m in list_models():
        subprocess.run(["ollama", "stop", m], capture_output=True)


def generate(model: str, prompt: str, ctx: int = 16384, max_tok: int = 900) -> tuple[str, dict]:
    opts = {"temperature": 0.1, "num_ctx": ctx, "num_predict": max_tok}
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
    meta = {"in_tok": out.get("prompt_eval_count"), "out_tok": out.get("eval_count"),
            "dur_s": round(out.get("total_duration", 0) / 1e9, 1)}
    return text, meta


def score_summary(summary: str, golds: list[dict]) -> dict:
    """Fact-recall of LLM summary vs gold session summaries."""
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
            missing.append(f'  [{tag}] {", ".join(lost[:5])}')
    recall = found / total if total else 1.0
    return {"total": total, "found": found, "recall": recall,
            "no_facts": no_facts, "missing": missing}


def eval_hour(hour: str, models: list[str]):
    print(f"\n{'='*72}")
    print(f"HOUR: {hour}")
    print(f"{'='*72}")

    # Build reduced hour text
    t0 = time.time()
    body, st = build_hour_text(hour)
    print(f"Hour text: {st['nsess']} sessions · {st['raw_chars']//1000}k→{st['out_chars']//1000}k chars "
          f"(~{st['out_chars']//4} tok) · {time.time()-t0:.1f}s\n")

    golds = gold_for_hour(hour)
    if not golds:
        print("No gold sessions for this hour — skip")
        return

    # Baseline: fact recall of the raw hour text (pre-LLM)
    base = score_summary(body, golds)
    print(f"BASELINE (hour text before LLM)  recall={base['recall']:.1%}  "
          f"facts={base['total']} found={base['found']}")
    print()

    results = []
    prompt = WORKLOG_PROMPT.format(hour=hour, body=body)

    for model in models:
        print(f"── {model} ──")
        # Stop everything else first
        stop_all()
        time.sleep(1)

        t0 = time.time()
        try:
            summary, meta = generate(model, prompt)
        except Exception as e:
            print(f"  ERROR: {e}\n")
            continue
        elapsed = time.time() - t0

        sc = score_summary(summary, golds)
        tps = round((meta["out_tok"] or 0) / max(elapsed, 1), 1)

        print(f"  recall={sc['recall']:.1%}  facts={sc['total']} found={sc['found']}  "
              f"in={meta['in_tok']}tok out={meta['out_tok']}tok  {tps}tok/s  {elapsed:.0f}s")

        # Sample of missing facts
        if sc["missing"]:
            print(f"  missing ({len(sc['missing'])} sessions):")
            for l in sc["missing"][:8]:
                print(l)

        print(f"\n--- {model} WORKLOG ---")
        print(summary[:3000])
        if len(summary) > 3000:
            print(f"  ... [{len(summary)-3000} chars truncated]")
        print()

        results.append({"model": model, **sc, **meta, "tps": tps})

        # Unload immediately
        subprocess.run(["ollama", "stop", model], capture_output=True)
        time.sleep(1)

    # Final comparison table
    print(f"\n{'='*72}")
    print(f"SUMMARY — {hour}")
    print(f"{'='*72}")
    print(f"  {'Model':30} {'Recall':>8} {'Found/Total':>12} {'In tok':>7} {'Out tok':>7} {'tok/s':>7}")
    print(f"  {'BASELINE (hour text)':30} {base['recall']:>7.1%} {base['found']:>5}/{base['total']:<5}")
    for r in results:
        print(f"  {r['model']:30} {r['recall']:>7.1%} {r['found']:>5}/{r['total']:<5} "
              f"{r['in_tok'] or 0:>7} {r['out_tok'] or 0:>7} {r['tps']:>7.1f}")
    return results


def peak_hours(n: int = 12) -> list[str]:
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
    hours = sys.argv[1:] if len(sys.argv) > 1 else peak_hours(12)
    models = list_models()

    print(f"Models ({len(models)}): {models}")
    print(f"Hours  ({len(hours)}): {hours}\n")
    stop_all()

    # ── Phase 1: build all reduced hour texts up front (fast, cached) ──────────
    print("Building reduced hour texts...")
    hour_data: dict[str, tuple[str, dict, list]] = {}  # hour → (body, stats, golds)
    for hour in hours:
        body, st = build_hour_text(hour)
        golds = gold_for_hour(hour)
        base = score_summary(body, golds)
        hour_data[hour] = (body, st, golds, base)
        print(f"  {hour}  {st['nsess']:2}s  {st['raw_chars']//1000:3}k→{st['out_chars']//1000:2}k chars  "
              f"baseline={base['recall']:.0%}  gold={len(golds)}")

    print()

    # ── Phase 2: for each model, run all hours, then unload ───────────────────
    # results[model][hour] = score dict + generated text
    results: dict[str, dict] = {m: {} for m in models}

    for model in models:
        print(f"\n{'='*72}")
        print(f"MODEL: {model}")
        print(f"{'='*72}")
        stop_all(); time.sleep(1)

        for hour in hours:
            body, st, golds, base = hour_data[hour]
            prompt = WORKLOG_PROMPT.format(hour=hour, body=body)
            try:
                t0 = time.time()
                summary, meta = generate(model, prompt)
                elapsed = time.time() - t0
            except Exception as e:
                print(f"  {hour}  ERROR: {e}")
                results[model][hour] = {"recall": 0.0, "error": str(e)}
                continue

            sc = score_summary(summary, golds)
            tps = round((meta["out_tok"] or 0) / max(elapsed, 1), 1)
            results[model][hour] = {**sc, **meta, "tps": tps, "summary": summary,
                                    "baseline": base["recall"]}

            delta = sc["recall"] - base["recall"]
            print(f"  {hour}  baseline={base['recall']:.0%}  "
                  f"→ recall={sc['recall']:.0%} ({delta:+.0%})  "
                  f"out={meta['out_tok']}tok  {tps}tok/s  {elapsed:.0f}s")

        stop_all(); time.sleep(1)

    # ── Phase 3: print generated worklogs ──────────────────────────────────────
    print(f"\n\n{'#'*72}")
    print("GENERATED WORKLOGS")
    print(f"{'#'*72}")
    for hour in hours:
        print(f"\n{'='*72}")
        print(f"HOUR: {hour}")
        print(f"{'='*72}")
        for model in models:
            r = results[model].get(hour, {})
            if "error" in r:
                print(f"\n[{model}] ERROR: {r['error']}")
                continue
            print(f"\n[{model}]  recall={r.get('recall',0):.0%}  out={r.get('out_tok')}tok")
            print(r.get("summary", "")[:1500])
            if len(r.get("summary","")) > 1500:
                print(f"  ...[{len(r['summary'])-1500} chars truncated]")

    # ── Phase 4: grand summary table ──────────────────────────────────────────
    print(f"\n\n{'#'*72}")
    print("GRAND SUMMARY — fact recall vs gold session summaries")
    print(f"{'#'*72}")
    print(f"\n  {'Hour':18} {'Base':>6} ", end="")
    for m in models:
        short = m.split(":")[0][-10:] + (":" + m.split(":")[-1])[-5:] if ":" in m else m[-14:]
        print(f" {short:>10}", end="")
    print()

    grand_base = grand_model = {m: {"found": 0, "total": 0} for m in models}
    grand_base_total = grand_base_found = 0

    for hour in hours:
        _, _, _, base = hour_data[hour]
        print(f"  {hour}  {base['recall']:>5.0%} ", end="")
        grand_base_total += base["total"]; grand_base_found += base["found"]
        for m in models:
            r = results[m].get(hour, {})
            rc = r.get("recall", 0.0)
            grand_model[m]["found"] += r.get("found", 0)
            grand_model[m]["total"] += r.get("total", 0)
            print(f" {rc:>9.0%} ", end="")
        print()

    # Grand totals row
    grand_base_rc = grand_base_found / grand_base_total if grand_base_total else 0
    print(f"\n  {'GRAND TOTAL':18} {grand_base_rc:>5.0%} ", end="")
    for m in models:
        t = grand_model[m]["total"]; f = grand_model[m]["found"]
        rc = f / t if t else 0
        print(f" {rc:>9.0%} ", end="")
    print()

    print(f"\n  tok/s (avg): ", end="")
    for m in models:
        tps_vals = [results[m][h].get("tps", 0) for h in hours if "tps" in results[m].get(h, {})]
        avg = sum(tps_vals) / len(tps_vals) if tps_vals else 0
        print(f" {avg:>9.1f} ", end="")
    print()


if __name__ == "__main__":
    main()


if __name__ == "__main__":
    main()
