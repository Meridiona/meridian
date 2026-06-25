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

import sqlite3  # noqa: E402

from agents._prompts import build_user_message  # noqa: E402
from agents.run_task_linker_mlx import _fetch_recent_ticket_activity  # noqa: E402

EVAL_DIR = Path(__file__).parent
SEED_DIR = EVAL_DIR / "data" / "seeds"
PERSONA_FILES = {
    "a_meridian": {"sessions": "sessions_a_meridian.json", "tickets": "tickets_meridian.json"},
    "b_generic":  {"sessions": "sessions_b_generic.json",  "tickets": "tickets_generic.json"},
}


def _project_recent(
    prior: list[dict], current_started_at: str, candidate_keys: list[str]
) -> list[dict]:
    """Build the per-ticket continuity prior for a seed session, reusing the EXACT
    production aggregation (`_fetch_recent_ticket_activity`) so rendered goldens
    match the live prompt. We load the prior scoreable seeds into a throwaway
    in-memory DB and run the real query against it (windowing, confidence floor,
    candidate-gating, recency ordering all happen there — one source of truth)."""
    con = sqlite3.connect(":memory:")
    con.row_factory = sqlite3.Row
    con.execute(
        "CREATE TABLE app_sessions ("
        " id INTEGER PRIMARY KEY AUTOINCREMENT,"
        " task_key TEXT, started_at TEXT, ended_at TEXT, duration_s REAL,"
        " task_confidence REAL, task_session_type TEXT)"
    )
    for s in prior:
        gt = s.get("ground_truth", {})
        tk = gt.get("task_key")
        task_key = tk if tk and tk != "none" else None
        if not task_key:
            continue  # untracked/overhead priors carry no continuity signal
        con.execute(
            "INSERT INTO app_sessions"
            " (task_key, started_at, ended_at, duration_s, task_confidence, task_session_type)"
            " VALUES (?, ?, ?, ?, 1.0, 'task')",
            (task_key, s["started_at"], s.get("ended_at") or s["started_at"], s["duration_s"]),
        )
    con.commit()
    return _fetch_recent_ticket_activity(con, current_started_at, candidate_keys)


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

        recent = _project_recent(
            scoreable_prior, s["started_at"], [c["task_key"] for c in candidates]
        )
        prompt = build_user_message(
            s, candidates, recent_activity=recent, now_iso=s["started_at"]
        )

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
