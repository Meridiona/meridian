"""Build the reduced summariser input for a cluster: an evidence pack (exact
facts via regex over ALL sessions) + N representative sessions (medoid + most
divergent, chosen from the embedding vectors), each capped. Framework-neutral.

Usage: python reduce.py [seed_id] [min] [thr] [n_reps] [cap]  -> prints blob
"""
import sqlite3, re, sys, json, numpy as np
from collections import Counter
import datetime as _dt
import clib, embed

def _load_window(minutes):
    con = sqlite3.connect(clib.DB)
    rows = con.execute(
        """SELECT id,app_name,started_at,duration_s,window_titles,session_text,task_key,task_session_type
           FROM app_sessions WHERE app_name NOT IN ('Claude Code','Codex','GitHub Copilot')
             AND started_at >= date('now','-2 days') AND duration_s>=8
             AND session_text IS NOT NULL AND LENGTH(session_text)>40 ORDER BY started_at""").fetchall()
    con.close()
    maxt = max(clib._epoch(r[2]) for r in rows); cut = maxt - minutes*60
    rows = [r for r in rows if clib._epoch(r[2]) >= cut]
    S = []
    for sid, app, started, dur, wt, txt, tk, tt in rows:
        try: titles = " ".join(d.get("window_name", "") for d in json.loads(wt or "[]"))
        except Exception: titles = ""
        s = clib.Session(sid, app, started, clib._epoch(started), dur, titles,
                         clib.TS_RE.sub("", txt or ""), tk)
        s.entities = clib.extract_entities(titles + " " + s.raw_body)
        S.append(s)
    return S

def cluster_members(S, seed_id, thr):
    clib.prepare_content(S, strip=False)
    V = embed.embed("qwen3-0.6b", [s.content for s in S])
    cos = np.clip(V @ V.T, -1, 1)
    A = clib.combine_affinity(cos, S, {"time_mode": "gate", "tau_min": 120, "ent_boost": 0.0})
    labels = clib.cluster_agglomerative(A, thr)
    si = next(i for i, s in enumerate(S) if s.id == seed_id)
    idx = [i for i, l in enumerate(labels) if l == labels[si]]
    return idx, V

def pick_reps(idx, V, n):
    sub = V[idx]; ctr = sub.mean(0); ctr /= np.linalg.norm(ctr)
    sim = sub @ ctr
    picked = [int(np.argmax(sim))]
    while len(picked) < min(n, len(idx)):
        best, bestv = None, 2.0
        for j in range(len(idx)):
            if j in picked: continue
            mx = max(float(sub[j] @ sub[k]) for k in picked)
            if mx < bestv: bestv, best = mx, j
        picked.append(best)
    return [idx[j] for j in picked]

def evidence_pack(S, idx):
    tickets, paths, cmds, domains, titles, apps = (Counter() for _ in range(6))
    tmin = tmax = None; secs = 0
    for i in idx:
        s = S[i]; apps[s.app] += 1; secs += s.duration_s
        tmin = s.ts if tmin is None else min(tmin, s.ts)
        tmax = s.ts if tmax is None else max(tmax, s.ts)
        for t in s.entities.get("tickets", []): tickets[t] += 1
        for p in s.entities.get("paths", []): paths[p] += 1
        for c in s.entities.get("cmds", []): cmds[c] += 1
        for d in s.entities.get("domains", []): domains[d] += 1
        for w in s.title_text.split("  "):
            w = w.strip()
            if w: titles[w] += 1
    f = lambda e: _dt.datetime.utcfromtimestamp(e).strftime("%H:%M")
    lines = [
        f"span: {f(tmin)}-{f(tmax)} ({(tmax-tmin)//60} min, {len(idx)} sessions, {secs//60} min active)",
        f"apps: {dict(apps)}",
        f"tickets: {[t for t,_ in tickets.most_common()]}",
        f"files: {[p for p,_ in paths.most_common()]}",
        f"commands: {[c for c,_ in cmds.most_common()]}",
        f"domains: {[d for d,_ in domains.most_common()]}",
    ]
    return "\n".join(lines), {"tickets": set(tickets), "files": set(paths)}

def build_blob(seed_id, minutes=60, thr=0.35, n_reps=4, cap=2500):
    S = _load_window(minutes)
    idx, V = cluster_members(S, seed_id, thr)
    reps = pick_reps(idx, V, n_reps)
    pack, gold = evidence_pack(S, idx)
    parts = ["=== EVIDENCE PACK (exact facts) ===", pack, "",
             f"=== {len(reps)} REPRESENTATIVE SCREEN EXCERPTS (of {len(idx)} sessions) ==="]
    for r in reps:
        s = S[r]
        body = re.sub(r"\s+", " ", s.raw_body)[:cap]
        parts.append(f"\n[{s.app} · {s.duration_s}s]\n{body}")
    return "\n".join(parts), gold, len(idx)

if __name__ == "__main__":
    seed = int(sys.argv[1]) if len(sys.argv) > 1 else 37166
    mins = int(sys.argv[2]) if len(sys.argv) > 2 else 60
    thr = float(sys.argv[3]) if len(sys.argv) > 3 else 0.35
    n = int(sys.argv[4]) if len(sys.argv) > 4 else 4
    cap = int(sys.argv[5]) if len(sys.argv) > 5 else 2500
    blob, gold, nmem = build_blob(seed, mins, thr, n, cap)
    print(blob)
    print(f"\n--- blob: {len(blob)} chars (~{len(blob)//4} tok) · cluster={nmem} sessions · gold={gold}", file=sys.stderr)
