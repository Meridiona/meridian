"""Sequential SLM summarisation bake-off on ONE cluster. One model resident at a
time (unload before load). Per-model documented sampling settings. Measures peak
RAM, latency, and auto-scores faithfulness vs the evidence pack.

Usage: python bakeoff2.py [seed_id]
"""
import json, re, time, subprocess, sys, urllib.request
from reduce import build_blob

SEED = int(sys.argv[1]) if len(sys.argv) > 1 else 37166
OLLAMA = "http://127.0.0.1:11434"

# (name, backend, repo/tag, temp, top_p, top_k, extra)
MODELS = [
    ("Qwen3.5-0.8B",  "mlx",    "mlx-community/Qwen3.5-0.8B-OptiQ-4bit", 0.7, 0.8, 20, {}),
    ("Qwen3.5-4B",    "mlx",    "mlx-community/Qwen3.5-4B-OptiQ-4bit",   0.7, 0.8, 20, {}),
    ("qwen3:1.7b",    "ollama", "qwen3:1.7b",     0.7, 0.8, 20, {"think": False}),
    ("smollm2:1.7b",  "ollama", "smollm2:1.7b",   0.3, 0.9, 50, {}),
    ("granite3.3:2b", "ollama", "granite3.3:2b",  0.6, 0.9, 50, {"min_p": 0.01}),
    ("llama3.2:3b",   "ollama", "llama3.2:3b",    0.6, 0.9,  0, {}),
    ("phi4-mini",     "ollama", "phi4-mini",      0.3, 0.95, 50, {}),
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

def ollama_stop(tag):
    try: subprocess.run(["ollama", "stop", tag], capture_output=True, timeout=60)
    except Exception: pass

def ollama_ps_gb():
    try:
        with urllib.request.urlopen(OLLAMA + "/api/ps", timeout=10) as r:
            d = json.loads(r.read())
        return round(max([m.get("size", 0) for m in d.get("models", [])] or [0]) / 1e9, 2)
    except Exception: return None

def run_ollama(tag, temp, top_p, top_k, extra, prompt):
    ollama_stop(tag)
    opts = {"temperature": temp, "top_p": top_p, "num_ctx": 4096, "num_predict": 400}
    if top_k: opts["top_k"] = top_k
    if "min_p" in extra: opts["min_p"] = extra["min_p"]
    body = {"model": tag, "prompt": prompt, "stream": False, "options": opts, "keep_alive": "120s"}
    if "think" in extra: body["think"] = extra["think"]
    t = time.time()
    req = urllib.request.Request(OLLAMA + "/api/generate", data=json.dumps(body).encode(),
                                 headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=600) as r:
        out = json.loads(r.read())
    dt = time.time() - t
    mem = ollama_ps_gb()
    ollama_stop(tag)
    return out.get("response", "").strip(), dt, mem

def run_mlx(repo, temp, top_p, top_k, prompt):
    with open("/tmp/_mlx_prompt.txt", "w") as f: f.write(prompt)
    p = subprocess.run(["../../../../services/.venv/bin/python", "mlx_one.py", repo,
                        "/tmp/_mlx_prompt.txt", str(temp), str(top_p), str(top_k)],
                       capture_output=True, text=True, timeout=900)
    line = [l for l in p.stdout.splitlines() if l.startswith("@@JSON@@")]
    if not line:
        print("MLX stderr:", p.stderr[-800:]); return "(failed)", 0, None
    d = json.loads(line[0][len("@@JSON@@"):])
    return d["summary"], d["gen_s"], d["peak_gb"]

def score(text, gold):
    low = text.lower()
    tk, fl = gold["tickets"], {f.lower() for f in gold["files"]}
    tk_hit = sorted(t for t in tk if t.lower() in low)
    tk_miss = sorted(t for t in tk if t.lower() not in low)
    fl_hit = sorted(f for f in fl if f in low or f.split("/")[-1] in low)
    halluc = sorted(set(TICKET_RE.findall(text)) - tk)
    return tk_hit, tk_miss, fl_hit, halluc

def strip_think(t):
    return re.sub(r"<think>.*?</think>", "", t, flags=re.S).strip()

def main():
    blob, gold, nmem = build_blob(SEED)
    print(f"CLUSTER seed={SEED}: {nmem} sessions · gold tickets={sorted(gold['tickets'])} "
          f"files={len(gold['files'])}\nblob ~{len(blob)//4} tok\n")
    prompt = PROMPT.format(blob=blob)
    rows = []
    for name, backend, repo, temp, top_p, top_k, extra in MODELS:
        print(f"\n{'='*72}\n>>> {name}  (temp={temp} top_p={top_p} top_k={top_k} {extra})")
        try:
            if backend == "mlx":
                text, dt, mem = run_mlx(repo, temp, top_p, top_k, prompt)
            else:
                text, dt, mem = run_ollama(repo, temp, top_p, top_k, extra, prompt)
        except Exception as e:
            print(f"  !! failed: {e}"); continue
        text = strip_think(text)
        tk_hit, tk_miss, fl_hit, halluc = score(text, gold)
        rows.append((name, dt, mem, tk_hit, tk_miss, fl_hit, halluc, text))
        print(f"  time={dt:.1f}s  peak_RAM={mem}GB")
        print(f"  tickets HIT={tk_hit or '-'}  MISSED={tk_miss or '-'}")
        print(f"  files HIT={fl_hit or '-'}")
        print(f"  HALLUCINATED tickets={halluc or 'none'}")
        print(f"  --- summary ---\n{text}\n")
    print(f"\n{'#'*72}\nSCOREBOARD ({nmem}-session cluster, gold={sorted(gold['tickets'])})")
    print(f"{'model':15}{'sec':>6}{'RAM':>7}  {'tkHIT':>5}/{len(gold['tickets'])} {'files':>5}  hallucinated")
    for name, dt, mem, tk_hit, tk_miss, fl_hit, halluc, _ in rows:
        print(f"{name:15}{dt:>6.1f}{(str(mem)+'GB'):>7}  {len(tk_hit):>5}/{len(gold['tickets'])} "
              f"{len(fl_hit):>5}  {halluc or '-'}")

if __name__ == "__main__":
    main()
