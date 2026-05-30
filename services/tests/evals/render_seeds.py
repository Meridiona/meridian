"""Render hand-authored seed sessions into the deepeval Golden format.

Reads data/seeds/sessions_<persona>.json + data/seeds/tickets_<persona>.json
and writes data/generated/goldens_<persona>.json — the deepeval Golden shape
consumed by eval_classifier.py and test_classifier.py.

Each scoreable session becomes one Golden. The recent-sessions block in the
rendered prompt is built from the last 5 SCOREABLE prior sessions (matches
build_dataset.py's filter — non-scoreable sessions are excluded).

Usage:
    python services/tests/evals/render_seeds.py [persona]

    persona defaults to 'a_meridian'; valid: a_meridian, b_generic
"""
from __future__ import annotations

import json
import sys
from collections import Counter
from pathlib import Path

_SERVICES_DIR = Path(__file__).parent.parent.parent
if str(_SERVICES_DIR) not in sys.path:
    sys.path.insert(0, str(_SERVICES_DIR))

from agents._prompts import build_user_message  # noqa: E402

EVAL_DIR = Path(__file__).parent
SEED_DIR = EVAL_DIR / "data" / "seeds"
PERSONA_FILES = {
    "a_meridian": {"sessions": "sessions_a_meridian.json", "tickets": "tickets_meridian.json"},
    "b_generic":  {"sessions": "sessions_b_generic.json",  "tickets": "tickets_generic.json"},
}


def _project_recent(prior: list[dict]) -> list[dict]:
    """Project a list of prior seed sessions into the shape build_user_message wants."""
    out = []
    for s in prior:
        gt = s.get("ground_truth", {})
        tk = gt.get("task_key")
        task_key = tk if tk and tk != "none" else None
        out.append({
            "app_name":     s["app_name"],
            "started_at":   s["started_at"],
            "duration_s":   s["duration_s"],
            "task_key":     task_key,
            "task_routing": "auto" if task_key else None,
            "category":     s.get("category", ""),
        })
    return out


def render(persona: str) -> list[dict]:
    if persona not in PERSONA_FILES:
        raise ValueError(f"unknown persona {persona!r}, valid: {list(PERSONA_FILES)}")
    sessions_path = SEED_DIR / PERSONA_FILES[persona]["sessions"]
    tickets_path  = SEED_DIR / PERSONA_FILES[persona]["tickets"]
    if not sessions_path.exists():
        raise FileNotFoundError(f"missing seed file: {sessions_path}")
    if not tickets_path.exists():
        raise FileNotFoundError(f"missing tickets file: {tickets_path}")

    sessions        = json.loads(sessions_path.read_text())["sessions"]
    _tickets_raw    = json.loads(tickets_path.read_text())
    candidates = _tickets_raw.get("tasks") or _tickets_raw.get("tickets", [])
    # Normalise: some candidate files use 'id' instead of 'task_key'
    candidates = [
        {**c, "task_key": c["task_key"]} if "task_key" in c else {**c, "task_key": c["id"]}
        for c in candidates
    ]

    goldens: list[dict] = []
    scoreable_prior: list[dict] = []

    for s in sessions:
        gt = s.get("ground_truth", {})
        if not gt.get("scoreable"):
            continue

        recent = _project_recent(scoreable_prior[-5:])
        prompt = build_user_message(s, candidates, recent_sessions=recent)

        expected = {
            "task_key":     gt.get("task_key", "none"),
            "session_type": gt.get("session_type", "overhead"),
            "reasoning":    gt.get("reasoning", ""),
        }
        goldens.append({
            "input": prompt,
            "expected_output": json.dumps(expected, ensure_ascii=False),
            "additional_metadata": {
                "seed_id":    s["id"],
                "app_name":   s["app_name"],
                "difficulty": gt.get("difficulty", "unknown"),
                "persona":    persona,
            },
        })
        scoreable_prior.append(s)

    return goldens


def main() -> None:
    persona = sys.argv[1] if len(sys.argv) > 1 else "a_meridian"
    goldens = render(persona)
    generated_dir = EVAL_DIR / "data" / "generated"
    generated_dir.mkdir(parents=True, exist_ok=True)
    output_path = generated_dir / f"goldens_{persona}.json"
    output_path.write_text(json.dumps(goldens, indent=2, ensure_ascii=False))

    print(f"Rendered {len(goldens)} scoreable Goldens → {output_path}")
    print()
    print("Difficulty distribution:")
    counts = Counter(g["additional_metadata"]["difficulty"] for g in goldens)
    for tier, n in sorted(counts.items()):
        print(f"  {tier:<14} {n}")


if __name__ == "__main__":
    main()
