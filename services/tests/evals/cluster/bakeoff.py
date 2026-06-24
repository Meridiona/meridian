"""Summarisation bake-off across local SLMs via Ollama. Framework-neutral: we
compare SUMMARY QUALITY, not the serving stack. Each model gets the same reduced
cluster blob + worklog prompt; we auto-score faithfulness (reproduces evidence-
pack tickets/files? invents any ticket key?) and record latency.

Usage: python bakeoff.py
"""
import json, re, time, urllib.request, sys
from reduce import build_blob

OLLAMA = "http://127.0.0.1:11434/api/generate"
MODELS = [m.strip() for m in (sys.argv[1].split(",") if len(sys.argv) > 1 else
          ["qwen3:1.7b", "llama3.2:3b", "phi4-mini", "gemma3:4b", "granite3.3:2b"])]

CLUSTERS = [
    ("C7-code", 37166),   # 20 sessions, Code+DBeaver, multi-topic, real tickets
    ("C4-deno", 37059),   # 3 sessions, browser research, untracked (no ticket)
]

PROMPT = """You are writing a developer worklog entry from noisy screen-capture OCR of one work session.
The EVIDENCE PACK lists exact facts (tickets, files, commands, time) extracted reliably.
The REPRESENTATIVE EXCERPTS are noisy/garbled OCR samples of what was on screen.

Write a concise factual worklog (2-4 sentences) describing what the developer worked on.
Rules:
- Use ONLY facts present in the input. Do NOT invent ticket numbers, files, or features.
- Name the specific tickets and files actually worked on, drawn from the evidence pack.
- If the work is not tied to any ticket, say so plainly.
- No preamble, no markdown headers. Just the worklog text.

{blob}

Worklog entry:"""

TICKET_RE = re.compile(r"\b[A-Z]{2,5}-\d{1,5}\b")

def call(model, prompt):
    body = json.dumps({"model": model, "prompt": prompt, "stream": False,
                       "options": {"temperature": 0.2, "num_ctx": 4096, "num_predict": 300}}).encode()
    t = time.time()
    req = urllib.request.Request(OLLAMA, data=body, headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=600) as r:
        out = json.loads(r.read())
    return out.get("response", "").strip(), time.time() - t

def score(text, gold):
    low = text.lower()
    tk = gold["tickets"]; fl = {f.lower() for f in gold["files"]}
    tk_hit = sum(1 for t in tk if t.lower() in low)
    fl_hit = sum(1 for f in fl if f in low or f.split("/")[-1] in low)
    found = set(TICKET_RE.findall(text))
    halluc = sorted(found - tk)
    return {
        "tickets": f"{tk_hit}/{len(tk)}" if tk else "n/a",
        "files": f"{fl_hit}/{len(fl)}" if fl else "n/a",
        "halluc_tickets": halluc,
    }

def main():
    blobs = {name: build_blob(seed) for name, seed in CLUSTERS}
    results = []
    for model in MODELS:
        for name, seed in CLUSTERS:
            blob, gold, nmem = blobs[name]
            try:
                text, dt = call(model, PROMPT.format(blob=blob))
            except Exception as e:
                print(f"!! {model} on {name}: {e}")
                continue
            sc = score(text, gold)
            results.append((model, name, dt, sc, text))
            print(f"\n{'='*70}\n## {model}  ·  {name}  ({dt:.1f}s)")
            print(f"   tickets={sc['tickets']}  files={sc['files']}  hallucinated={sc['halluc_tickets'] or 'none'}")
            print(f"   ---\n   {text}")
    print(f"\n\n{'='*70}\nSUMMARY TABLE")
    print(f"{'model':16} {'cluster':9} {'sec':>5} {'tickets':>8} {'files':>7} {'hallucinated'}")
    for model, name, dt, sc, _ in results:
        print(f"{model:16} {name:9} {dt:>5.1f} {sc['tickets']:>8} {sc['files']:>7} {sc['halluc_tickets'] or '-'}")

if __name__ == "__main__":
    main()
