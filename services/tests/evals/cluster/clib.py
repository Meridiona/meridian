"""Offline session-clustering experiment library.

Goal (Stage A only): given app_sessions.session_text + start time, group sessions
that represent the SAME unit of work, so a downstream stage can bind a whole bucket
to a PM ticket (or spawn a new task) once instead of classifying every session.

Pure read-only against ~/.meridian/meridian.db. No production impact.

Design notes (grounded in the real data, see conversation):
  - OCR text is garbled + heavy per-app UI boilerplate (tab bars, panels, TUI chrome)
    that recurs across UNRELATED sessions -> must strip before embedding or clusters
    collapse into "all DBeaver" / "all Chrome".
  - Clean high-signal tokens (ticket keys KAN-\\d+, file paths, domains, commands)
    survive OCR -> used as a lexical channel / BM25 booster.
  - Conservative merging: a high distance threshold keeps uncertain sessions as
    singletons (== today's per-session behaviour, so no regression).
"""
from __future__ import annotations
import os, re, sqlite3, json, hashlib, time
from dataclasses import dataclass, field
from collections import Counter, defaultdict
import numpy as np

DB = os.path.expanduser(os.environ.get("MERIDIAN_DB", "~/.meridian/meridian.db"))
CACHE = os.path.join(os.path.dirname(__file__), "cache")
EXCLUDE_APPS = ("Claude Code", "Codex", "GitHub Copilot")  # self-summarising CLI agents

TICKET_RE = re.compile(r"\b[A-Z]{2,5}-\d{1,6}\b")
PATH_RE = re.compile(r"[\w./~-]*?[\w-]+\.(?:rs|ts|tsx|py|sql|json|md|toml|js|sh|lock|yaml|yml|txt|nix)\b")
DOMAIN_RE = re.compile(r"\b(?:[a-z0-9-]+\.)+(?:com|net|org|io|ai|dev|sh|app)\b")
CMD_RE = re.compile(r"\b(?:cargo|git|npm|pnpm|python|python3|sqlite3|curl|brew|bash|cd|grep|jq|docker|psql)\b")
WORD_RE = re.compile(r"[A-Za-z0-9_./-]+")
TS_RE = re.compile(r"^\[\d\d:\d\d:\d\d\]\s*", re.M)


@dataclass
class Session:
    id: int
    app: str
    started_at: str
    ts: float            # epoch seconds
    duration_s: int
    title_text: str
    raw_body: str
    task_key: str | None
    entities: dict = field(default_factory=dict)
    clean_body: str = ""
    content: str = ""    # what actually gets embedded


def _epoch(iso: str) -> float:
    # ISO8601 UTC like 2026-06-22T13:42:53Z or with fractional secs
    iso = iso.replace("Z", "")
    for fmt in ("%Y-%m-%dT%H:%M:%S.%f", "%Y-%m-%dT%H:%M:%S"):
        try:
            return time.mktime(time.strptime(iso[:26], fmt))
        except ValueError:
            continue
    return 0.0


def load_sessions(days: int = 10, min_dur: int = 15, min_len: int = 50) -> list[Session]:
    con = sqlite3.connect(DB)
    q = f"""
        SELECT id, app_name, started_at, duration_s, window_titles, session_text, task_key
        FROM app_sessions
        WHERE app_name NOT IN ({','.join('?'*len(EXCLUDE_APPS))})
          AND started_at >= date('now', ?)
          AND duration_s >= ?
          AND session_text IS NOT NULL AND LENGTH(session_text) > ?
        ORDER BY started_at
    """
    rows = con.execute(q, (*EXCLUDE_APPS, f"-{days} days", min_dur, min_len)).fetchall()
    con.close()
    out = []
    for sid, app, started, dur, wt, txt, tk in rows:
        titles = []
        try:
            titles = [d.get("window_name", "") for d in json.loads(wt or "[]")]
        except Exception:
            pass
        title_text = " ".join(t for t in titles if t)
        body = TS_RE.sub("", txt or "")
        s = Session(id=sid, app=app, started_at=started, ts=_epoch(started),
                    duration_s=dur, title_text=title_text, raw_body=body, task_key=tk)
        s.entities = extract_entities(title_text + " " + body)
        out.append(s)
    return out


def extract_entities(text: str) -> dict:
    return {
        "tickets": sorted(set(TICKET_RE.findall(text))),
        "paths": sorted(set(m.lower() for m in PATH_RE.findall(text)))[:40],
        "domains": sorted(set(DOMAIN_RE.findall(text)))[:20],
        "cmds": sorted(set(CMD_RE.findall(text))),
    }


# ---------------- boilerplate stripping (per-app word-shingle DF) ----------------
def _shingles(words: list[str], n: int) -> set[str]:
    return {" ".join(words[i:i+n]) for i in range(len(words) - n + 1)}


def build_boilerplate(sessions: list[Session], n: int = 6, df_frac: float = 0.30,
                      min_app_sessions: int = 5) -> dict[str, set[str]]:
    """Per app, find word n-grams present in > df_frac of that app's sessions = chrome/boilerplate."""
    by_app: dict[str, list[list[str]]] = defaultdict(list)
    for s in sessions:
        by_app[s.app].append([w.lower() for w in WORD_RE.findall(s.raw_body)])
    bp: dict[str, set[str]] = {}
    for app, docs in by_app.items():
        if len(docs) < min_app_sessions:
            bp[app] = set()
            continue
        df = Counter()
        for words in docs:
            for sh in _shingles(words, n):
                df[sh] += 1
        thr = max(2, int(df_frac * len(docs)))
        bp[app] = {sh for sh, c in df.items() if c >= thr}
    return bp


def strip_boilerplate(s: Session, bp: dict[str, set[str]], n: int = 6) -> str:
    words = [w for w in WORD_RE.findall(s.raw_body)]
    low = [w.lower() for w in words]
    boiler = bp.get(s.app, set())
    if not boiler:
        return s.raw_body
    drop = [False] * len(words)
    for i in range(len(low) - n + 1):
        sh = " ".join(low[i:i+n])
        if sh in boiler:
            for j in range(i, i + n):
                drop[j] = True
    kept = [w for w, d in zip(words, drop) if not d]
    return " ".join(kept)


def prepare_content(sessions: list[Session], strip: bool = True,
                    n: int = 6, df_frac: float = 0.30) -> None:
    bp = build_boilerplate(sessions, n=n, df_frac=df_frac) if strip else {}
    for s in sessions:
        s.clean_body = strip_boilerplate(s, bp, n=n) if strip else s.raw_body
        ent = s.entities
        ent_str = " ".join(ent["tickets"] * 3 + ent["paths"] + ent["domains"] + ent["cmds"])
        # title (high signal) + entities (repeated for weight) + cleaned body, capped
        s.content = f"{s.title_text} . {ent_str} . {s.clean_body}"[:8000]


# ---------------- affinity + clustering ----------------
def time_kernel(sessions: list[Session], tau_min: float) -> np.ndarray:
    t = np.array([s.ts for s in sessions])
    dt = np.abs(t[:, None] - t[None, :]) / 60.0  # minutes
    return np.exp(-dt / tau_min)


def jaccard_entity(sessions: list[Session]) -> np.ndarray:
    sets = []
    for s in sessions:
        e = set(s.entities["tickets"]) | set("p:" + p for p in s.entities["paths"]) \
            | set("d:" + d for d in s.entities["domains"])
        sets.append(e)
    n = len(sets)
    M = np.zeros((n, n))
    for i in range(n):
        for j in range(i, n):
            a, b = sets[i], sets[j]
            if not a and not b:
                v = 0.0
            else:
                inter = len(a & b); uni = len(a | b)
                v = inter / uni if uni else 0.0
            M[i, j] = M[j, i] = v
    return M


def combine_affinity(cos: np.ndarray, sessions: list[Session], cfg: dict) -> np.ndarray:
    S = cos.copy()
    mode = cfg.get("time_mode", "gate")
    tau = cfg.get("tau_min", 30.0)
    if mode != "none":
        K = time_kernel(sessions, tau)
        if mode == "gate":          # multiplicative: must be close in BOTH
            S = S * K
        elif mode == "mix":
            a = cfg.get("time_alpha", 0.3)
            S = (1 - a) * S + a * K
    if cfg.get("ent_boost", 0.0) > 0:
        S = S + cfg["ent_boost"] * jaccard_entity(sessions)
    return np.clip(S, 0.0, None)


def cluster_agglomerative(S: np.ndarray, threshold: float):
    from sklearn.cluster import AgglomerativeClustering
    D = 1.0 - S
    np.fill_diagonal(D, 0.0)
    D[D < 0] = 0.0
    m = AgglomerativeClustering(n_clusters=None, metric="precomputed",
                                linkage="average", distance_threshold=threshold)
    return m.fit_predict(D)


def cluster_hdbscan(S: np.ndarray, min_cluster_size: int = 2):
    import hdbscan
    D = (1.0 - S).astype("float64")
    np.fill_diagonal(D, 0.0); D[D < 0] = 0.0
    m = hdbscan.HDBSCAN(metric="precomputed", min_cluster_size=min_cluster_size,
                        min_samples=1, cluster_selection_epsilon=0.0)
    return m.fit_predict(D)


# ---------------- evaluation ----------------
def evaluate(sessions: list[Session], labels: np.ndarray) -> dict:
    from sklearn.metrics import (homogeneity_score, completeness_score,
                                 v_measure_score, adjusted_rand_score)
    idx = [i for i, s in enumerate(sessions) if s.task_key]
    gt = [sessions[i].task_key for i in idx]
    pr = [labels[i] for i in idx]
    n_clusters = len(set(labels))
    sizes = Counter(labels)
    singletons = sum(1 for _, c in sizes.items() if c == 1)
    res = {
        "n_sessions": len(sessions),
        "n_clusters": n_clusters,
        "singletons": singletons,
        "largest": max(sizes.values()),
        "labeled": len(idx),
    }
    # consolidation: how much real grouping happened (the efficiency win)
    multi = [c for c, n in sizes.items() if n > 1]
    grouped = sum(sizes[c] for c in multi)
    res["grouped_frac"] = round(grouped / len(sessions), 3)
    res["mean_multi"] = round(np.mean([sizes[c] for c in multi]), 2) if multi else 0.0
    if len(set(gt)) > 1:
        # pairwise precision/recall over labeled sessions — the regression-safety numbers
        g = np.array([hash(x) for x in gt]); p = np.array(pr)
        same_task = g[:, None] == g[None, :]
        same_clu = p[:, None] == p[None, :]
        tri = np.triu(np.ones_like(same_task), k=1).astype(bool)
        st = same_task & tri; sc = same_clu & tri
        tp = int((st & sc).sum()); merged = int(sc.sum())
        # recall denominator restricted to time-local same-task pairs (worklog-relevant)
        tl = np.array([sessions[i].ts for i in idx])
        close = (np.abs(tl[:, None] - tl[None, :]) / 60.0 <= 120.0)
        st_close = st & close
        truth = int(st_close.sum()); tp_close = int((st_close & sc).sum())
        res.update({
            "pair_prec": round(tp / merged, 3) if merged else 1.0,   # merged pairs that truly match
            "pair_rec": round(tp_close / truth, 3) if truth else 0.0,  # local same-task pairs captured
            "homogeneity": round(homogeneity_score(gt, pr), 3),
            "completeness": round(completeness_score(gt, pr), 3),
            "v_measure": round(v_measure_score(gt, pr), 3),
            "ari": round(adjusted_rand_score(gt, pr), 3),
        })
    return res


def cluster_purity_report(sessions: list[Session], labels: np.ndarray, top: int = 12) -> str:
    groups = defaultdict(list)
    for s, l in zip(sessions, labels):
        groups[l].append(s)
    out = []
    multi = sorted([g for g in groups.values() if len(g) > 1],
                   key=lambda g: -len(g))[:top]
    for g in multi:
        tks = Counter(s.task_key for s in g if s.task_key)
        apps = Counter(s.app for s in g)
        tickets = Counter(t for s in g for t in s.entities["tickets"])
        span = (max(s.ts for s in g) - min(s.ts for s in g)) / 60.0
        out.append(f"\n● cluster n={len(g)} span={span:.0f}min apps={dict(apps)} "
                   f"task_keys={dict(tks) or '-'} ent_tickets={dict(tickets.most_common(4)) or '-'}")
        for s in sorted(g, key=lambda x: x.ts)[:6]:
            snip = re.sub(r"\s+", " ", s.clean_body)[:90]
            out.append(f"    [{s.started_at[11:16]}] {s.app:14.14} tk={s.task_key or '-':8.8} {snip}")
    return "\n".join(out)
