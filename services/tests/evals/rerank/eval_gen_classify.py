"""Generative-classifier benchmark on the week dataset (101 Claude Code sessions).
Each generative SLM directly PICKS one candidate ticket or NONE (its own abstain),
unlike the rerankers (which score pairs + threshold). Identical inputs via
plans.load_week(). Scored like plans.report: accuracy, confident-accuracy,
false-binds, recall-misses, per session_type. One model at a time (stop between).

Usage: python eval_gen_classify.py [model1,model2,...]
"""
import json, re, time, subprocess, sys, urllib.request
from plans import load_week

OLLAMA = "http://127.0.0.1:11434"
MODELS = (sys.argv[1].split(",") if len(sys.argv) > 1 else
          ["qwen3:0.6b","qwen3:1.7b","llama3.2:3b","granite3.3:2b","phi4-mini","qwen3.5:4b","smollm2:1.7b"])
KAN_RE = re.compile(r"\bKAN-\d{2,4}\b")

PROMPT = """A developer completed one coding session. Decide which ONE ticket (if any) this session advances.

Session summary:
{summary}

Candidate tickets:
{cands}

Rules:
- Pick the single ticket the session most directly advances.
- If it does not clearly match ANY candidate (unrelated work, admin/overhead, or untracked work with no matching ticket), answer NONE.
- Answer with ONLY the ticket key (e.g. {example}) or NONE. No explanation.

Answer:"""

def ps_gb():
    try:
        with urllib.request.urlopen(OLLAMA+"/api/ps",timeout=10) as r: d=json.loads(r.read())
        return round(max([m.get("size",0) for m in d.get("models",[])] or [0])/1e9,2)
    except Exception: return None

def ask(tag, prompt):
    opts={"temperature":0.0,"num_ctx":4096,"num_predict":60}
    body={"model":tag,"prompt":prompt,"stream":False,"options":opts,"keep_alive":"120s"}
    if tag.startswith("qwen3"): body["think"]=False
    req=urllib.request.Request(OLLAMA+"/api/generate",data=json.dumps(body).encode(),
                               headers={"Content-Type":"application/json"})
    with urllib.request.urlopen(req,timeout=300) as r: out=json.loads(r.read())
    return re.sub(r"<think>.*?</think>","",out.get("response",""),flags=re.S).strip()

def parse(text, cands):
    """Return chosen candidate key or 'NONE'. First candidate mentioned wins; else NONE."""
    found = KAN_RE.findall(text)
    for k in found:
        if k in cands: return k
    if re.search(r"\bNONE\b", text, re.I): return "NONE"
    return "NONE"

def run_model(tag, plans, docs):
    subprocess.run(["ollama","stop",tag],capture_output=True)
    t0=time.time(); preds=[]
    for p in plans:
        cands=p["candidates"]
        cand_str="\n".join(f"- {k}: {docs.get(k,'')[:280]}" for k in cands)
        txt=ask(tag, PROMPT.format(summary=p["summary"][:1500], cands=cand_str, example=cands[0]))
        preds.append((p, parse(txt, set(cands))))
    dt=time.time()-t0; ram=ps_gb()
    subprocess.run(["ollama","stop",tag],capture_output=True)
    return preds, dt, ram

def score(preds):
    n=len(preds); hit=fb=rmiss=uh=ut=0; bytype={}
    for p,pred in preds:
        ok = pred in (p["acceptable"] | ({"NONE"} if p["truth"]=="NONE" else set()))
        hit+=ok
        if p["uncertain"]: ut+=1; uh+=ok
        if pred!="NONE" and p["truth"]=="NONE": fb+=1
        if not ok and pred=="NONE" and p["truth"]!="NONE": rmiss+=1
        st=p.get("session_type","?"); bytype.setdefault(st,[0,0]); bytype[st][1]+=1; bytype[st][0]+=ok
    conf = (hit-uh)/(n-ut) if n-ut else 0
    return {"n":n,"acc":hit/n,"hit":hit,"conf":conf,"fb":fb,"rmiss":rmiss,"bytype":bytype}

def main():
    plans, docs = load_week()
    print(f"WEEK dataset: {len(plans)} sessions  (reranker bench: Qwen3-Reranker-0.6B = 94%, 0 false-binds)\n")
    rows=[]
    for tag in MODELS:
        try: preds, dt, ram = run_model(tag, plans, docs)
        except Exception as e: print(f"!! {tag}: {e}"); continue
        sc=score(preds); rows.append((tag,sc,dt,ram))
        bt=" ".join(f"{k}={v[0]}/{v[1]}" for k,v in sorted(sc["bytype"].items()))
        print(f"{'='*72}\n## {tag}  ({dt:.0f}s, RAM={ram}GB)")
        print(f"   ACCURACY {sc['hit']}/{sc['n']} = {sc['acc']:.0%}   confident={sc['conf']:.0%}   "
              f"false-binds={sc['fb']}   recall-misses={sc['rmiss']}")
        print(f"   by type: {bt}")
    print(f"\n\n{'#'*72}\nSCOREBOARD (week, {len(plans)} sessions)")
    print(f"{'model':15}{'acc':>6}{'conf':>6}{'FB':>4}{'Rmiss':>7}{'sec':>6}{'RAM':>7}")
    print(f"{'Qwen3-Rerank-0.6B':15}{'94%':>6}{'-':>6}{'0':>4}{'-':>7}{'-':>6}{'1.5GB':>7}  <- reranker bench")
    for tag,sc,dt,ram in sorted(rows,key=lambda r:-r[1]['acc']):
        print(f"{tag:15}{sc['acc']:>6.0%}{sc['conf']:>6.0%}{sc['fb']:>4}{sc['rmiss']:>7}{dt:>6.0f}{(str(ram)+'GB'):>7}")

if __name__=="__main__": main()
