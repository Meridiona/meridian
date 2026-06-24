"""Cluster the most recent N minutes of sessions and print what got grouped.

Usage: python dump_window.py [minutes] [threshold]
"""
import sys, sqlite3, json, re, numpy as np
import clib, embed

MIN = int(sys.argv[1]) if len(sys.argv) > 1 else 60
THR = float(sys.argv[2]) if len(sys.argv) > 2 else 0.35

con = sqlite3.connect(clib.DB)
# pull a generous recent slice, then filter by real epoch time in Python
q = f"""SELECT id, app_name, started_at, duration_s, window_titles, session_text, task_key
        FROM app_sessions
        WHERE app_name NOT IN ({','.join('?'*3)})
          AND started_at >= date('now','-2 days')
          AND duration_s >= 8 AND session_text IS NOT NULL AND LENGTH(session_text) > 40
        ORDER BY started_at"""
allrows = con.execute(q, clib.EXCLUDE_APPS).fetchall()
con.close()
maxt = max(clib._epoch(r[2]) for r in allrows)
cutoff = maxt - MIN * 60
rows = [r for r in allrows if clib._epoch(r[2]) >= cutoff]

sessions = []
for sid, app, started, dur, wt, txt, tk in rows:
    try:
        titles = " ".join(d.get("window_name", "") for d in json.loads(wt or "[]"))
    except Exception:
        titles = ""
    body = clib.TS_RE.sub("", txt or "")
    s = clib.Session(id=sid, app=app, started_at=started, ts=clib._epoch(started),
                     duration_s=dur, title_text=titles, raw_body=body, task_key=tk)
    s.entities = clib.extract_entities(titles + " " + body)
    sessions.append(s)

span_lbl = f"{rows[0][2][11:16]}–{rows[-1][2][11:16]} UTC" if rows else "—"
print(f"window: last {MIN} min ({span_lbl})  |  {len(sessions)} sessions\n")
if len(sessions) < 2:
    print("not enough sessions to cluster"); sys.exit()

clib.prepare_content(sessions, strip=False)
vecs = embed.embed("qwen3-0.6b", [s.content for s in sessions])
cos = np.clip(vecs @ vecs.T, -1, 1)
S = clib.combine_affinity(cos, sessions, {"time_mode": "gate", "tau_min": 120, "ent_boost": 0.0})
labels = clib.cluster_agglomerative(S, THR)

from collections import defaultdict, Counter
groups = defaultdict(list)
for s, l in zip(sessions, labels):
    groups[l].append(s)
multi = sorted([g for g in groups.values() if len(g) > 1], key=lambda g: min(x.ts for x in g))
singles = [g[0] for g in groups.values() if len(g) == 1]

print(f"=> {len(groups)} groups: {len(multi)} multi-session buckets, {len(singles)} singletons\n")
for i, g in enumerate(multi, 1):
    span = (max(s.ts for s in g) - min(s.ts for s in g)) / 60
    apps = Counter(s.app for s in g)
    tks = Counter(s.task_key for s in g if s.task_key)
    ents = Counter(t for s in g for t in s.entities["tickets"])
    print(f"━━ BUCKET {i}  ({len(g)} sessions, {span:.0f} min)  apps={dict(apps)}")
    print(f"   9B task_keys={dict(tks) or '—'}   ticket mentions={dict(ents.most_common(4)) or '—'}")
    for s in sorted(g, key=lambda x: x.ts):
        title = re.sub(r"\s+", " ", s.title_text)[:70]
        snip = re.sub(r"\s+", " ", s.clean_body)[:70]
        print(f"     [{s.started_at[11:16]}] {s.app:13.13} {s.duration_s:4d}s  {title}")
    print()

if singles:
    print("── singletons (not grouped):")
    for s in sorted(singles, key=lambda x: x.ts):
        title = re.sub(r"\s+", " ", s.title_text)[:60]
        print(f"     [{s.started_at[11:16]}] {s.app:13.13} {s.duration_s:4d}s  {title}")
