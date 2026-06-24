"""Validate the hour session-text: feed it to small LLMs with a PM-worklog prompt and
see if the summary is faithful + usable. One model resident at a time (stop between).

Usage: python validate_hour.py <YYYY-MM-DDTHH> [model1,model2,...]
"""
import sys, os, json, time, subprocess, urllib.request

OLLAMA = "http://127.0.0.1:11434"
HOUR = sys.argv[1] if len(sys.argv) > 1 else "2026-06-23T05"
MODELS = (sys.argv[2].split(",") if len(sys.argv) > 2 else
          ["granite3.3:2b", "phi4-mini", "qwen3.5:4b"])

PROMPT = """You are writing a developer's work-log for one hour, from noisy screen-capture text.
The text below is time-ordered excerpts from apps used this hour (terminal/editor narration,
browser pages, etc). It is reduced and may contain OCR errors — infer through them.

Write a factual work-log of THIS HOUR. Requirements:
- List the distinct pieces of work (tasks/threads) the developer actually advanced.
- For each: 1-2 sentences on what was done, with concrete specifics (files, PRs, tickets,
  features, decisions) ONLY if they appear in the text. Do not invent.
- Mention any ticket keys (like KAN-123) or PR numbers exactly as they appear.
- End with a one-line "Time spent on:" list.
Do not summarize the instructions. Output only the work-log.

=== HOUR ACTIVITY ===
{body}

=== WORK-LOG ==="""


def ps_gb():
    try:
        with urllib.request.urlopen(OLLAMA + "/api/ps", timeout=10) as r:
            d = json.loads(r.read())
        return round(max([m.get("size", 0) for m in d.get("models", [])] or [0]) / 1e9, 2)
    except Exception:
        return None


def gen(tag, prompt):
    opts = {"temperature": 0.2, "num_ctx": 16384, "num_predict": 700}
    body = {"model": tag, "prompt": prompt, "stream": False, "options": opts, "keep_alive": "60s"}
    if tag.startswith("qwen3"):
        body["think"] = False
    req = urllib.request.Request(OLLAMA + "/api/generate", data=json.dumps(body).encode(),
                                 headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=600) as r:
        out = json.loads(r.read())
    return out


def main():
    path = os.path.join(os.path.dirname(__file__), f"hourtext_{HOUR}.txt")
    body = open(path).read()
    in_tok_est = len(body) // 4
    print(f"INPUT: {len(body)} chars (~{in_tok_est} tok) from {path}\n")
    for tag in MODELS:
        subprocess.run(["ollama", "stop", tag], capture_output=True)
        t0 = time.time()
        try:
            out = gen(tag, PROMPT.format(body=body))
        except Exception as e:
            print(f"!! {tag}: {e}\n"); continue
        dt = time.time() - t0
        txt = out.get("response", "").strip()
        it, ot = out.get("prompt_eval_count"), out.get("eval_count")
        ram = ps_gb()
        print("=" * 72)
        print(f"## {tag}   {dt:.1f}s   in={it}tok out={ot}tok   RAM={ram}GB")
        print("=" * 72)
        print(txt)
        print()
        subprocess.run(["ollama", "stop", tag], capture_output=True)


if __name__ == "__main__":
    main()
