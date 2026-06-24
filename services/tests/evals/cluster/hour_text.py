"""Build a GOOD, information-preserving, noise-reduced "hour session text" from one
hour of app_sessions — WITHOUT any regex signal/ticket backbone. Tickets, files, etc.
survive ONLY because the spans that mention them are high-information and get kept.

Pipeline (grounded in screenpipe selection + SemDeDup/facility-location research):
  1. load non-coding-agent sessions in [hour], duration >= MIN_DUR (15s)
  2. split each session_text into line-spans, attach (app, window, time)
  3. NOISE STRIP: drop harness/UI boilerplate, url markers, ctrl+o junk, tiny/punct lines,
     and lines whose normalised form recurs across many sessions (cross-session DF = chrome)
  4. LEXICAL DEDUP: collapse near-identical normalised lines (kills exact OCR frame repeats)
  5. SEMANTIC DEDUP (SemDeDup): embed survivors, greedily drop cosine>THR near-duplicates
     (kills OCR per-frame variants "Baseline"/"Daselıne" that lexical dedup misses)
  6. SELECT under budget: per window-thread, facility-location max-min pick of diverse spans
  7. STRUCTURE: time-ordered window-threads (app · window · time-span) + selected lines

Usage: python hour_text.py <YYYY-MM-DDTHH> [sem_thr] [budget_tok]
"""
from __future__ import annotations
import sys, os, sqlite3, json, re
from collections import Counter, defaultdict
import datetime as dt
import numpy as np
import clib, embed

EXCLUDE = ("Claude Code", "Codex", "GitHub Copilot", "Cursor Agent")
MIN_DUR = 15
SEM_THR = 0.86
BUDGET_TOK = 4000

# --- boilerplate / junk line filters (harness + UI chrome, NOT content) ---
JUNK_SUBSTR = (
    "<local-command-caveat>", "local-command-stdout", "ctrl+o to expand",
    "ctrl+o to", "lines (ctrl", "auto mode classifier", "Allowed by auto mode",
    "for agents", "shift+tab to cycle", "+ s ifi", "Type to", "tokens · ",
)
URL_RE = re.compile(r"^\[url\]\s*", re.I)
TS_LINE_RE = re.compile(r"^\[\d\d:\d\d:\d\d\]\s*$")
NONWORD_RE = re.compile(r"[^a-z0-9]+")
WORDTOK_RE = re.compile(r"[A-Za-z][A-Za-z']+")
# stopwords = the skeleton of real prose; keyword-salad OCR (youtube/finder tiles) has none
STOP = set("the a an and or but to of in on for with at by from is are was were be been "
           "this that these those it its as into out up down over we i you he she they "
           "have has had do does did not no so if then than when while there here can will "
           "would should could about which who what your my our their let me now".split())
CODEISH_RE = re.compile(r"[/\\.]|::|->|\b(?:def|fn|let|const|import|cargo|git|npm|python|SELECT|FROM|WHERE)\b")
# file-extension tokens — used to detect IDE file-tree "salad" (a bare list of filenames is
# UI chrome, not work narration; a single file mentioned inside prose is real content)
EXT_RE = re.compile(r"\.(?:py|md|sh|json|toml|rs|tsx?|jsx?|lock|txt|ya?ml|cfg|ini|sql|db|env|rb|go|c|h)\b", re.I)
SPINNER_RE = re.compile(r"(?:Photosynthesizing|Thinking|tokens\)|esc to interrupt|↓|✶|✳|⠂|⏵)")
# OCR glues the whole screen (file-tree + tab-bar + status-line + content pane) into ONE
# run-on line. A substring junk match anywhere then nukes the entire blob — including the
# real content. Segment long lines at UI-separator glyphs + multi-space runs so each chrome
# fragment isolates (and dies in hard-junk/DF) while content clauses survive on their own.
SEG_SPLIT_RE = re.compile(r"[•·▸►▶‣⁃◦●○➤➔→⟶»«›‹❯❮|©®®✓✦✱✶✳※❘┃│]+|\s{2,}")
SEG_TRIGGER = 140  # only segment lines longer than this; short prose passes through intact


def segment_line(ln: str):
    """Yield pseudo-lines: long OCR blobs split on UI separators, short lines unchanged."""
    if len(ln) <= SEG_TRIGGER:
        yield ln
        return
    for piece in SEG_SPLIT_RE.split(ln):
        p = piece.strip()
        if p:
            yield p


def norm(line: str) -> str:
    return NONWORD_RE.sub(" ", line.lower()).strip()


def clean_title(w: str) -> str:
    w = re.sub(r"\s+", " ", w or "").strip()
    if "local-command-caveat" in w or w.lower().startswith("terminal"):
        # terminal tab titles are harness noise; keep only "Terminal"
        return "Terminal"
    return w[:50]


def is_hard_junk(line: str) -> bool:
    """Cheap, always-on junk: too short, harness/spinner markers, mostly non-letters."""
    s = line.strip()
    if len(s) < 18:
        return True
    if TS_LINE_RE.match(s):
        return True
    if any(j in s for j in JUNK_SUBSTR):
        return True
    if SPINNER_RE.search(s):
        return True
    letters = sum(c.isalpha() for c in s)
    if letters < 10 or letters / len(s) < 0.45:
        return True
    return False


def fails_prose_gate(line: str) -> bool:
    """Heuristic salience: keep sentences (stopwords) or code/paths; drop keyword salad."""
    s = line.strip()
    toks = [t.lower() for t in WORDTOK_RE.findall(s)]
    if not toks:
        return True
    nstop = sum(t in STOP for t in toks)
    # IDE file-tree salad: >=3 filename tokens with no real sentence skeleton = chrome, drop it
    if len(EXT_RE.findall(s)) >= 3 and nstop < 2:
        return True
    if CODEISH_RE.search(s):
        return False
    return nstop < 2 or nstop / max(len(toks), 1) < 0.08


def is_junk(line: str, prose_gate: bool = True) -> bool:
    if is_hard_junk(line):
        return True
    if prose_gate and fails_prose_gate(line):
        return True
    return False


def load_hour(hour_prefix: str):
    con = sqlite3.connect(clib.DB)
    rows = con.execute(
        f"""SELECT id,app_name,started_at,duration_s,window_titles,session_text
            FROM app_sessions
            WHERE app_name NOT IN ({','.join('?'*len(EXCLUDE))})
              AND duration_s >= ?
              AND session_text IS NOT NULL AND LENGTH(session_text) > 40
              AND started_at LIKE ? ORDER BY started_at""",
        (*EXCLUDE, MIN_DUR, hour_prefix + "%")).fetchall()
    con.close()
    return rows


def build_hour_text(hour: str, sem_thr: float = SEM_THR, budget_tok: int = BUDGET_TOK,
                    select_mode: str = "floc"):
    """select_mode: 'floc' = heuristic prose-gate + length/diversity facility-location.
                    'rerank' = same prose-gate candidate pool, but Qwen3-Reranker scores
                    worklog-salience for the final per-session pick (replaces floc max-min)."""
    prose_gate = True
    rows = load_hour(hour)
    if not rows:
        return "", {"raw_chars": 0, "included_sids": []}
    raw_chars = sum(len(r[5]) for r in rows)

    # ---- 1+2: build spans (+ per-session rescue candidates so no session vanishes) ----
    spans = []           # dict: sid, app, win, t(str), line  (passed the prose gate)
    rescue_by_sid = {}   # sid -> [(t, line)]  light-filtered fallback (skips prose gate)
    meta = {}            # sid -> (app, win, started_hhmm)
    for sid, app, started, dur, wt, txt in rows:
        try:
            wins = json.loads(wt or "[]")
            win = max(wins, key=lambda d: d.get("count", 0)).get("window_name", "") if wins else ""
        except Exception:
            win = ""
        win = clean_title(win)
        meta[sid] = (app, win, started[11:16])
        rescue = []
        cur_t = started[11:16]
        for ln in (txt or "").splitlines():
            m = re.match(r"^\[(\d\d:\d\d:\d\d)\]\s*(.*)", ln)
            if m:
                cur_t = m.group(1)[:5]
                ln = m.group(2)
            ln = URL_RE.sub("", ln).strip()
            if not ln:
                continue
            for seg in segment_line(ln):
                # light gate for rescue: real letters, no harness/spinner junk (keeps OCR-ish lines)
                letters = sum(c.isalpha() for c in seg)
                if (len(seg) >= 18 and letters >= 12 and letters / len(seg) >= 0.5
                        and not any(j in seg for j in JUNK_SUBSTR) and not SPINNER_RE.search(seg)):
                    rescue.append((cur_t, seg))
                if is_junk(seg, prose_gate=prose_gate):
                    continue
                spans.append({"sid": sid, "app": app, "win": win, "t": cur_t, "line": seg})
        rescue_by_sid[sid] = rescue

    n_after_junk = len(spans)

    # ---- 3: cross-session DF boilerplate (a normalised line in many sessions = chrome) ----
    # Named entities (ticket keys, PR numbers, file paths with extensions) are exempt: if you
    # work on one file or PR all hour, every session mentions it — that is the central work
    # artifact, NOT chrome, even though it triggers the DF threshold.
    NAMED_ENTITY_RE = re.compile(
        r'\b[A-Z]{2,10}-\d+\b'          # ticket keys  KAN-123
        r'|(?:PR\s*#?|#)\d{2,5}\b'       # PR / issue numbers
        r'|\b\w[\w./\\-]{3,}\.'          # file paths with extensions
        r'(?:rs|py|ts|tsx|js|jsx|md|json|toml|sh|sql|yaml|yml)\b', re.I
    )
    df = defaultdict(set)
    for sp in spans:
        df[norm(sp["line"])].add(sp["sid"])
    nsess = len({r[0] for r in rows})
    df_cut = max(3, int(0.25 * nsess))
    spans = [sp for sp in spans
             if len(df[norm(sp["line"])]) < df_cut or NAMED_ENTITY_RE.search(sp["line"])]
    n_after_df = len(spans)

    # ---- 4: lexical dedup on normalised form (keep first occurrence, time-ordered) ----
    spans.sort(key=lambda s: (s["t"], s["sid"]))
    seen = set(); lex = []
    for sp in spans:
        k = norm(sp["line"])[:80]
        if k in seen:
            continue
        seen.add(k); lex.append(sp)
    n_after_lex = len(lex)

    # ---- 5: semantic dedup (SemDeDup) ----
    texts = [sp["line"] for sp in lex]
    V = embed.embed("qwen3-0.6b", texts, cache_tag=f"hourlines-{hour}")
    keep_mask = np.ones(len(lex), bool)
    # Greedy cross-session dedup: drop a span if cosine>SEM_THR to any already-kept span,
    # UNLESS it contains a named entity (ticket key, PR number, file path) AND the
    # most-similar kept span is from a DIFFERENT session — each session's own mention
    # of a key artifact carries different context and must not be silenced by an earlier
    # session's mention of the same file or PR.
    kept_idx = []
    for i in range(len(lex)):
        if kept_idx:
            sims = V[i] @ V[kept_idx].T
            if sims.max() > sem_thr:
                # Check exemption: named-entity span from a different session
                top_kept = kept_idx[int(sims.argmax())]
                same_sid = lex[top_kept]["sid"] == lex[i]["sid"]
                if same_sid or not NAMED_ENTITY_RE.search(lex[i]["line"]):
                    keep_mask[i] = False
                    continue
        kept_idx.append(i)
    sem = [lex[i] for i in range(len(lex)) if keep_mask[i]]
    n_after_sem = len(sem)

    # ---- 5b: (rerank mode) score worklog-salience with Qwen3-Reranker ----
    if select_mode == "rerank":
        import salience
        embed.free()  # one model resident at a time
        sc = salience.score_lines([sp["line"] for sp in sem])
        for sp, s in zip(sem, sc):
            sp["score"] = s
        salience.free()

    # ---- 6: PER-SESSION selection with a floor (every session is represented) ----
    idx_of = {id(sp): i for i, sp in enumerate(sem)}
    Vsem = V[[i for i in range(len(lex)) if keep_mask[i]]]

    def score_pick(sp_list, k):
        # keep the k highest worklog-salience spans, then restore time order
        ranked = sorted(sp_list, key=lambda s: -s.get("score", 0.0))[:k]
        return sorted(ranked, key=lambda s: s["t"])

    def floc_pick(sp_list, k):
        if len(sp_list) <= k:
            return sp_list
        ids = [idx_of[id(s)] for s in sp_list]
        sub = Vsem[ids]
        # start from longest line (most informative), then max-min diversity
        picked = [int(np.argmax([len(s["line"]) for s in sp_list]))]
        while len(picked) < k:
            best, bestv = None, 2.0
            for j in range(len(sp_list)):
                if j in picked:
                    continue
                mx = max(float(sub[j] @ sub[p]) for p in picked)
                if mx < bestv:
                    bestv, best = mx, j
            picked.append(best)
        return [sp_list[j] for j in sorted(picked, key=lambda j: sp_list[j]["t"])]

    by_sid = defaultdict(list)
    for sp in sem:
        by_sid[sp["sid"]].append(sp)

    FLOOR, CEIL = 3, 14
    selected = []
    n_rescued = 0
    for sid in [r[0] for r in rows]:
        sps = by_sid.get(sid, [])
        if sps:
            cap = max(FLOOR, min(CEIL, len(sps)))
            pick = score_pick if select_mode == "rerank" else floc_pick
            selected.extend(pick(sps, cap))
        else:
            # RESCUE: session lost all prose-gated spans -> keep its 2 longest raw lines
            app, win, t0 = meta[sid]
            seen2 = set(); uniq = []
            for t, l in sorted(rescue_by_sid.get(sid, []), key=lambda x: -len(x[1])):
                k = norm(l)[:80]
                if k in seen2:
                    continue
                seen2.add(k); uniq.append((t, l))
                if len(uniq) >= 2:
                    break
            if uniq:
                n_rescued += 1
                for t, l in sorted(uniq):
                    selected.append({"sid": sid, "app": app, "win": win, "t": t, "line": l})
            else:
                # truly empty OCR (AnyDesk/Activity Monitor): keep a presence marker
                selected.append({"sid": sid, "app": app, "win": win, "t": t0,
                                 "line": f"(no readable on-screen text; {app} window '{win}')"})

    # ---- 7: order by time, regroup into window-threads ----
    selected.sort(key=lambda s: (s["t"], s["sid"]))
    threads = []
    for sp in selected:
        if threads and threads[-1]["app"] == sp["app"] and threads[-1]["win"] == sp["win"]:
            threads[-1]["spans"].append(sp)
        else:
            threads.append({"app": sp["app"], "win": sp["win"], "t0": sp["t"], "spans": [sp]})

    out_lines = []
    apps_tl = [t["app"] for t in threads]
    for t in threads:
        block = [f'\n[{t["t0"]} · {t["app"]} · {t["win"] or "—"}]']
        for sp in t["spans"]:
            block.append(f'  {sp["line"][:220]}')
        out_lines.append("\n".join(block))

    # header
    span_min = rows[0][2][11:16]; span_max = rows[-1][2][11:16]
    active_s = sum(r[3] for r in rows)
    header = (f"=== HOUR {hour[11:]}:00 · {nsess} sessions · {active_s//60} min active · "
              f"{span_min}–{span_max} ===\n"
              f"app timeline: {' → '.join(apps_tl)}")
    body = header + "\n" + "\n".join(out_lines)

    out_chars = len(body)
    stats = {"raw_chars": raw_chars, "out_chars": out_chars, "nsess": nsess,
             "after_junk": n_after_junk, "after_df": n_after_df,
             "after_lex": n_after_lex, "after_sem": n_after_sem, "rescued": n_rescued,
             "threads": len(threads), "included_sids": [r[0] for r in rows],
             # sessions with real text (sem spans or rescue lines); presence-marker-only excluded
             "sids_with_spans": sorted({sp["sid"] for sp in selected
                                        if not sp["line"].startswith("(no readable")})}
    return body, stats


def main():
    hour = sys.argv[1] if len(sys.argv) > 1 else "2026-06-23T05"
    mode = sys.argv[2] if len(sys.argv) > 2 else "floc"
    body, st = build_hour_text(hour, select_mode=mode)
    print(body)
    rep = (f"\n{'='*70}\nFUNNEL  raw={st['raw_chars']} chars (~{st['raw_chars']//4} tok)  "
           f"spans:{st['after_junk']}→df:{st['after_df']}→lex:{st['after_lex']}→sem:{st['after_sem']}\n"
           f"OUTPUT {st['out_chars']} chars (~{st['out_chars']//4} tok)  "
           f"reduction {100*(1-st['out_chars']/max(st['raw_chars'],1)):.1f}%  threads={st['threads']}")
    print(rep, file=sys.stderr)
    with open(os.path.join(os.path.dirname(__file__), f"hourtext_{hour}.txt"), "w") as f:
        f.write(body)


if __name__ == "__main__":
    main()
