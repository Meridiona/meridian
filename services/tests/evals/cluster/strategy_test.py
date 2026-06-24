"""Compare summarisation STRATEGIES for the same model on one cluster:
  A REDUCED   : evidence pack + 4 representatives  (1 call, tiny input)
  B FULL      : evidence pack + ALL sessions raw   (1 call, big input)
  C MAPREDUCE : summarise chunks of sessions, then combine (N+1 calls)
Records exact input/output TOKENS (from ollama), wall latency, peak RAM (ollama
/api/ps), peak CPU% + RSS (ps sampler over ollama procs), and faithfulness.

Usage: python strategy_test.py <ollama_tag> [seed] [temp] [top_p] [top_k]
"""
import json, re, time, subprocess, sys, threading, urllib.request
import numpy as np
import clib, embed
from reduce import _load_window, cluster_members, pick_reps, evidence_pack

TAG = sys.argv[1] if len(sys.argv) > 1 else "qwen3:1.7b"
SEED = int(sys.argv[2]) if len(sys.argv) > 2 else 37166
TEMP = float(sys.argv[3]) if len(sys.argv) > 3 else 0.7
TOP_P = float(sys.argv[4]) if len(sys.argv) > 4 else 0.8
TOP_K = int(sys.argv[5]) if len(sys.argv) > 5 else 20
OLLAMA = "http://127.0.0.1:11434"
TICKET_RE = re.compile(r"\b[A-Z]{2,5}-\d{1,5}\b")

WORKLOG = """You are writing a developer worklog entry from noisy screen-capture OCR of one work session.
Write a concise factual worklog (2-4 sentences): what the developer worked on, which tickets and files.
Use ONLY facts in the input. Do NOT invent ticket numbers or files. If no ticket applies, say so. No preamble.

{blob}

Worklog entry:"""

CHUNK_SUM = """Summarise what the developer did in these screen-capture OCR excerpts in 2 sentences.
Mention concrete tickets/files/actions only if present. Noisy OCR; do not invent.

{blob}

Summary:"""

COMBINE = """Combine these partial summaries of ONE work session into a single worklog entry (2-4 sentences).
Evidence pack of exact facts:
{pack}

Partial summaries:
{parts}

Use only facts above; do not invent tickets/files. If no ticket applies, say so. Worklog entry:"""

class Sampler(threading.Thread):
    """Poll ollama process CPU%/RSS while a call runs."""
    def __init__(self):
        super().__init__(daemon=True); self.stop=False; self.cpu=0.0; self.rss=0
    def run(self):
        pids = subprocess.run(["pgrep","-f","ollama"],capture_output=True,text=True).stdout.split()
        while not self.stop:
            tot_cpu=0.0; tot_rss=0
            for pid in pids:
                r=subprocess.run(["ps","-o","%cpu=","-o","rss=","-p",pid],capture_output=True,text=True).stdout.split()
                if len(r)==2:
                    tot_cpu+=float(r[0]); tot_rss+=int(r[1])
            self.cpu=max(self.cpu,tot_cpu); self.rss=max(self.rss,tot_rss)
            time.sleep(0.3)

def ps_gb():
    try:
        with urllib.request.urlopen(OLLAMA+"/api/ps",timeout=10) as r:
            d=json.loads(r.read())
        return round(max([m.get("size",0) for m in d.get("models",[])] or [0])/1e9,2)
    except Exception: return None

def gen(prompt, num_ctx):
    opts={"temperature":TEMP,"top_p":TOP_P,"num_ctx":num_ctx,"num_predict":400}
    if TOP_K: opts["top_k"]=TOP_K
    body={"model":TAG,"prompt":prompt,"stream":False,"options":opts,"keep_alive":"120s"}
    if TAG.startswith("qwen3"): body["think"]=False
    smp=Sampler(); smp.start()
    t=time.time()
    req=urllib.request.Request(OLLAMA+"/api/generate",data=json.dumps(body).encode(),
                               headers={"Content-Type":"application/json"})
    with urllib.request.urlopen(req,timeout=900) as r: out=json.loads(r.read())
    dt=time.time()-t; smp.stop=True; smp.join()
    return {"text":re.sub(r"<think>.*?</think>","",out.get("response",""),flags=re.S).strip(),
            "in_tok":out.get("prompt_eval_count"),"out_tok":out.get("eval_count"),
            "sec":round(dt,1),"ram_gb":ps_gb(),"cpu_pct":round(smp.cpu,0),"rss_gb":round(smp.rss/1e6,2)}

def score(text,gold):
    low=text.lower()
    tk=gold["tickets"]; fl={f.lower() for f in gold["files"]}
    return (sorted(t for t in tk if t.lower() in low),
            sorted(t for t in tk if t.lower() not in low),
            sorted(f for f in fl if f in low or f.split("/")[-1] in low),
            sorted(set(TICKET_RE.findall(text))-tk))

def main():
    S=_load_window(60)
    idx,V=cluster_members(S,SEED,0.35)
    pack,gold=evidence_pack(S,idx)
    reps=pick_reps(idx,V,4)
    print(f"MODEL={TAG}  cluster={len(idx)} sessions  gold_tickets={sorted(gold['tickets'])} files={len(gold['files'])}\n")

    # A REDUCED
    a_parts=["=== EVIDENCE PACK ===",pack,"",f"=== 4 REPRESENTATIVES (of {len(idx)}) ==="]
    for r in reps: a_parts.append(f"\n[{S[r].app} {S[r].duration_s}s]\n"+re.sub(r'\s+',' ',S[r].raw_body)[:2500])
    A=gen(WORKLOG.format(blob="\n".join(a_parts)),4096)

    # B FULL — evidence pack + ALL sessions raw
    b_parts=["=== EVIDENCE PACK ===",pack,"",f"=== ALL {len(idx)} SESSIONS ==="]
    for r in sorted(idx,key=lambda i:S[i].ts): b_parts.append(f"\n[{S[r].app} {S[r].duration_s}s]\n"+re.sub(r'\s+',' ',S[r].raw_body))
    full="\n".join(b_parts)
    B=gen(WORKLOG.format(blob=full),32768)

    # C MAPREDUCE — chunks of 5 sessions, then combine
    order=sorted(idx,key=lambda i:S[i].ts); chunks=[order[i:i+5] for i in range(0,len(order),5)]
    part_sums=[]; c_in=0; c_out=0; c_sec=0.0; c_ram=0; c_cpu=0; c_calls=0
    for ch in chunks:
        cb="\n".join(f"[{S[r].app} {S[r].duration_s}s] "+re.sub(r'\s+',' ',S[r].raw_body)[:3000] for r in ch)
        rsum=gen(CHUNK_SUM.format(blob=cb),8192)
        part_sums.append(rsum["text"]); c_in+=rsum["in_tok"] or 0; c_out+=rsum["out_tok"] or 0
        c_sec+=rsum["sec"]; c_ram=max(c_ram,rsum["ram_gb"] or 0); c_cpu=max(c_cpu,rsum["cpu_pct"]); c_calls+=1
    fin=gen(COMBINE.format(pack=pack,parts="\n".join(f"- {p}" for p in part_sums)),8192)
    c_in+=fin["in_tok"] or 0; c_out+=fin["out_tok"] or 0; c_sec+=fin["sec"]
    c_ram=max(c_ram,fin["ram_gb"] or 0); c_cpu=max(c_cpu,fin["cpu_pct"]); c_calls+=1
    C={"text":fin["text"],"in_tok":c_in,"out_tok":c_out,"sec":round(c_sec,1),"ram_gb":c_ram,
       "cpu_pct":c_cpu,"calls":c_calls}

    for nm,res in [("A REDUCED (1 call)",A),("B FULL give-all (1 call)",B),
                   (f"C MAPREDUCE ({C['calls']} calls)",C)]:
        th,tm,fh,hl=score(res["text"],gold)
        print(f"{'='*72}\n## {nm}")
        print(f"  input_tok={res['in_tok']}  output_tok={res['out_tok']}  wall={res['sec']}s  "
              f"model_RAM={res.get('ram_gb')}GB  peakCPU={res.get('cpu_pct')}%"+(f"  rss={res.get('rss_gb')}GB" if 'rss_gb' in res else ""))
        print(f"  tickets HIT={th or '-'} MISSED={tm or '-'}  files={len(fh)}/{len(gold['files'])}  HALLUC={hl or 'none'}")
        print(f"  --- summary ---\n  {res['text']}\n")

if __name__=="__main__":
    main()
