"""Deterministic content-recall audit for the reduced hour session-text.

For each peak-hour gold session_summary, extract concrete named facts
(ticket keys, file paths, PR numbers, feature names, quoted strings).
Then check which of those facts are findable in the reduced hour text.
No LLM, no randomness — pure string matching.

Recall = facts_found / total_facts  (per session and overall).
Missing facts are printed so we can SEE the actual loss.

Usage: python audit_facts.py [N_hours] [select_mode]
"""
import sys, os, re, sqlite3
from collections import Counter, defaultdict
import clib
from hour_text import build_hour_text, EXCLUDE, MIN_DUR

def _main_args():
    global N_HOURS, SELECT_MODE
    N_HOURS = int(sys.argv[1]) if len(sys.argv) > 1 else 12
    SELECT_MODE = sys.argv[2] if len(sys.argv) > 2 else "floc"

N_HOURS = 12
SELECT_MODE = "floc"

# ---- fact extractors ----
TICKET_RE = re.compile(r'\b([A-Z]{2,10}-\d+)\b')
PR_RE = re.compile(r'\b(?:PR\s*#?|#)(\d{2,5})\b')
EXT_PATH_RE = re.compile(
    r'(?:^|[\s\'"`(])([a-zA-Z0-9_./\\-]{4,}\.(?:rs|py|ts|tsx|js|jsx|md|json|toml|sh|sql|yaml|yml|env|lock))\b'
)
QUOTED_RE = re.compile(r'[`"\']([A-Za-z][A-Za-z0-9_./: -]{3,40})[`"\']')
FUNC_RE = re.compile(r'\b([a-z][a-z0-9]*(?:_[a-z][a-z0-9]+)+)\b')   # snake_case identifiers
CAMEL_RE = re.compile(r'\b([A-Z][a-z]+(?:[A-Z][a-z0-9]+)+)\b')        # CamelCase names
URL_SLUG_RE = re.compile(r'(?:https?://[^\s]+/|/)([a-zA-Z0-9_-]{5,})')

COMMON = set(
    "the and for this that with from have has been when what about which were they "
    "will not you can but its use used using like more some into out would should "
    "could also just then there where because while though after before update fixed "
    "added removed changed also view page file code test build run make done get set "
    "new old first last work user data time list item full now open show back next "
    "start end return type name path mode key flag true false null none error".split()
)

def extract_facts(summary: str) -> list[str]:
    """Pull concrete named facts from a gold summary. Order: most-specific first."""
    facts = []
    s = summary

    # 1. ticket keys (KAN-123) — high precision
    for m in TICKET_RE.finditer(s):
        facts.append(m.group(1))

    # 2. PR / issue numbers (#325, PR 316)
    for m in PR_RE.finditer(s):
        facts.append(f"#{m.group(1)}")

    # 3. file paths / filenames with extensions
    for m in EXT_PATH_RE.finditer(s):
        f = m.group(1).lstrip("./")
        if len(f) >= 4:
            facts.append(f)

    # 4. quoted strings (feature names, branch names, config keys)
    for m in QUOTED_RE.finditer(s):
        tok = m.group(1).strip()
        if tok.lower() not in COMMON and len(tok) >= 4:
            facts.append(tok)

    # 5. snake_case identifiers (function / variable names)
    for m in FUNC_RE.finditer(s):
        tok = m.group(1)
        if tok.lower() not in COMMON and len(tok) >= 8:
            facts.append(tok)

    # 6. CamelCase type / component names
    for m in CAMEL_RE.finditer(s):
        tok = m.group(1)
        if tok.lower() not in COMMON:
            facts.append(tok)

    # 7. URL slugs (page / route names)
    for m in URL_SLUG_RE.finditer(s):
        tok = m.group(1)
        if tok.lower() not in COMMON and len(tok) >= 5:
            facts.append(tok)

    # deduplicate preserving order
    seen = set(); out = []
    for f in facts:
        k = f.lower()
        if k not in seen:
            seen.add(k); out.append(f)
    return out


def fact_present(fact: str, body: str) -> bool:
    """Case-insensitive substring check. Ticket keys must be whole-word."""
    fl = fact.lower()
    bl = body.lower()
    if TICKET_RE.match(fact) or fact.startswith("#"):
        # require word boundary for ticket / PR numbers
        return bool(re.search(r'\b' + re.escape(fl) + r'\b', bl))
    return fl in bl


def peak_hours(n: int) -> list[str]:
    con = sqlite3.connect(clib.DB)
    rows = con.execute(
        f"""SELECT substr(started_at,1,13) h, COUNT(*) c,
                   SUM(CASE WHEN session_summary IS NOT NULL AND LENGTH(session_summary)>5 THEN 1 ELSE 0 END) g
            FROM app_sessions
            WHERE app_name NOT IN ({','.join('?'*len(EXCLUDE))})
              AND duration_s >= ? AND session_text IS NOT NULL AND LENGTH(session_text)>40
              AND started_at >= date('now','-7 days')
            GROUP BY h HAVING g >= 5 ORDER BY c DESC LIMIT ?""",
        (*EXCLUDE, MIN_DUR, n)).fetchall()
    con.close()
    return [r[0] for r in rows]


def gold_for_hour(hour: str) -> list[dict]:
    con = sqlite3.connect(clib.DB)
    rows = con.execute(
        f"""SELECT id, app_name, task_key, task_session_type,
                   COALESCE(NULLIF(session_summary,''), task_reasoning, '')
            FROM app_sessions
            WHERE app_name NOT IN ({','.join('?'*len(EXCLUDE))})
              AND duration_s >= ? AND session_text IS NOT NULL AND LENGTH(session_text)>40
              AND started_at LIKE ? ORDER BY started_at""",
        (*EXCLUDE, MIN_DUR, hour + "%")).fetchall()
    con.close()
    return [{"sid": r[0], "app": r[1], "tk": r[2], "type": r[3], "gold": r[4]}
            for r in rows if r[4] and len(r[4]) > 10]


def audit_hour(hour: str, select_mode: str):
    body, st = build_hour_text(hour, select_mode=select_mode)
    golds = gold_for_hour(hour)
    survived = set(st.get("sids_with_spans", []))

    total_facts = found_facts = 0
    n_no_facts = n_dropout = 0
    lost_lines = []
    by_type: dict[str, Counter] = defaultdict(Counter)

    for g in golds:
        facts = extract_facts(g["gold"])
        ty = g["type"] or "?"

        if g["sid"] not in survived:
            # deterministic dropout — count all facts as missing
            n_dropout += 1
            total_facts += max(len(facts), 1)
            by_type[ty]["dropout"] += 1
            if facts:
                tag = f'{g["app"]}{"/"+g["tk"] if g["tk"] else ""}|{ty}'
                lost_lines.append(f'  [DROPOUT sid{g["sid"]} {tag}] {", ".join(facts[:6])}')
            continue

        if not facts:
            n_no_facts += 1
            continue

        pres = [fact_present(f, body) for f in facts]
        n_found = sum(pres)
        total_facts += len(facts)
        found_facts += n_found
        by_type[ty]["total"] += len(facts)
        by_type[ty]["found"] += n_found

        if n_found < len(facts):
            missing = [f for f, ok in zip(facts, pres) if not ok]
            tag = f'{g["app"]}{"/"+g["tk"] if g["tk"] else ""}|{ty}'
            lost_lines.append(
                f'  [sid{g["sid"]} {tag}] missing {len(missing)}/{len(facts)}: '
                + ", ".join(missing[:8])
            )

    recall = found_facts / total_facts if total_facts else 1.0
    return {
        "hour": hour, "recall": recall,
        "total_facts": total_facts, "found": found_facts,
        "no_facts": n_no_facts, "dropout": n_dropout,
        "nsess": st.get("nsess", 0), "ngold": len(golds),
        "raw_chars": st.get("raw_chars", 0), "out_chars": st.get("out_chars", 0),
        "by_type": dict(by_type), "lost": lost_lines,
    }


def main():
    hours = peak_hours(N_HOURS)
    print(f"Deterministic fact-recall audit · {len(hours)} peak hours · mode={SELECT_MODE}\n")

    grand_total = grand_found = grand_dropout = 0
    all_lost = []
    by_type_grand: dict[str, Counter] = defaultdict(Counter)

    for hour in hours:
        r = audit_hour(hour, SELECT_MODE)
        grand_total += r["total_facts"]
        grand_found += r["found"]
        grand_dropout += r["dropout"]
        red = 100 * (1 - r["out_chars"] / max(r["raw_chars"], 1))
        pct = r["recall"] * 100
        print(
            f"  {hour}  sessions={r['nsess']:2}  gold={r['ngold']:2}  "
            f"facts={r['total_facts']:3}  found={r['found']:3}  "
            f"recall={pct:5.1f}%  reduction={red:.0f}%  "
            f"dropout={r['dropout']}  no-facts={r['no_facts']}"
        )
        all_lost.extend(r["lost"])
        for ty, c in r["by_type"].items():
            by_type_grand[ty].update(c)

    grand_recall = grand_found / grand_total if grand_total else 1.0
    print(f"\n{'='*72}")
    print(f"GRAND  facts={grand_total}  found={grand_found}  "
          f"RECALL={grand_recall:.1%}  dropouts={grand_dropout}")

    print("\nBY SESSION TYPE:")
    for ty, c in sorted(by_type_grand.items(), key=lambda x: -x[1].get("total", 0)):
        tf = c.get("total", 0); ff = c.get("found", 0)
        rc = ff / tf if tf else 1.0
        print(f"  {ty:12} facts={tf:4}  found={ff:4}  recall={rc:.0%}  "
              f"dropouts={c.get('dropout',0)}")

    if all_lost:
        print(f"\n{'='*72}")
        print(f"MISSING FACTS ({len(all_lost)} sessions with losses):")
        for l in all_lost:
            print(l)
    else:
        print("\nNo facts missing — all named details recoverable from the hour text.")


if __name__ == "__main__":
    _main_args()
    main()
