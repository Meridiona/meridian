"""Correctness gate for the classifier prompt-cache.

The MLX classifier reuses the cached system+skill prefix across sessions
(run_task_linker_mlx._generate_constrained). Greedy decoding (temperature 0)
means a correct cache MUST yield byte-identical output to the uncached path — a
mismatch is silent classification corruption, the one failure mode that matters.

This test classifies real sessions both ways and asserts the verdict + prose are
identical. It loads the 9B MLX model in-process (~7 GB, a few seconds), so it is
NOT part of the fast CI suite — run it manually after touching the cache logic:

    services/.venv/bin/python services/tests/test_prompt_cache_equivalence.py [SID ...]

It auto-skips (exit 0) when MLX or the model can't be loaded (non-Apple-Silicon
CI), or when no suitable sessions are found in the local DB.
"""
from __future__ import annotations

import os
import sqlite3
import sys
from pathlib import Path

_SERVICES_DIR = Path(__file__).resolve().parent.parent
if str(_SERVICES_DIR) not in sys.path:
    sys.path.insert(0, str(_SERVICES_DIR))

_DB = os.path.expanduser(os.environ.get("MERIDIAN_DB", "~/.meridian/meridian.db"))

# Fields that must be byte-identical between cached and uncached classification.
_COMPARE = ("reasoning", "task_key", "confidence", "session_type", "category", "session_summary")


def _default_session_ids(con: sqlite3.Connection) -> list[int]:
    rows = con.execute(
        "SELECT id FROM app_sessions"
        " WHERE LENGTH(COALESCE(session_text,'')) > 800"
        "   AND coding_agent_session_uuid IS NULL"
        " ORDER BY id DESC LIMIT 3"
    ).fetchall()
    return [r[0] for r in rows]


def _verdict(result: dict) -> dict:
    return {k: result.get(k) for k in _COMPARE}


def main(argv: list[str]) -> int:
    if not Path(_DB).exists():
        print(f"SKIP: meridian DB not found at {_DB}")
        return 0
    try:
        import mlx_lm  # noqa: F401
        import agents.run_task_linker_mlx as rtl
    except Exception as exc:  # noqa: BLE001
        print(f"SKIP: MLX unavailable ({exc})")
        return 0

    con = sqlite3.connect(f"file:{_DB}?mode=ro", uri=True)
    con.row_factory = sqlite3.Row

    sids = [int(a) for a in argv] or _default_session_ids(con)
    if not sids:
        print("SKIP: no suitable sessions in DB")
        return 0
    print(f"Comparing cached vs uncached classification for sessions: {sids}")

    # Baseline — cache disabled, fresh every call.
    rtl._PROMPT_CACHE_ENABLED = False
    rtl._invalidate_prompt_cache()
    baseline = {sid: _verdict(rtl._classify_one(sid, con)) for sid in sids}

    # Cached — cold prime on the first session, warm hits afterwards. Re-run the
    # first session last to confirm a warm hit on an already-seen prefix matches.
    rtl._PROMPT_CACHE_ENABLED = True
    rtl._invalidate_prompt_cache()
    cached: dict[int, dict] = {}
    for sid in [*sids, sids[0]]:
        cached[sid] = _verdict(rtl._classify_one(sid, con))

    failures = 0
    for sid in sids:
        if cached[sid] == baseline[sid]:
            print(f"  PASS  session {sid}: cached == uncached")
        else:
            failures += 1
            print(f"  FAIL  session {sid}: cached != uncached")
            for k in _COMPARE:
                if cached[sid].get(k) != baseline[sid].get(k):
                    print(f"        {k}:\n          uncached={baseline[sid].get(k)!r}\n          cached  ={cached[sid].get(k)!r}")

    print(f"\n{len(sids) - failures}/{len(sids)} sessions identical")
    return 0 if failures == 0 else 1


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
