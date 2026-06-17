"""One-off in-process accuracy check for the rewritten classifier.

Runs the goldens through the CURRENT in-process code (new system prompt + new
reasoning-first SessionClassification schema), bypassing the running HTTP server
so we measure exactly what's on disk. Scores task_key + session_type vs the
golden's expected_output. Not part of CI — a manual validation aid.

    services/.venv/bin/python services/tests/evals/_inprocess_accuracy.py [goldens.json]
"""
from __future__ import annotations

import json
import sys
from pathlib import Path

_SERVICES_DIR = Path(__file__).resolve().parents[2]
if str(_SERVICES_DIR) not in sys.path:
    sys.path.insert(0, str(_SERVICES_DIR))

import agents.run_task_linker_mlx as m  # noqa: E402


def _classify(user_message: str) -> m.SessionClassification:
    from mlx_lm.sample_utils import make_sampler
    from outlines.inputs import Chat

    messages = [
        {"role": "system", "content": m._SYSTEM_PROMPT},
        {"role": "user", "content": user_message},
    ]
    with m.model_session() as model:
        raw = model(
            Chat(messages),
            output_type=m.SessionClassification,
            max_tokens=m._MAX_TOKENS,
            sampler=make_sampler(temp=m._TEMPERATURE),
            verbose=False,
        )
    return m.SessionClassification.model_validate_json(raw)


def main(argv: list[str]) -> int:
    path = Path(argv[0]) if argv else (
        _SERVICES_DIR / "tests/evals/data/generated/goldens_a_meridian.json"
    )
    goldens = json.loads(path.read_text())
    print(f"Scoring {len(goldens)} goldens from {path.name}\n")

    task_ok = type_ok = both_ok = 0
    fails: list[str] = []
    for i, g in enumerate(goldens, 1):
        exp = json.loads(g["expected_output"])
        exp_key = exp.get("task_key") or None
        if exp_key == "none":
            exp_key = None
        exp_type = exp.get("session_type", "overhead")
        meta = g.get("additional_metadata", {})
        try:
            out = _classify(g["input"])
        except Exception as exc:  # noqa: BLE001
            fails.append(f"#{i} {meta.get('seed_id')} EXC {exc}")
            continue
        got_key = out.task_key or None
        k_ok = got_key == exp_key
        t_ok = out.session_type == exp_type
        task_ok += k_ok
        type_ok += t_ok
        both_ok += (k_ok and t_ok)
        flag = "ok " if (k_ok and t_ok) else "DIFF"
        if not (k_ok and t_ok):
            fails.append(
                f"#{i} seed={meta.get('seed_id')} diff={meta.get('difficulty')} app={meta.get('app_name')}: "
                f"key exp={exp_key!r} got={got_key!r} | type exp={exp_type!r} got={out.session_type!r}"
            )
        print(f"  [{flag}] #{i:2d} key {got_key or '-':<10} (exp {exp_key or '-':<10}) "
              f"type {out.session_type:<9} (exp {exp_type})")

    n = len(goldens)
    print(f"\n  task_key acc:     {task_ok}/{n} = {task_ok/n:.1%}")
    print(f"  session_type acc: {type_ok}/{n} = {type_ok/n:.1%}")
    print(f"  both correct:     {both_ok}/{n} = {both_ok/n:.1%}")
    if fails:
        print("\n  Misses:")
        for f in fails:
            print(f"    {f}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
