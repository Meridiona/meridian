"""Render hand-authored seed sessions into the deepeval Golden format.

Bridge between golden_seed/dev_<persona>_sessions.json (structured + ground truth)
and .synthetic-dataset-<persona>.json (the deepeval input/expected_output shape
that test_mlx_classifier.py consumes).

Each scoreable session in the seed file becomes one Golden. The recent-sessions
block in the rendered prompt is built from the last 5 SCOREABLE prior sessions
(matches build_dataset.py:_fetch_recent's filter — sub-scoreable sessions are
treated like duration_s <= 1 / empty session_text and excluded).

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
SEED_DIR = EVAL_DIR / "golden_seed"
PERSONA_FILES = {
    "a_meridian": {"sessions": "dev_a_sessions.json",         "candidates": "candidates_meridian.json"},
    "b_generic":  {"sessions": "dev_b_generic_sessions.json", "candidates": "candidates_generic.json"},
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
    sessions_path   = SEED_DIR / PERSONA_FILES[persona]["sessions"]
    candidates_path = SEED_DIR / PERSONA_FILES[persona]["candidates"]
    if not sessions_path.exists():
        raise FileNotFoundError(f"missing seed file: {sessions_path}")
    if not candidates_path.exists():
        raise FileNotFoundError(f"missing candidates file: {candidates_path}")

    sessions   = json.loads(sessions_path.read_text())["sessions"]
    _candidates_raw = json.loads(candidates_path.read_text())
    candidates = _candidates_raw.get("tasks") or _candidates_raw.get("tickets", [])
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
    output_path = EVAL_DIR / f".synthetic-dataset-{persona}.json"
    output_path.write_text(json.dumps(goldens, indent=2, ensure_ascii=False))

    print(f"Rendered {len(goldens)} scoreable Goldens → {output_path}")
    print()
    print("Difficulty distribution:")
    counts = Counter(g["additional_metadata"]["difficulty"] for g in goldens)
    for tier, n in sorted(counts.items()):
        print(f"  {tier:<14} {n}")


if __name__ == "__main__":
    main()
