"""Summarise ONE app session with every model (1 call, full session_text).
Records input/output tokens, wall time, peak model RAM. Prints each summary for
eyeballing + flags any hallucinated ticket key.

Usage: python single_session_test.py <session_id>
"""
import json, re, time, subprocess, sys, urllib.request, sqlite3
import clib

SID = int(sys.argv[1]) if len(sys.argv) > 1 else 20174
OLLAMA = "http://127.0.0.1:11434"
TICKET_RE = re.compile(r"\b[A-Z]{2,5}-\d{1,5}\b")

# name, temp, top_p, top_k
MODELS = [
    ("qwen3:1.7b",   0.7, 0.8, 20),
    ("llama3.2:3b",  0.6, 0.9, 0),
    ("phi4-mini",    0.3, 0.95, 50),
    ("granite3.3:2b",0.6, 0.9, 50),
    ("qwen3.5:4b",   0.7, 0.8, 20),
    ("smollm2:1.7b", 0.3, 0.9, 50),
]

PROMPT = """You are writing a developer worklog entry from the noisy screen-capture OCR of ONE browser session.
Write a concise factual worklog (2-3 sentences): what the user was reading/researching and why it might matter.
Use ONLY what's in the text. Do NOT invent ticket numbers, products, or facts. No preamble.

SESSION OCR:
{txt}

Worklog entry:"""

def ps_gb():
    try:
        with urllib.request.urlopen(OLLAMA+"/api/ps",timeout=10) as r: d=json.loads(r.read())
        return round(max([m.get("size",0) for m in d.get("models",[])] or [0])/1e9,2)
    except Exception: return None

def gen(tag, temp, top_p, top_k, prompt):
    opts={"temperature":temp,"top_p":top_p,"num_ctx":16384,"num_predict":300}
    if top_k: opts["top_k"]=top_k
    body={"model":tag,"prompt":prompt,"stream":False,"options":opts,"keep_alive":"60s"}
    if tag.startswith("qwen3"): body["think"]=False
    t=time.time()
    req=urllib.request.Request(OLLAMA+"/api/generate",data=json.dumps(body).encode(),
                               headers={"Content-Type":"application/json"})
    with urllib.request.urlopen(req,timeout=600) as r: out=json.loads(r.read())
    return {"text":re.sub(r"<think>.*?</think>","",out.get("response",""),flags=re.S).strip(),
            "in_tok":out.get("prompt_eval_count"),"out_tok":out.get("eval_count"),
            "sec":round(time.time()-t,1),"ram":ps_gb()}

def main():
    con=sqlite3.connect(clib.DB)
    r=con.execute("SELECT duration_s,session_text,task_key,task_session_type FROM app_sessions WHERE id=?",(SID,)).fetchone()
    con.close()
    txt=re.sub(r"\s+"," ",clib.TS_RE.sub("",r[1] or "")).strip()
    print(f"SESSION {SID}: {r[0]}s · {len(txt)} chars (~{len(txt)//4} tok) · 9B tk={r[2]} type={r[3]}\n")
    prompt=PROMPT.format(txt=txt)
    rows=[]
    for tag,temp,top_p,top_k in MODELS:
        subprocess.run(["ollama","stop",tag],capture_output=True)
        try: res=gen(tag,temp,top_p,top_k,prompt)
        except Exception as e: print(f"!! {tag}: {e}"); continue
        halluc=sorted(set(TICKET_RE.findall(res["text"])))
        rows.append((tag,res,halluc))
        subprocess.run(["ollama","stop",tag],capture_output=True)
        print(f"{'='*72}\n## {tag}  in={res['in_tok']}tok out={res['out_tok']} wall={res['sec']}s RAM={res['ram']}GB  halluc_tickets={halluc or 'none'}")
        print(f"  {res['text']}\n")
    print(f"\n{'#'*72}\nSUMMARY")
    print(f"{'model':15}{'in_tok':>7}{'out':>5}{'sec':>6}{'RAM':>7}  halluc")
    for tag,res,halluc in rows:
        print(f"{tag:15}{res['in_tok']:>7}{res['out_tok']:>5}{res['sec']:>6}{(str(res['ram'])+'GB'):>7}  {halluc or '-'}")

if __name__=="__main__": main()
