"""Cluster a SPECIFIC UTC time window and deep-dive each cluster.
Usage: python cluster_window.py <YYYY-MM-DD> <start_hour> <end_hour> [thr]
"""
import sys, sqlite3, json, re, numpy as np
import clib, embed
from collections import defaultdict, Counter
import datetime as dt

DAY = sys.argv[1]; H0 = int(sys.argv[2]); H1 = int(sys.argv[3])
THR = float(sys.argv[4]) if len(sys.argv) > 4 else 0.35

con = sqlite3.connect(clib.DB)
allrows = con.execute(
    """SELECT id,app_name,started_at,duration_s,window_titles,session_text,task_key,task_session_type
       FROM app_sessions WHERE app_name NOT IN ('Claude Code','Codex','GitHub Copilot')
         AND date(started_at)=? AND duration_s>=8
         AND session_text IS NOT NULL AND LENGTH(session_text)>40 ORDER BY started_at""", (DAY,)).fetchall()
con.close()

def hr(e): return int(dt.datetime.utcfromtimestamp(e).strftime("%H"))
rows = [r for r in allrows if H0 <= hr(clib._epoch(r[2])) < H1]

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

groups = defaultdict(list)
for i, l in enumerate(labels): groups[l].append(i)
order = sorted(groups, key=lambda l: min(S[i].ts for i in groups[l]))
nsing = sum(1 for l in groups if len(groups[l]) == 1)

print(f"╔══ {DAY} {H0:02d}:00–{H1:02d}:00 UTC | {len(S)} sessions | {len(groups)} clusters ({nsing} singletons) | thr={THR}\n")
for n, l in enumerate(order, 1):
    idx = sorted(groups[l], key=lambda i: S[i].ts)
    apps = Counter(S[i].app for i in idx)
    tks = Counter(S[i].task_key for i in idx if S[i].task_key)
    tts = Counter(types[S[i].id] for i in idx if types[S[i].id])
    secs = sum(S[i].duration_s for i in idx)
    span = (S[idx[-1]].ts - S[idx[0]].ts) / 60
    print(f"╠═ CLUSTER {n}: {len(idx)} sess · {S[idx[0]].started_at[11:16]}–{S[idx[-1]].started_at[11:16]} ({span:.0f}m) · {secs}s")
    print(f"║   apps={dict(apps)}  9B_tk={dict(tks) or '—'}  9B_type={dict(tts) or '—'}")
    for i in idx:
        s = S[i]
        body = re.sub(r"\s+", " ", s.clean_body)[:95]
        ents = ",".join(s.entities["tickets"][:3])
        print(f"║   {s.id:<6}[{s.started_at[11:16]}] {s.app:13.13}{s.duration_s:4d}s tk={s.task_key or '-':8.8} {types[s.id] or '-':9.9} {body}")
        if ents: print(f"║          tickets-on-screen: {ents}")
    print()
