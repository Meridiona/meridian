"""Explain WHY a cluster holds together. Re-runs the last-N-min clustering,
picks the cluster containing a seed id, and prints, for every member:
 - raw content cosine to the cluster centroid
 - nearest in-cluster neighbour (content cosine) + the time gap to it
 - whether content alone (cos>=thr) or time-bridging kept it in.
Usage: python why_cluster.py [seed_id] [min] [thr]
"""
import sys, sqlite3, json, re, numpy as np
import clib, embed
from collections import Counter

SEED = int(sys.argv[1]) if len(sys.argv) > 1 else 37166
MIN = int(sys.argv[2]) if len(sys.argv) > 2 else 60
THR = float(sys.argv[3]) if len(sys.argv) > 3 else 0.35

con = sqlite3.connect(clib.DB)
allrows = con.execute(
    f"""SELECT id,app_name,started_at,duration_s,window_titles,session_text,task_key,task_session_type
        FROM app_sessions WHERE app_name NOT IN ('Claude Code','Codex','GitHub Copilot')
          AND started_at >= date('now','-2 days') AND duration_s>=8
          AND session_text IS NOT NULL AND LENGTH(session_text)>40 ORDER BY started_at""").fetchall()
con.close()
maxt = max(clib._epoch(r[2]) for r in allrows); cut = maxt - MIN*60
rows = [r for r in allrows if clib._epoch(r[2]) >= cut]

S, types = [], {}
for sid, app, started, dur, wt, txt, tk, tt in rows:
    try: titles = " ".join(d.get("window_name", "") for d in json.loads(wt or "[]"))
    except Exception: titles = ""
    s = clib.Session(sid, app, started, clib._epoch(started), dur, titles,
                     clib.TS_RE.sub("", txt or ""), tk)
    s.entities = clib.extract_entities(titles + " " + s.raw_body)
    types[sid] = tt
    S.append(s)
clib.prepare_content(S, strip=False)
vecs = embed.embed("qwen3-0.6b", [s.content for s in S])
cos = np.clip(vecs @ vecs.T, -1, 1)
A = clib.combine_affinity(cos, S, {"time_mode": "gate", "tau_min": 120, "ent_boost": 0.0})
labels = clib.cluster_agglomerative(A, THR)

seed_idx = next(i for i, s in enumerate(S) if s.id == SEED)
lab = labels[seed_idx]
idx = [i for i, l in enumerate(labels) if l == lab]
idx.sort(key=lambda i: S[i].ts)

centroid = vecs[idx].mean(0); centroid /= np.linalg.norm(centroid)
print(f"== CLUSTER containing id={SEED}: {len(idx)} sessions, thr={THR} ==\n")
print(f"{'id':>6} {'time':>5} {'app':12} {'dur':>4}  {'cos→ctr':>7}  {'nn':>6} {'nn_cos':>6} {'gap_min':>7}  in_via")
for i in idx:
    ctr = float(vecs[i] @ centroid)
    others = [j for j in idx if j != i]
    ncos = [(float(vecs[i] @ vecs[j]), j) for j in others]
    nc, nj = max(ncos)
    gap = abs(S[i].ts - S[nj].ts)/60
    via = "content" if nc >= THR else "TIME-bridge"
    print(f"{S[i].id:>6} {S[i].started_at[11:16]} {S[i].app:12.12} {S[i].duration_s:>4} "
          f"{ctr:>7.2f}  {S[nj].id:>6} {nc:>6.2f} {gap:>7.1f}  {via}")

# how many members are within content-thr of >=1 other member, ignoring time
print("\n-- content-only graph (cos>=thr, time ignored) --")
n = len(idx)
adj = [[float(vecs[idx[a]] @ vecs[idx[b]]) >= THR for b in range(n)] for a in range(n)]
# connected components
seen = set(); comps = []
for a in range(n):
    if a in seen: continue
    stack=[a]; comp=[]
    while stack:
        x=stack.pop()
        if x in seen: continue
        seen.add(x); comp.append(x)
        for b in range(n):
            if b not in seen and adj[x][b]: stack.append(b)
    comps.append(comp)
comps.sort(key=len, reverse=True)
print(f"content-only would split this cluster into {len(comps)} component(s): "
      f"{[ [S[idx[c]].id for c in comp] for comp in comps ]}")
