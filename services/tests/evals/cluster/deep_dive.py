"""Deep-dive into the last-N-min clusters: cohesion, separation, distinctive terms,
per-session attachment strength. Usage: python deep_dive.py [minutes] [threshold]
"""
import sys, sqlite3, json, re, numpy as np
import clib, embed
from collections import defaultdict, Counter
from sklearn.feature_extraction.text import TfidfVectorizer

MIN = int(sys.argv[1]) if len(sys.argv) > 1 else 60
THR = float(sys.argv[2]) if len(sys.argv) > 2 else 0.35

con = sqlite3.connect(clib.DB)
allrows = con.execute(
    f"""SELECT id,app_name,started_at,duration_s,window_titles,session_text,task_key
        FROM app_sessions WHERE app_name NOT IN ('Claude Code','Codex','GitHub Copilot')
          AND started_at >= date('now','-2 days') AND duration_s>=8
          AND session_text IS NOT NULL AND LENGTH(session_text)>40 ORDER BY started_at""").fetchall()
con.close()
maxt = max(clib._epoch(r[2]) for r in allrows); cut = maxt - MIN*60
rows = [r for r in allrows if clib._epoch(r[2]) >= cut]

S = []
for sid, app, started, dur, wt, txt, tk in rows:
    try: titles = " ".join(d.get("window_name", "") for d in json.loads(wt or "[]"))
    except Exception: titles = ""
    s = clib.Session(sid, app, started, clib._epoch(started), dur, titles,
                     clib.TS_RE.sub("", txt or ""), tk)
    s.entities = clib.extract_entities(titles + " " + s.raw_body)
    S.append(s)
clib.prepare_content(S, strip=False)

vecs = embed.embed("qwen3-0.6b", [s.content for s in S])
cos = np.clip(vecs @ vecs.T, -1, 1)
A = clib.combine_affinity(cos, S, {"time_mode": "gate", "tau_min": 120, "ent_boost": 0.0})
labels = clib.cluster_agglomerative(A, THR)

# centroids in embedding space
groups = defaultdict(list)
for i, l in enumerate(labels): groups[l].append(i)
cents = {l: vecs[idx].mean(0) for l, idx in groups.items()}
cents = {l: c/ (np.linalg.norm(c)+1e-9) for l, c in cents.items()}

# distinctive terms per cluster (tf-idf, cluster-doc = concat member bodies)
docs = {l: " ".join(re.sub(r"[^A-Za-z0-9_./-]", " ", S[i].clean_body) for i in idx) for l, idx in groups.items()}
tf = TfidfVectorizer(lowercase=True, ngram_range=(1, 2), min_df=1, max_df=0.7, max_features=4000,
                     stop_words="english")
ls = sorted(docs); X = tf.fit_transform([docs[l] for l in ls]); vocab = np.array(tf.get_feature_names_out())
top_terms = {}
for r_, l in enumerate(ls):
    row = X[r_].toarray()[0]; top = row.argsort()[::-1][:8]
    top_terms[l] = [vocab[j] for j in top if row[j] > 0]

multi = sorted([l for l, idx in groups.items() if len(idx) > 1], key=lambda l: min(S[i].ts for i in groups[l]))
print(f"last {MIN}min: {len(S)} sessions, {len(groups)} clusters ({len(multi)} multi)\n")
for n, l in enumerate(multi, 1):
    idx = groups[l]
    sub = cos[np.ix_(idx, idx)]
    cohesion = (sub.sum() - len(idx)) / (len(idx)*(len(idx)-1))   # mean intra pairwise cosine
    # nearest other cluster by centroid sim
    others = [(float(cents[l] @ cents[o]), o) for o in cents if o != l]
    near_sim, near = max(others) if others else (0, None)
    span = (max(S[i].ts for i in idx) - min(S[i].ts for i in idx))/60
    apps = Counter(S[i].app for i in idx); tks = Counter(S[i].task_key for i in idx if S[i].task_key)
    print(f"━━━━ CLUSTER {n}  (n={len(idx)}, {span:.0f}min) ━━━━")
    print(f"  cohesion(mean intra-cosine)={cohesion:.2f}   nearest-other-cluster sim={near_sim:.2f}")
    print(f"  apps={dict(apps)}  9B_task_keys={dict(tks) or '—'}")
    print(f"  distinctive terms: {', '.join(top_terms[l][:8])}")
    for i in sorted(idx, key=lambda i: S[i].ts):
        own = float(vecs[i] @ cents[l])
        # best other centroid
        oth = max((float(vecs[i] @ cents[o]) for o in cents if o != l), default=0)
        margin = own - oth
        flag = "  <-- weak/borderline" if margin < 0.03 else ""
        snip = re.sub(r"\s+", " ", S[i].clean_body)[:95]
        print(f"    [{S[i].started_at[11:16]}] {S[i].app:11.11} own={own:.2f} Δ={margin:+.2f}{flag}")
        print(f"         {snip}")
    print()
