"""Re-run last-hour clustering with two fixes vs the old prep, side by side:
  FIX 1: no OCR-extracted ticket injection into embedded text.
  FIX 2: strip cross-session boilerplate (tab strips, panels) via global word-shingle
         document-frequency computed over a background corpus (last 3 days, same apps).
Reports each cluster and specifically tracks google session 37088.
Usage: python rerun_fixed.py [min] [thr]
"""
import sys, sqlite3, json, re, numpy as np
import clib, embed
from collections import defaultdict, Counter

MIN = int(sys.argv[1]) if len(sys.argv) > 1 else 60
THR = float(sys.argv[2]) if len(sys.argv) > 2 else 0.35
WORD = clib.WORD_RE


def load(days_back_clause, only_window=False, cutoff=None):
    con = sqlite3.connect(clib.DB)
    rows = con.execute(
        f"""SELECT id,app_name,started_at,duration_s,window_titles,session_text,task_key
            FROM app_sessions WHERE app_name NOT IN ('Claude Code','Codex','GitHub Copilot')
              AND started_at >= date('now', ?) AND duration_s>=8
              AND session_text IS NOT NULL AND LENGTH(session_text)>40 ORDER BY started_at""",
        (days_back_clause,)).fetchall()
    con.close()
    out = []
    for sid, app, started, dur, wt, txt, tk in rows:
        try: titles = " ".join(d.get("window_name", "") for d in json.loads(wt or "[]"))
        except Exception: titles = ""
        s = clib.Session(sid, app, started, clib._epoch(started), dur, titles,
                         clib.TS_RE.sub("", txt or ""), tk)
        s.entities = clib.extract_entities(titles + " " + s.raw_body)
        out.append(s)
    return out


# ---- FIX 2: per-app WORD-LEVEL (unigram) DF stoplist — robust to OCR garbling ----
# Exact n-gram shingles fail on garbled OCR (each capture scrambles differently), so we
# drop individual high-DF words instead: tab-strip vocab recurs even when sequence scrambles.
def build_global_boiler(bg, df_frac=0.4, min_n=8):
    by_app = defaultdict(list)
    for s in bg:
        by_app[s.app].append({w.lower() for w in WORD.findall(s.raw_body)})  # set per doc
    boiler = {}
    for app, docsets in by_app.items():
        if len(docsets) < min_n:
            boiler[app] = set(); continue
        df = Counter()
        for ws in docsets:
            for w in ws: df[w] += 1
        thr = max(3, int(df_frac*len(docsets)))
        boiler[app] = {w for w, c in df.items() if c >= thr}
    return boiler


def strip_global(s, boiler, n=4):
    words = WORD.findall(s.raw_body)
    bset = boiler.get(s.app, set())
    if not bset: return s.raw_body
    return " ".join(w for w in words if w.lower() not in bset)


def prep(sessions, boiler, inject_entities, n=4):
    for s in sessions:
        body = strip_global(s, boiler, n) if boiler is not None else s.raw_body
        if inject_entities:
            ent = s.entities
            ent_str = " ".join(ent["tickets"]*3 + ent["paths"] + ent["domains"] + ent["cmds"])
            s.content = f"{s.title_text} . {ent_str} . {body}"[:8000]
        else:
            # title + domains/paths (clean, OCR-robust) but NO ticket keys, NO ×3 boost
            ent = s.entities
            clean_ents = " ".join(ent["domains"] + ent["paths"])
            s.content = f"{s.title_text} . {clean_ents} . {body}"[:8000]


def cluster(sessions, tag_suffix):
    vecs = embed.embed("qwen3-0.6b", [s.content for s in sessions], cache_tag="qwen3-win-" + tag_suffix)
    cos = np.clip(vecs @ vecs.T, -1, 1)
    A = clib.combine_affinity(cos, sessions, {"time_mode": "gate", "tau_min": 120, "ent_boost": 0.0})
    return clib.cluster_agglomerative(A, THR), cos


def report(name, sessions, labels):
    groups = defaultdict(list)
    for i, l in enumerate(labels): groups[l].append(i)
    order = sorted(groups, key=lambda l: min(sessions[i].ts for i in groups[l]))
    multi = [l for l in order if len(groups[l]) > 1]
    print(f"\n########## {name}: {len(groups)} clusters ({len(multi)} multi) ##########")
    for n, l in enumerate(order, 1):
        idx = sorted(groups[l], key=lambda i: sessions[i].ts)
        apps = Counter(sessions[i].app for i in idx)
        tks = Counter(sessions[i].task_key for i in idx if sessions[i].task_key)
        has88 = " <<< 37088 google HERE" if any(sessions[i].id == 37088 for i in idx) else ""
        kind = "singleton" if len(idx) == 1 else f"{len(idx)} sessions"
        print(f"  C{n}: {kind:13} {sessions[idx[0]].started_at[11:16]}-{sessions[idx[-1]].started_at[11:16]} "
              f"apps={dict(apps)} 9B={dict(tks) or '-'}{has88}")


# background corpus for DF, window for clustering
print("loading background corpus (3 days) + window...")
bg = load("-3 days")
win_cut = max(s.ts for s in bg) - MIN*60
window = [s for s in bg if s.ts >= win_cut]
print(f"background={len(bg)} sessions, window(last {MIN}min)={len(window)} sessions")

boiler = build_global_boiler(bg, df_frac=0.4)
nb = sum(len(v) for v in boiler.values())
print(f"boilerplate words: {nb} across {sum(1 for v in boiler.values() if v)} apps")
for app, ws in boiler.items():
    if ws: print(f"   {app:14.14} {len(ws)} words e.g. {sorted(list(ws))[:12]}")
# show how much it strips per app on the window
from statistics import mean
red = defaultdict(list)
for s in window:
    st = strip_global(s, boiler, 4)
    if len(s.raw_body): red[s.app].append(1 - len(st)/len(s.raw_body))
for app, v in sorted(red.items(), key=lambda x: -len(x[1]))[:6]:
    print(f"   {app:14.14} n={len(v):3d} removed={mean(v):.0%}")

# OLD prep (entity injection, no strip)
prep(window, None, inject_entities=True)
old_labels, _ = cluster(window, "old")
report("OLD (entity-inject, no strip)", window, old_labels)

# NEW prep (no ticket inject + boilerplate strip)
prep(window, boiler, inject_entities=False)
new_labels, cos = cluster(window, "fixed")
report("NEW (no-ticket-inject + boilerplate-strip)", window, new_labels)

# what cluster did 37088 land in, and its top neighbors now
idmap = {s.id: i for i, s in enumerate(window)}
if 37088 in idmap:
    g = idmap[37088]
    print(f"\n=== 37088 after FIX: embedded content head ===")
    print("  " + re.sub(r"\s+", " ", window[g].content)[:200])
    sims = sorted([(cos[g, i], window[i]) for i in range(len(window)) if i != g], key=lambda x: -x[0])[:5]
    print("  top neighbors now:")
    for sim, s in sims:
        print("   cos=%.3f id=%d %-12.12s %s" % (sim, s.id, s.app, re.sub(r'\s+', ' ', s.content)[:55]))
