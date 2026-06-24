"""Direct in-process eval of Qwen3.5-9B-OptiQ-4bit on the week golden dataset.

Uses the EXACT same system prompt, SKILL.md, build_user_message(), and
outlines FSM-constrained decoding as the production MLX server — no HTTP.

Usage:
  cd /Users/adityaharish/Documents/Meridiona/meridian
  services/.venv/bin/python services/tests/evals/rerank/eval_classify_week.py
"""
from __future__ import annotations
import json, os, sys, time
from pathlib import Path

# ── path setup ────────────────────────────────────────────────────────────────
_RERANK_DIR = Path(__file__).parent
_EVALS_DIR = _RERANK_DIR.parent
_SERVICES_DIR = _EVALS_DIR.parent.parent
_DATA_DIR = _RERANK_DIR / "data"

sys.path.insert(0, str(_SERVICES_DIR))  # gives us agents.*
sys.path.insert(0, str(_DATA_DIR))      # gives us labels_week

MODEL_ID = "mlx-community/Qwen3.5-9B-OptiQ-4bit"
MAX_TOKENS = 1024

# ── load production prompts ────────────────────────────────────────────────────
from agents._system_context import SYSTEM_CONTEXT
from agents._prompts import build_user_message
from agents.mlx_classifier import SessionClassification, _SKILL_PATH

try:
    _skill = _SKILL_PATH.read_text(encoding="utf-8")
except OSError:
    _skill = ""
    print("WARNING: SKILL.md not found — system prompt will be incomplete", flush=True)

SYSTEM_PROMPT = SYSTEM_CONTEXT + ("\n\n---\n\n" + _skill if _skill else "")
print(f"System prompt: {len(SYSTEM_PROMPT)} chars, skill loaded: {bool(_skill)}", flush=True)

# ── load dataset ──────────────────────────────────────────────────────────────
from labels_week import L as LABELS

sessions_raw = json.loads((_DATA_DIR / "sessions_week.json").read_text())
tickets_raw  = json.loads((_DATA_DIR / "tickets.json").read_text())
# tickets.json is {key: {...}} dict
tickets_map = tickets_raw if isinstance(tickets_raw, dict) else {t["task_key"]: t for t in tickets_raw}

print(f"Sessions: {len(sessions_raw)}  |  Tickets in pool: {len(tickets_map)}", flush=True)

# ── build task dicts for build_user_message ────────────────────────────────────
def _make_task_dict(key: str) -> dict:
    t = tickets_map.get(key, {})
    return {
        "task_key": key,
        "title": t.get("title", key),
        "description_text": t.get("description_text", ""),
        "issue_type": t.get("issue_type", ""),
        "epic_title": t.get("epic_title", ""),
        "sprint_name": "",
        "tags": "",
        "is_today_focus": False,  # no plan-boost in eval — pure candidate match
    }


def _build_session_dict(s: dict) -> dict:
    """Wrap the week session into the format build_user_message expects."""
    return {
        "id": s["id"],
        "app_name": "Claude Code",
        "started_at": s.get("started", ""),
        "ended_at": "",
        "duration_s": (s.get("min") or 0) * 60,
        "session_text": s.get("session_summary", ""),
        "session_text_source": "claude_jsonl",
        "window_titles": [],
        "category": None,
        "confidence": 0.0,
        "audio_snippets": [],
    }


# ── load model + FSM ──────────────────────────────────────────────────────────
print(f"\nLoading {MODEL_ID} ...", flush=True)
import mlx.core as mx

import mlx_lm
_mlx_model, _tok = mlx_lm.load(MODEL_ID)
print(f"Active mem: {mx.get_active_memory()/1e9:.2f} GB", flush=True)

import outlines
from outlines.generator import Generator

print("Compiling FSM (one-time, ~6s) ...", flush=True)
_outlines_model = outlines.from_mlxlm(_mlx_model, _tok)
_gen = Generator(_outlines_model, SessionClassification)
print("FSM ready.", flush=True)


def classify_one(session_dict: dict, candidate_keys: list[str]) -> SessionClassification | None:
    candidates = [_make_task_dict(k) for k in candidate_keys]
    user_msg = build_user_message(session_dict, candidates, recent_activity=[], now_iso=None)

    # Build chat-formatted prompt identical to the server
    messages = [
        {"role": "system", "content": SYSTEM_PROMPT},
        {"role": "user",   "content": user_msg},
    ]
    prompt_str = _tok.apply_chat_template(
        messages, tokenize=False, add_generation_prompt=True
    )
    try:
        result = _gen(prompt_str, max_tokens=MAX_TOKENS)
        # outlines returns a SessionClassification instance directly
        if isinstance(result, str):
            return SessionClassification.model_validate_json(result)
        return result
    except Exception as e:
        print(f"  classify error: {e}", flush=True)
        return None


# ── eval loop ─────────────────────────────────────────────────────────────────
print(f"\nRunning eval on {len(sessions_raw)} sessions ...\n", flush=True)
t0 = time.time()

hit = fb = rmiss = 0
by_type: dict[str, list[int]] = {}
misses = []
uncertain_hit = uncertain_tot = 0

for i, s in enumerate(sessions_raw):
    sid = s["id"]
    label = LABELS.get(sid)
    if label is None:
        print(f"  SKIP {sid} — no label", flush=True)
        continue
    primary, okset, unc, stype, note = label
    truth = primary  # "NONE" or "KAN-xxx"
    cands = s["candidates"]  # pre-computed per-day working set

    sess_dict = _build_session_dict(s)
    result = classify_one(sess_dict, cands)

    if result is None:
        pred_key = None
    else:
        pred_key = result.task_key  # None or "KAN-xxx"

    # score: pred must be in acceptable set; NONE sessions accept null or "NONE"
    acceptable = set(okset)
    if truth == "NONE":
        acceptable.add("NONE")
        acceptable.add(None)

    pred_label = pred_key if pred_key else "NONE"
    ok = (pred_key in acceptable) or (pred_label in acceptable)
    hit += ok

    if unc:
        uncertain_tot += 1
        uncertain_hit += ok

    by_type.setdefault(stype, [0, 0])
    by_type[stype][1] += 1
    if ok:
        by_type[stype][0] += 1

    if not ok:
        conf_str = f"{result.confidence:.2f}" if result else "?"
        misses.append((sid, stype, truth, pred_label, conf_str, note[:55]))
        if truth == "NONE" and pred_key:
            fb += 1
        if truth != "NONE" and not pred_key:
            rmiss += 1

    elapsed = time.time() - t0
    bar = "█" * (i + 1) + "░" * (len(sessions_raw) - i - 1)
    print(
        f"\r[{i+1:3d}/{len(sessions_raw)}] {bar[:30]} {elapsed:5.0f}s  hit={hit}",
        end="", flush=True,
    )

print()

# ── report ─────────────────────────────────────────────────────────────────────
n = len(sessions_raw)
peak_gb = mx.get_peak_memory() / 1e9
elapsed = time.time() - t0

print(f"\n{'='*60}")
print(f"MODEL: {MODEL_ID}  |  week dataset ({n} sessions)")
print(f"Overall: {hit}/{n} = {hit/n:.1%}")
print(f"Confident (non-uncertain): {hit-uncertain_hit}/{n-uncertain_tot} = {(hit-uncertain_hit)/(n-uncertain_tot):.1%}")
print(f"False-binds (NONE→task):   {fb}")
print(f"Recall-misses (task→NONE): {rmiss}")
print(f"Time: {elapsed:.0f}s  |  Peak RAM: {peak_gb:.2f} GB")

print(f"\nPer session_type:")
for t, (h, tot) in sorted(by_type.items()):
    print(f"  {t:10s}: {h}/{tot} = {h/tot:.0%}")

if misses:
    print(f"\nMisses ({len(misses)}):")
    for sid, st, truth, pred, conf, note in misses:
        print(f"  {sid} [{st}] truth={truth} pred={pred} conf={conf}  {note}")

# ── save ──────────────────────────────────────────────────────────────────────
results_dir = _EVALS_DIR / "rerank" / "results"
results_dir.mkdir(exist_ok=True)
out = {
    "model": MODEL_ID, "dataset": "week", "n": n,
    "hit": hit, "acc": round(hit/n, 4),
    "false_binds": fb, "recall_misses": rmiss,
    "by_type": {t: {"hit": h, "total": tot} for t, (h, tot) in by_type.items()},
    "peak_gb": round(peak_gb, 2), "elapsed_s": round(elapsed, 1),
}
out_path = results_dir / "classify_qwen35_9b_week.json"
out_path.write_text(json.dumps(out, indent=2))
print(f"\nresults → {out_path.relative_to(_RERANK_DIR.parent.parent.parent)}")
