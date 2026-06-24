"""
Rerank untracked sessions against all open PM tasks using Qwen3-Reranker.

Usage:
  services/.venv/bin/python services/tests/evals/rerank/run_jun18_untracked.py [--date YYYY-MM-DD] [--days N] [--db PATH] [--model REPO]

  --date  YYYY-MM-DD   single day to analyse (default: yesterday)
  --days  N            instead of --date, analyse the last N days
  --db    PATH         path to meridian.db (default: ~/.meridian/meridian.db)
  --model REPO         HuggingFace repo for the reranker
                       (default: kerncore/Qwen3-Reranker-0.6B-MLX-4bit on Apple Silicon,
                        Qwen/Qwen3-Reranker-0.6B otherwise)
"""
import argparse, json, os, platform, re, time, sqlite3, sys
from datetime import date, timedelta
from pathlib import Path

import mlx.core as mx
import mlx.nn as nn
from mlx_lm import load

# ── CLI args ──────────────────────────────────────────────────────────────────
parser = argparse.ArgumentParser(description="Rerank untracked sessions against PM tasks")
parser.add_argument("--date",  default=None, help="YYYY-MM-DD to analyse (default: yesterday)")
parser.add_argument("--days",  type=int, default=None, help="analyse last N days instead of --date")
parser.add_argument("--db",    default=os.path.expanduser("~/.meridian/meridian.db"), help="path to meridian.db")
parser.add_argument("--model", default=None, help="HuggingFace reranker repo")
args = parser.parse_args()

# Resolve date window
if args.days:
    today = date.today()
    date_filter = f"date(started_at) >= '{(today - timedelta(days=args.days)).isoformat()}'"
    run_label = f"last{args.days}d"
elif args.date:
    date_filter = f"date(started_at) = '{args.date}'"
    run_label = args.date
else:
    yesterday = (date.today() - timedelta(days=1)).isoformat()
    date_filter = f"date(started_at) = '{yesterday}'"
    run_label = yesterday

DB = args.db

# Resolve model: prefer MLX 4-bit on Apple Silicon, base model elsewhere
is_apple_silicon = platform.system() == "Darwin" and platform.machine() == "arm64"
if args.model:
    REPO = args.model
elif is_apple_silicon:
    REPO = "kerncore/Qwen3-Reranker-0.6B-MLX-4bit"
else:
    REPO = "Qwen/Qwen3-Reranker-0.6B"

ROOT = Path(__file__).parent
OUT_DIR = ROOT / "results"

MAX_LENGTH = 8192
THR_HIGH   = 0.50   # confident bind (also needs margin >= MARGIN_MIN)
THR_MID    = 0.30   # ambiguous — needs LLM gate
MARGIN_MIN = 0.20   # top1 - top2 must exceed this for a HIGH bind

# ── instruction injection ─────────────────────────────────────────────────────
# Mirrors the sentence-transformers `prompts` parameter: keeps the instruction
# separate from the query text so the model applies it as intended.

INSTRUCTIONS = {
    # Balanced: same component/objective required, but code review / monitoring
    # / research count as valid work modes — not just "writing the deliverable".
    # Rejects sessions where the ticket appears only in background context.
    "balanced": (
        "The Query is a developer work-session summary; the Document is a PM ticket. "
        "Answer yes if the session's primary focus is the SAME feature, component, or "
        "system this ticket is about — including: writing or editing code for it, "
        "reviewing a PR for it, debugging or inspecting traces/dashboards for it, or "
        "researching a specific blocker for it. "
        "Answer no if: the ticket's name or file paths appear only on a background "
        "screen or side panel while the developer's active work is on a different "
        "component; the session is generic housekeeping (git status, brief UI navigation, "
        "Google searches unrelated to the ticket's specific goal); or the session text "
        "explicitly states the work does not match the ticket's scope."
    ),
    # Strictest — primary objective must match, reading/passive excluded.
    "entity": (
        "The Query is a developer work-session summary; the Document is a PM ticket. "
        "Answer yes only if the session's PRIMARY OBJECTIVE is the SAME feature or "
        "component this ticket is about. Reject if: the ticket name merely appears on "
        "screen or in a background tab; the developer is reading/researching rather "
        "than actively building the ticket's deliverable; or the session is git "
        "housekeeping, brief UI navigation, or passive monitoring with no hands-on "
        "work performed."
    ),
    # Loosest — any advancement of the ticket goal counts.
    "base": (
        "Given a software-engineering work-session summary (the Query), judge whether "
        "the work described advances the goal of the project-management ticket (the "
        "Document). Answer yes only if completing this work would make progress on "
        "that specific ticket."
    ),
}
DEFAULT_INSTR = "balanced"

PREFIX = (
    "<|im_start|>system\nJudge whether the Document meets the requirements based on "
    "the Query and the Instruct provided. Note that the answer can only be \"yes\" or "
    "\"no\".<|im_end|>\n<|im_start|>user\n"
)
SUFFIX = "<|im_end|>\n<|im_start|>assistant\n<think>\n\n</think>\n\n"


def format_instruction(instr_key: str, query: str, doc: str) -> str:
    """Build the full reranker prompt. Mirrors HF format_instruction()."""
    return (
        f"{PREFIX}"
        f"<Instruct>: {INSTRUCTIONS[instr_key]}\n"
        f"<Query>: {query}\n"
        f"<Document>: {doc}"
        f"{SUFFIX}"
    )


# ── pre-filters (generic — derived from summariser output patterns, no ticket
#    or machine knowledge needed) ──────────────────────────────────────────────

OVERHEAD_PHRASES = [
    "no code edits", "no hands-on work", "no ticket-specific work",
    "no coding", "no work performed", "passive monitoring",
    "session was brief", "git pull", "git status", "git fetch",
    "no edits or commits", "no edits or terminal", "no production work",
]

BACKGROUND_MISMATCH_PATTERNS = [
    r"visible in background but not",
    r"active work is on\b",
    r"not on those tickets",
    r"does not clearly match",
    r"does not map to",
    r"screen displays.*but the active work",
    r"unrelated to.*current",
]


def is_overhead(summary: str) -> bool:
    """True when the summariser's own language signals no real work was done.

    Duration is intentionally NOT used: screenpipe creates fine-grained
    per-app-switch segments (often 10-90s) even for substantive work like
    reviewing a PR or inspecting a dashboard trace.
    """
    s = summary.lower()
    return sum(1 for p in OVERHEAD_PHRASES if p in s) >= 2


def has_background_mismatch(summary: str) -> bool:
    """True when the summariser explicitly flagged that visible context ≠ active work."""
    s = summary.lower()
    return any(re.search(p, s) for p in BACKGROUND_MISMATCH_PATTERNS)


# ── load tickets from DB (generic across all installs) ────────────────────────
def load_tickets(db_path: str) -> dict:
    """Load all open, non-terminal PM tasks from meridian.db."""
    con = sqlite3.connect(db_path)
    rows = con.execute("""
        SELECT task_key, title, description_text, issue_type, epic_title
        FROM pm_tasks
        WHERE is_terminal = 0
    """).fetchall()
    con.close()
    return {
        r[0]: {
            "task_key": r[0], "title": r[1], "description_text": r[2],
            "issue_type": r[3] or "Task", "epic_title": r[4] or "",
        }
        for r in rows
    }


def doc_text(t: dict) -> str:
    title = f"[{t['issue_type']}] {t['title']}"
    epic  = f" Epic: {t['epic_title']}." if t["epic_title"] else ""
    desc  = (t.get("description_text") or "").strip().replace("\n", " ")[:600]
    return f"{title}.{epic} {desc}".strip()


# ── load sessions ─────────────────────────────────────────────────────────────
def load_sessions(db_path: str, date_filter: str) -> list[dict]:
    """Load untracked sessions matching the date filter."""
    con = sqlite3.connect(db_path)
    rows = con.execute(f"""
        SELECT id, app_name, started_at, duration_s, session_summary, window_titles
        FROM app_sessions
        WHERE task_session_type = 'untracked'
          AND {date_filter}
          AND session_summary IS NOT NULL AND session_summary != ''
        ORDER BY started_at
    """).fetchall()
    con.close()
    return [
        {"id": r[0], "app": r[1], "started": r[2], "duration_s": r[3],
         "summary": r[4], "window_titles": r[5]}
        for r in rows
    ]


# ── load data ─────────────────────────────────────────────────────────────────
tasks = load_tickets(DB)
if not tasks:
    print("No open PM tasks found in DB — run the Jira sync first.", file=sys.stderr)
    sys.exit(1)
task_keys = list(tasks.keys())
docs = {k: doc_text(t) for k, t in tasks.items()}
print(f"Tickets loaded: {len(tasks)} (from {DB})")

sessions = load_sessions(DB, date_filter)
if not sessions:
    print(f"No untracked sessions found for filter: {date_filter}", file=sys.stderr)
    sys.exit(0)
print(f"Sessions loaded: {len(sessions)} (filter: {date_filter})")

# ── load model ────────────────────────────────────────────────────────────────
print(f"\nLoading {REPO} ...", flush=True)
model, tok = load(REPO)
tok.padding_side = "left"   # required: causal LM needs left-padding for batched inference
pad_id = tok.pad_token_id if tok.pad_token_id is not None else tok.eos_token_id
yes_id = tok.encode("yes", add_special_tokens=False)[0]
no_id  = tok.encode("no",  add_special_tokens=False)[0]
print(f"Active mem: {mx.get_active_memory()/1e9:.2f} GB", flush=True)


def score_batch(query: str, doc_list: list[str], instr_key: str = DEFAULT_INSTR) -> list[float]:
    """Score one query against all docs in a single MLX forward pass.

    Tokenises each (query, doc) pair, truncates to MAX_LENGTH from the left
    so the yes/no prediction token always sits at position [-1], then
    left-pads all sequences to the same length before the forward pass.
    """
    all_ids = []
    for d in doc_list:
        ids = tok.encode(format_instruction(instr_key, query, d), add_special_tokens=False)
        all_ids.append(ids[-MAX_LENGTH:])   # truncate from left

    max_len = max(len(ids) for ids in all_ids)
    padded  = [[pad_id] * (max_len - len(ids)) + ids for ids in all_ids]

    batch  = mx.array(padded)               # (N, max_len)
    logits = model(batch)[:, -1, :]        # (N, vocab) — final-token logits only

    yes_l   = logits[:, yes_id]
    no_l    = logits[:, no_id]
    probs   = nn.softmax(mx.stack([no_l, yes_l], axis=1), axis=1)[:, 1]
    mx.eval(probs)
    return probs.tolist()


# ── score ─────────────────────────────────────────────────────────────────────
print(f"\nScoring {len(sessions)} sessions × {len(tasks)} tickets ({DEFAULT_INSTR} instruction) ...\n", flush=True)
t0 = time.time()

results, pre_filtered = [], []

for i, s in enumerate(sessions):
    tag = f"\r[{i+1:3d}/{len(sessions)}]"

    if is_overhead(s["summary"]):
        pre_filtered.append({**s, "filter": "overhead", "ranked": [], "top_key": None, "top_score": 0.0, "margin": 0.0})
        print(f"{tag} overhead", end="", flush=True)
        continue

    if has_background_mismatch(s["summary"]):
        pre_filtered.append({**s, "filter": "background_mismatch", "ranked": [], "top_key": None, "top_score": 0.0, "margin": 0.0})
        print(f"{tag} bg-mismatch", end="", flush=True)
        continue

    scores_list = score_batch(s["summary"], [docs[k] for k in task_keys])
    ranked = sorted(zip(task_keys, scores_list), key=lambda x: -x[1])
    top_key, top_score = ranked[0]
    margin = top_score - (ranked[1][1] if len(ranked) > 1 else 0.0)

    results.append({**s, "filter": None, "ranked": ranked, "top_key": top_key, "top_score": top_score, "margin": margin})
    print(f"{tag} {time.time()-t0:4.0f}s  top={top_key}({top_score:.2f}) margin={margin:.2f}", end="", flush=True)

print(f"\n\nDone in {time.time()-t0:.0f}s  |  peak mem {mx.get_peak_memory()/1e9:.2f} GB\n")

# ── analyse ───────────────────────────────────────────────────────────────────
high             = [r for r in results if r["top_score"] >= THR_HIGH and r["margin"] >= MARGIN_MIN]
high_low_margin  = [r for r in results if r["top_score"] >= THR_HIGH and r["margin"] < MARGIN_MIN]
mid              = [r for r in results if THR_MID <= r["top_score"] < THR_HIGH]
low              = [r for r in results if r["top_score"] < THR_MID]
overhead_f       = [r for r in pre_filtered if r["filter"] == "overhead"]
mismatch_f       = [r for r in pre_filtered if r["filter"] == "background_mismatch"]
total            = len(sessions)

print("=" * 72)
print(f"Untracked sessions — {run_label}  (n={total}  instr={DEFAULT_INSTR}  margin>={MARGIN_MIN})")
print(f"  PRE-FILTERED overhead     : {len(overhead_f):3d}  ({len(overhead_f)/total:.0%})")
print(f"  PRE-FILTERED bg-mismatch  : {len(mismatch_f):3d}  ({len(mismatch_f)/total:.0%})")
print(f"  BIND  (≥{THR_HIGH}, margin≥{MARGIN_MIN})    : {len(high):3d}  ({len(high)/total:.0%})")
print(f"  BIND? (≥{THR_HIGH}, margin<{MARGIN_MIN}) : {len(high_low_margin):3d}  ({len(high_low_margin)/total:.0%})  → LLM gate")
print(f"  AMBIG ({THR_MID}–{THR_HIGH})            : {len(mid):3d}  ({len(mid)/total:.0%})  → LLM gate")
print(f"  NO MATCH (<{THR_MID})           : {len(low):3d}  ({len(low)/total:.0%})  → new-task candidate")
print()

from collections import Counter, defaultdict

if high:
    ticket_hits: dict = defaultdict(list)
    for r in high:
        ticket_hits[r["top_key"]].append(r)
    print("Ticket binding distribution (confident binds):")
    for k in sorted(ticket_hits, key=lambda k: -len(ticket_hits[k])):
        sc = [r["top_score"] for r in ticket_hits[k]]
        print(f"  {k:12s} {tasks[k]['title'][:55]:55s} → {len(ticket_hits[k])} sessions  avg={sum(sc)/len(sc):.2f}")
    print()

def app_table(bucket, label):
    c = Counter(r["app"] for r in bucket)
    print(f"{label}  (n={len(bucket)}):")
    for app, cnt in c.most_common():
        print(f"  {cnt:3d}  {app}")
    print()

app_table(overhead_f,          "PRE-FILTERED overhead")
app_table(mismatch_f,          "PRE-FILTERED bg-mismatch")
app_table(high,                f"BIND (≥{THR_HIGH}, margin≥{MARGIN_MIN})")
app_table(mid + high_low_margin, f"LLM GATE")
app_table(low,                 f"NO MATCH (<{THR_MID})")

def print_table(bucket, label, max_rows=30):
    print(f"\n{'='*72}\n{label}  [{len(bucket)} sessions]")
    print(f"{'ID':>7}  {'App':14}  {'Sc':5} {'Mrg':5}  {'TopTicket':10}  Summary[:90]")
    print("-" * 72)
    for r in bucket[:max_rows]:
        sc = r.get("top_score", 0); mg = r.get("margin", 0)
        print(f"{r['id']:7d}  {r['app']:14}  {sc:.3f} {mg:.3f}  {str(r.get('top_key') or '—'):10}  {r['summary'].replace(chr(10),' ')[:90]}")
    if len(bucket) > max_rows:
        print(f"  ... and {len(bucket)-max_rows} more")

print_table(high,            f"CONFIDENT BINDS (≥{THR_HIGH} + margin≥{MARGIN_MIN})")
print_table(high_low_margin, f"HIGH SCORE LOW MARGIN → LLM gate")
print_table(mid,             f"AMBIGUOUS ({THR_MID}–{THR_HIGH}) → LLM gate")
print_table(overhead_f,      "PRE-FILTERED: overhead")
print_table(mismatch_f,      "PRE-FILTERED: background mismatch")

# ── save ──────────────────────────────────────────────────────────────────────
out = {
    "run": run_label, "instr": DEFAULT_INSTR, "model": REPO,
    "n_sessions": total, "n_tickets": len(tasks),
    "thresholds": {"high": THR_HIGH, "mid": THR_MID, "margin": MARGIN_MIN},
    "bind": len(high), "bind_low_margin": len(high_low_margin),
    "ambig": len(mid), "no_match": len(low),
    "filtered_overhead": len(overhead_f), "filtered_mismatch": len(mismatch_f),
    "details": [
        {"id": r["id"], "app": r["app"], "started": r["started"],
         "duration_s": r["duration_s"], "filter": r.get("filter"),
         "top_key": r.get("top_key"), "top_score": round(r.get("top_score", 0), 4),
         "margin": round(r.get("margin", 0), 4),
         "top5": [(k, round(sc, 4)) for k, sc in r.get("ranked", [])[:5]],
         "summary_100": r["summary"][:100]}
        for r in results + pre_filtered
    ],
}
OUT_DIR.mkdir(exist_ok=True)
out_path = OUT_DIR / f"untracked_rerank_{run_label}.json"
out_path.write_text(json.dumps(out, indent=2))
print(f"\nResults → {out_path.relative_to(ROOT.parent.parent.parent)}")
