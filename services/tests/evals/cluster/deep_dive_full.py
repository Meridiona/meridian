"""Full per-cluster deep dive of the last N minutes: every app session listed,
with app composition, timing, 9B label, and content. Usage: python deep_dive_full.py [min] [thr]
"""
import sys, sqlite3, json, re, numpy as np
import clib, embed
from collections import defaultdict, Counter

MIN = int(sys.argv[1]) if len(sys.argv) > 1 else 60
THR = float(sys.argv[2]) if len(sys.argv) > 2 else 0.35

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

groups = defaultdict(list)
for i, l in enumerate(labels): groups[i and 0 or 0] if False else groups[l].append(i)
order = sorted(groups, key=lambda l: min(S[i].ts for i in groups[l]))

mins = (max(s.ts for s in S) - min(s.ts for s in S))/60
print(f"╔══ LAST {MIN} MIN  |  {len(S)} app-sessions  |  {len(groups)} CLUSTERS  (threshold={THR})")
print(f"║   actual span {S[0].started_at[11:16]}–{S[-1].started_at[11:16]} ({mins:.0f}min)\n")

for n, l in enumerate(order, 1):
    idx = sorted(groups[l], key=lambda i: S[i].ts)
    apps = Counter(S[i].app for i in idx)
    tks = Counter(S[i].task_key for i in idx if S[i].task_key)
    tts = Counter(types[S[i].id] for i in idx if types[S[i].id])
    secs = sum(S[i].duration_s for i in idx)
    span = (S[idx[-1]].ts - S[idx[0]].ts)/60
    print(f"╠═══════════════════════════════════════════════════════════════════")
    print(f"║ CLUSTER {n}: {len(idx)} sessions · {S[idx[0]].started_at[11:16]}–{S[idx[-1]].started_at[11:16]} ({span:.0f}min) · {secs}s active")
    print(f"║   APPS: {dict(apps)}")
    print(f"║   9B task_key: {dict(tks) or '—'}   |   9B type: {dict(tts) or '—'}")
    print(f"╟───────────────────────────────────────────────────────────────────")
    for i in idx:
        s = S[i]
        title = re.sub(r"\s+", " ", s.title_text)[:62]
        body = re.sub(r"\s+", " ", s.clean_body)[:110]
        ents = ",".join(s.entities["tickets"][:3])
        print(f"║  id={s.id:<6} [{s.started_at[11:16]}] {s.app:13.13} {s.duration_s:4d}s  tk={s.task_key or '-':7.7} {types[s.id] or '-':9.9}")
        print(f"║         title: {title}")
        print(f"║         text : {body}")
        if ents: print(f"║         tickets-on-screen: {ents}")
    print()
