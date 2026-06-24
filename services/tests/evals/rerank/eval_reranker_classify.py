#ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
"""
Eval: Qwen3-Reranker-0.6B as a task-session classifier.

Supports two dataset formats:
  golden  — services/tests/evals/data/generated/goldens_real_combined.json
             (deepeval format; session text + 24 ticket docs parsed from rendered input)
  week    — services/tests/evals/rerank/data/sessions_week.json + labels_week.py + tickets.json
             (raw session_summary + per-session candidate list, 3-5 tickets)

No hardcoded session-text patterns (pre-filter is opt-in via --prefilter).
No machine-specific ticket key format (matches any PROJ-NNN style key).
Instruction injected via <Instruct>/<Query>/<Document> template — equivalent
to sentence-transformers CrossEncoder prompts= parameter.

HF-spec tokenisation (Qwen3-Reranker-0.6B model card):
  prefix + content[:content_max] + suffix
  probability: log_softmax → exp

Usage:
  # Full golden dataset (103 sessions, 24 candidates each)
  services/.venv/bin/python services/tests/evals/rerank/eval_reranker_classify.py

  # Week dataset (101 sessions, 3-5 candidates each)
  services/.venv/bin/python services/tests/evals/rerank/eval_reranker_classify.py --format week

  # Both sequentially
  services/.venv/bin/python services/tests/evals/rerank/eval_reranker_classify.py --format both

  # Override model or chunk size
  services/.venv/bin/python services/tests/evals/rerank/eval_reranker_classify.py --model Qwen/Qwen3-Reranker-0.6B --chunk 4
"""
import argparse, json, os, platform, re, sys, time
from collections import defaultdict
from pathlib import Path

import mlx.core as mx
import mlx.nn as nn
from mlx_lm import load

ROOT      = Path(__file__).parent
EVALS_DIR = ROOT.parent
OUT       = ROOT / "results"

GOLDEN_PATH  = EVALS_DIR / "data" / "generated" / "goldens_real_combined.json"
WEEK_PATH    = ROOT / "data" / "sessions_week.json"
TICKETS_PATH = ROOT / "data" / "tickets.json"

# ── CLI ───────────────────────────────────────────────────────────────────────
parser = argparse.ArgumentParser(description="Eval Qwen3-Reranker-0.6B on task-session classification")
parser.add_argument("--format",    default="golden",
                    choices=["golden", "week", "both"],
                    help="Dataset format (default: golden)")
parser.add_argument("--golden",    default=str(GOLDEN_PATH), help="Path to deepeval goldens JSON")
parser.add_argument("--week",      default=str(WEEK_PATH),   help="Path to sessions_week.json")
parser.add_argument("--tickets",   default=str(TICKETS_PATH),help="Path to tickets.json (week mode)")
parser.add_argument("--model",     default=None,             help="HF repo override")
parser.add_argument("--chunk",     type=int, default=1,
                    help="Docs per forward pass (lower = less GPU mem, default: 6)")
parser.add_argument("--prefilter", action="store_true",
                    help="Enable phrase-based overhead pre-filter (off by default — "
                         "these phrases are summariser-specific, not generic)")
args = parser.parse_args()

is_apple = platform.system() == "Darwin" and platform.machine() == "arm64"
REPO     = args.model or ("kerncore/Qwen3-Reranker-0.6B-MLX-4bit" if is_apple else "Qwen/Qwen3-Reranker-0.6B")
CHUNK    = args.chunk

MAX_LENGTH = 8192
# Eval threshold: match plans.py / eval_mlx.py approach.
# For standalone evaluation we just pick the top-ranked ticket; the margin
# gate is reserved for production where an LLM handles the ambiguous zone.
THR_HIGH   = 0.10   # above this → bind to top-ranked ticket
THR_MID    = 0.10   # same as THR_HIGH → all non-low scores bind
MARGIN_MIN = 0.00   # no margin requirement for eval mode

# ── Instruction (balanced: code, review, monitoring all count as task work)
# Injected via <Instruct>: field — equivalent to CrossEncoder prompts= parameter.
INSTR = (
    "Given a software-engineering work-session summary (the Query), judge whether "
    "the work described advances the goal of the project-management ticket (the "
    "Document). Answer yes only if completing this work would make progress on "
    "that specific ticket."
)

# ── Optional pre-filter (--prefilter only; disabled by default) ───────────────
# These phrases are written by the Meridian summariser. On a different machine
# or with a different summariser the wording will vary — do not use these to
# judge generic accuracy.
_OVERHEAD_PHRASES = [
    "no code edits", "no hands-on work", "no ticket-specific work",
    "no coding", "no work performed", "passive monitoring",
    "no edits or commits", "no edits or terminal", "no production work",
    "generic browsing", "brief navigation", "no active development",
]
_BG_MISMATCH = [
    r"visible in background but not",
    r"active work is on\b",
    r"does not clearly match",
    r"does not map to",
    r"screen displays.*but the active work",
]

def _prefilter(text: str) -> str | None:
    """Return 'overhead' / 'mismatch' if the text matches; None otherwise."""
    t = text.lower()
    if sum(1 for p in _OVERHEAD_PHRASES if p in t) >= 2:
        return "overhead"
    if any(re.search(p, t) for p in _BG_MISMATCH):
        return "mismatch"
    return None


# ── Ticket doc text builder ───────────────────────────────────────────────────
def doc_text(t: dict) -> str:
    """Build a compact doc string from a ticket dict (any key format)."""
    issue_type = t.get("issue_type", "Task")
    title      = t.get("title", "")
    epic       = t.get("epic_title", "") or ""
    desc       = (t.get("description_text") or "").strip().replace("\n", " ")[:600]
    epic_part  = f" Epic: {epic}." if epic else ""
    return f"[{issue_type}]{epic_part} {title}. {desc}".strip()


# ── Golden format parsers ─────────────────────────────────────────────────────
# Ticket key pattern: any PROJ-NNN style (not just KAN-xxx)
_KEY_RE = re.compile(r"[A-Z]+-\d+")

def parse_golden(inp: str) -> tuple[str, dict[str, str]]:
    """Extract session text and {ticket_key: doc} from a deepeval golden's input.

    Parses what build_user_message() rendered — no golden-specific hardcoding
    beyond the known template structure (screen content block, numbered ticket list).
    """
    # Session text: after 'screen content [source]:\n' up to next blank + uppercase
    m = re.search(r"screen content \[[\w_]+\]:\n(.*?)(?=\n\n[A-Z0-9]|\Z)", inp, re.DOTALL)
    session_text = m.group(1).strip() if m else ""

    # Ticket docs: numbered entries '2. PROJ-123  [Type · Epic: ...]'
    tickets: dict[str, str] = {}
    for block in re.split(r"\n(?=\d+\. [A-Z]+-\d+\s+\[)", inp):
        km = re.match(
            r"\d+\. ([A-Z]+-\d+)\s+\[([^\]]+)\]\n\s+title:\s*(.*?)\n\s+description:\s*(.*?)(?=\n\n|\Z)",
            block, re.DOTALL,
        )
        if not km:
            continue
        key   = km.group(1)
        meta  = km.group(2)
        title = km.group(3).strip()
        desc  = km.group(4).strip().replace("\n", " ")[:600]
        epic  = ""
        em = re.search(r"Epic:\s*(.+)", meta)
        if em:
            epic = em.group(1).strip()
        fake_ticket = {"issue_type": "Task", "title": title,
                       "epic_title": epic, "description_text": desc}
        tickets[key] = doc_text(fake_ticket)

    return session_text, tickets


# ── Model load ────────────────────────────────────────────────────────────────
print(f"Loading {REPO} ...", flush=True)
model, tok = load(REPO)
tok.padding_side = "left"   # required: causal LM needs yes/no at position [-1]

PREFIX_STR = (
    "<|im_start|>system\nJudge whether the Document meets the requirements based on "
    "the Query and the Instruct provided. Note that the answer can only be \"yes\" or "
    "\"no\".<|im_end|>\n<|im_start|>user\n"
)
SUFFIX_STR = "<|im_end|>\n<|im_start|>assistant\n<think>\n\n</think>\n\n"

# Pre-compute once (HF spec: separate prefix/suffix from content budget)
prefix_tokens = tok.encode(PREFIX_STR, add_special_tokens=False)
suffix_tokens = tok.encode(SUFFIX_STR, add_special_tokens=False)
content_max   = MAX_LENGTH - len(prefix_tokens) - len(suffix_tokens)

yes_id = tok.encode("yes", add_special_tokens=False)[0]
no_id  = tok.encode("no",  add_special_tokens=False)[0]
pad_id = tok.pad_token_id if tok.pad_token_id is not None else tok.eos_token_id

print(f"Active mem: {mx.get_active_memory()/1e9:.2f} GB  "
      f"prefix={len(prefix_tokens)} suffix={len(suffix_tokens)} content_max={content_max}",
      flush=True)


def score_batch(query: str, doc_list: list[str]) -> list[float]:
    """
    HF-spec tokenisation (from model card):
      1. Tokenize content (instruct+query+doc) only — no prefix/suffix yet
      2. Truncate content to content_max so prefix+content+suffix <= MAX_LENGTH
      3. Prepend prefix_tokens, append suffix_tokens
    This guarantees the suffix (yes/no prediction anchor) is NEVER truncated,
    even for very long session texts. Encoding the full string and slicing with
    [:MAX_LENGTH] incorrectly cuts the suffix on long inputs.
    Call .item() after each inference to break the MLX lazy graph immediately,
    preventing computation-graph accumulation that causes OOM over many calls.
    """
    scores = []
    for doc in doc_list:
        content     = f"<Instruct>: {INSTR}\n<Query>: {query}\n<Document>: {doc}"
        content_ids = tok.encode(content, add_special_tokens=False)[:content_max]
        ids         = prefix_tokens + content_ids + suffix_tokens
        lg          = model(mx.array([ids]))[0, -1, :]
        p           = nn.softmax(mx.array([lg[no_id].item(), lg[yes_id].item()]))[1].item()
        scores.append(p)
    return scores


def score_all(query: str, docs: dict[str, str]) -> list[tuple[str, float]]:
    """Score query against all docs in CHUNK-sized batches; return sorted desc."""
    keys   = list(docs.keys())
    doc_ls = [docs[k] for k in keys]
    scores: list[float] = []
    for i in range(0, len(doc_ls), CHUNK):
        scores += score_batch(query, doc_ls[i:i + CHUNK])
    return sorted(zip(keys, scores), key=lambda x: -x[1])


def classify(session_text: str, ticket_docs: dict[str, str]) -> tuple[str, str | None, str]:
    """Classify one session. Returns (act_type, act_key, method_label)."""
    # Optional phrase pre-filter (only when --prefilter is passed)
    if args.prefilter:
        pf = _prefilter(session_text)
        if pf:
            return "overhead", None, f"pre-filter:{pf}"

    if not ticket_docs:
        return "untracked", None, "no-tickets"

    ranked      = score_all(session_text, ticket_docs)
    top_key, sc = ranked[0]
    margin      = sc - (ranked[1][1] if len(ranked) > 1 else 0.0)

    # Bind to top-ranked ticket if score exceeds threshold.
    # No margin gate in eval mode — the 9B LLM handles ambiguous cases in production.
    if sc >= THR_HIGH:
        return "task", top_key, f"bind({sc:.2f}/m{margin:.2f})"
    return "untracked", None, f"low({sc:.2f})"


# ── Eval runner ───────────────────────────────────────────────────────────────

def run_golden(path: Path) -> list[dict]:
    goldens = json.loads(path.read_text())
    print(f"\n{'='*70}")
    print(f"Dataset: {path.name}  ({len(goldens)} goldens)  format=golden")
    print(f"Chunk={CHUNK}  prefilter={args.prefilter}\n", flush=True)

    rows = []
    t0   = time.time()
    for i, g in enumerate(goldens):
        meta     = g.get("additional_metadata", {})
        seed_id  = meta.get("seed_id", meta.get("session_id", "?"))
        diff     = meta.get("difficulty", "?")
        exp      = json.loads(g["expected_output"])
        exp_key  = exp.get("task_key")
        exp_type = exp.get("session_type", "")

        session_text, ticket_docs = parse_golden(g["input"])
        tag = f"[{i+1:3d}/{len(goldens)}]"
        if not session_text:
            print(f"{tag} SKIP — no session text parsed", flush=True)
            continue

        act_type, act_key, method = classify(session_text, ticket_docs)
        # Match plans.py report() formula: pred in (acceptable | {'NONE'} if non-task)
        pred       = act_key if act_key else "NONE"
        accept_set = ({exp_key} if exp_key else set()) | ({"NONE"} if exp_type != "task" else set())
        hit        = pred in accept_set
        type_ok = key_ok = hit
        rows.append({"seed_id": seed_id, "difficulty": diff, "dataset": path.name,
                     "exp_type": exp_type, "act_type": act_type,
                     "exp_key": exp_key,   "act_key": act_key,
                     "type_ok": type_ok,   "key_ok": key_ok, "method": method})
        _print_row(tag, diff, exp_type, act_type, exp_key, act_key, type_ok, key_ok, method, t0)

        mx.clear_cache()  # free JIT kernel cache after every session — grows ~3-4 GB/session otherwise

    return rows


def run_week(sessions_path: Path, tickets_path: Path) -> list[dict]:
    import importlib.util, types

    sessions = json.loads(sessions_path.read_text())

    # Load labels_week dynamically (avoids sys.path pollution)
    labels_file = sessions_path.parent / "labels_week.py"
    spec = importlib.util.spec_from_file_location("labels_week", labels_file)
    mod  = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    labels: dict = mod.L   # {session_id: (primary, acceptable_set, uncertain, stype, note)}

    # Load ticket pool
    raw_tickets = json.loads(tickets_path.read_text())
    if isinstance(raw_tickets, list):
        ticket_map = {t["task_key"]: t for t in raw_tickets}
    else:
        ticket_map = raw_tickets
    all_docs = {k: doc_text(t) for k, t in ticket_map.items()}

    print(f"\n{'='*70}")
    print(f"Dataset: {sessions_path.name}  ({len(sessions)} sessions)  format=week")
    print(f"Tickets: {len(ticket_map)}  Chunk={CHUNK}  prefilter={args.prefilter}\n", flush=True)

    rows = []
    t0   = time.time()
    for i, s in enumerate(sessions):
        sid  = s["id"]
        label = labels.get(sid)
        if label is None:
            continue
        primary, acceptable, uncertain, stype, note = label
        exp_key  = primary if primary != "NONE" else None
        exp_type = stype

        # Use only the per-session candidate list
        cand_keys   = s.get("candidates", [])
        ticket_docs = {k: all_docs[k] for k in cand_keys if k in all_docs}
        session_text = s.get("session_summary", "")
        diff = "uncertain" if uncertain else stype

        tag = f"[{i+1:3d}/{len(sessions)}]"
        if not session_text:
            print(f"{tag} SKIP — empty session_summary", flush=True)
            continue

        act_type, act_key, method = classify(session_text, ticket_docs)

        # Match plans.py/eval_mlx.py report() formula exactly:
        #   pred = top_key if score >= THR else 'NONE'
        #   acceptable_set = okset | ({'NONE'} if primary == 'NONE' else {})
        #   hit = pred in acceptable_set
        pred         = act_key if act_key else "NONE"
        accept_set   = set(acceptable) | ({"NONE"} if primary == "NONE" else set())
        hit          = pred in accept_set
        type_ok = key_ok = hit

        rows.append({"seed_id": sid, "difficulty": diff, "dataset": sessions_path.name,
                     "exp_type": exp_type, "act_type": act_type,
                     "exp_key": exp_key,   "act_key": act_key,
                     "type_ok": type_ok,   "key_ok": key_ok, "method": method})
        _print_row(tag, diff, exp_type, act_type, exp_key, act_key, type_ok, key_ok, method, t0)

        mx.clear_cache()  # free JIT kernel cache after every session — grows ~3-4 GB/session otherwise

    return rows


def _print_row(tag, diff, exp_type, act_type, exp_key, act_key, type_ok, key_ok, method, t0):
    print(
        f"{tag} T{'✓' if type_ok else '✗'} K{'✓' if key_ok else '✗'}  "
        f"{diff:<12} exp={exp_type:9} act={act_type:9} "
        f"key={str(exp_key or '—'):10}→{str(act_key or '—'):10}  "
        f"{method}  [{time.time()-t0:.0f}s]",
        flush=True,
    )


def summarise(rows: list[dict], label: str) -> None:
    n            = len(rows)
    if n == 0:
        print(f"\n{label}: no rows scored.")
        return
    type_correct = sum(1 for r in rows if r["type_ok"])
    key_correct  = sum(1 for r in rows if r["key_ok"])
    both_correct = sum(1 for r in rows if r["type_ok"] and r["key_ok"])

    print(f"\n{'='*70}")
    print(f"RESULTS  {label}  (n={n}  model={REPO})")
    print(f"  session_type accuracy : {type_correct}/{n}  =  {type_correct/n:.1%}")
    print(f"  task_key accuracy     : {key_correct}/{n}   =  {key_correct/n:.1%}")
    print(f"  both correct          : {both_correct}/{n}  =  {both_correct/n:.1%}")

    by_diff: dict = defaultdict(list)
    by_type: dict = defaultdict(list)
    for r in rows:
        by_diff[r["difficulty"]].append(r)
        by_type[r["exp_type"]].append(r)

    print(f"\n  {'diff':<14} {'n':>4}  {'type%':>6}  {'key%':>6}  {'both%':>6}")
    print("  " + "-" * 40)
    for diff in sorted(by_diff):
        items = by_diff[diff]
        nd    = len(items)
        bc    = sum(1 for r in items if r["type_ok"] and r["key_ok"])
        tc    = sum(1 for r in items if r["type_ok"])
        kc    = sum(1 for r in items if r["key_ok"])
        print(f"  {diff:<14} {nd:>4}  {tc/nd:>6.0%}  {kc/nd:>6.0%}  {bc/nd:>6.0%}")

    print(f"\n  {'exp_type':<12} {'n':>4}  type_ok  key_ok  both_ok")
    print("  " + "-" * 42)
    for stype in sorted(by_type):
        items = by_type[stype]
        nd    = len(items)
        tc    = sum(1 for r in items if r["type_ok"])
        kc    = sum(1 for r in items if r["key_ok"])
        bc    = sum(1 for r in items if r["type_ok"] and r["key_ok"])
        print(f"  {stype:<12} {nd:>4}  {tc}/{nd:<5}  {kc}/{nd:<5}  {bc}/{nd}")

    failures = [r for r in rows if not (r["type_ok"] and r["key_ok"])]
    if failures:
        print(f"\n  Failures ({len(failures)}):")
        print(f"  {'id':>6}  {'diff':<12}  {'exp':9} → {'act':9}  {'exp_key':10} → act_key    method")
        print("  " + "-" * 78)
        for r in failures:
            print(
                f"  {str(r['seed_id']):>6}  {r['difficulty']:<12}  "
                f"{r['exp_type']:9} → {r['act_type']:9}  "
                f"{str(r['exp_key'] or '—'):10} → {str(r['act_key'] or '—'):10}  {r['method']}"
            )


# ── Main ──────────────────────────────────────────────────────────────────────
t_start  = time.time()
all_rows: list[dict] = []

formats = ["golden", "week"] if args.format == "both" else [args.format]

for fmt in formats:
    if fmt == "golden":
        p = Path(args.golden)
        if not p.exists():
            print(f"ERROR: {p} not found", file=sys.stderr); sys.exit(1)
        rows = run_golden(p)
        summarise(rows, p.name)
        all_rows += rows
    else:
        wp = Path(args.week)
        tp = Path(args.tickets)
        if not wp.exists():
            print(f"ERROR: {wp} not found", file=sys.stderr); sys.exit(1)
        if not tp.exists():
            print(f"ERROR: {tp} not found", file=sys.stderr); sys.exit(1)
        rows = run_week(wp, tp)
        summarise(rows, wp.name)
        all_rows += rows

if args.format == "both" and all_rows:
    summarise(all_rows, "COMBINED (both datasets)")

print(f"\nTotal elapsed: {time.time()-t_start:.0f}s  |  peak mem {mx.get_peak_memory()/1e9:.2f} GB")

# ── Save ──────────────────────────────────────────────────────────────────────
OUT.mkdir(exist_ok=True)
ts       = time.strftime("%Y%m%dT%H%M%S")
out_path = OUT / f"reranker_classify_{args.format}_{ts}.json"
out_path.write_text(json.dumps({
    "model": REPO, "format": args.format,
    "thresholds": {"high": THR_HIGH, "mid": THR_MID, "margin": MARGIN_MIN},
    "chunk_size": CHUNK, "prefilter": args.prefilter,
    "per_seed": all_rows,
}, indent=2))
print(f"Results → {out_path.relative_to(ROOT.parent.parent.parent)}")
