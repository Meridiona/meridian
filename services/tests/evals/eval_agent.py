"""Agent evaluation for the Stage 3 session→task classifier using DeepEval 4.0.

Follows the DeepEval AI Agent Evaluation pattern exactly:
  https://deepeval.com/docs/getting-started-agents

Two evaluation levels:
  1. End-to-end (trace): TaskCompletionMetric — did the hermes agent complete
     the session classification task correctly?
  2. Component (span):   TaskKeyMatchMetric  — did it output the right task_key?

Run:
    cd services
    MERIDIAN_DB=~/.meridian/meridian.db python tests/evals/eval_agent.py

Options (env vars):
    MERIDIAN_DB   Path to meridian.db
    OLLAMA_MODEL  Judge + hermes model  (default: gemma4:31b)
    OLLAMA_HOST   Ollama base URL       (default: http://localhost:11434)

Generate / refresh data/generated/goldens_real.json first:
    SESSION_IDS=2276,2354,1961,2181,1792,1972,2514 \\
      MERIDIAN_DB=~/.meridian/meridian.db \\
      python tests/evals/build_dataset.py
"""
from __future__ import annotations

import os
import sqlite3
import sys
import time
from pathlib import Path

# CONFIDENT_TRACE_FLUSH=1 prevents traces from being lost on early exit.
# Must be set before deepeval is imported.
os.environ.setdefault("CONFIDENT_TRACE_FLUSH", "1")

_SERVICES_DIR = Path(__file__).parent.parent.parent
if str(_SERVICES_DIR) not in sys.path:
    sys.path.insert(0, str(_SERVICES_DIR))

os.environ.setdefault("HERMES_HOME", str(_SERVICES_DIR / ".hermes"))

from deepeval.dataset import EvaluationDataset
from deepeval.test_case import LLMTestCase
from deepeval.tracing import observe, update_current_span

from metrics import AGENT_E2E_METRICS, CLASSIFIER_METRICS

MERIDIAN_DB = Path(os.environ.get("MERIDIAN_DB", Path.home() / ".meridian/meridian.db"))
_DATASET_FILE = Path(__file__).parent / "data" / "generated" / "goldens_real.json"


# ---------------------------------------------------------------------------
# Observed agent functions — DeepEval @observe wraps each function as a span.
# Nesting @observe calls creates a trace (outer) → span (inner) hierarchy.
# ---------------------------------------------------------------------------

@observe(metrics=CLASSIFIER_METRICS)
def _classify_session(prompt_input: str, session_id: int, expected_output: str) -> str:
    """Inner component span — runs hermes AIAgent and records the exact-match metric."""
    from agents.run_task_linker import _classify_one

    t0 = time.time()
    con = sqlite3.connect(str(MERIDIAN_DB))
    con.row_factory = sqlite3.Row
    try:
        result = _classify_one(session_id, str(MERIDIAN_DB), con)
    finally:
        con.close()
    elapsed = time.time() - t0

    actual_output = result["task_key"] or "none"

    # update_current_span is required for all metrics except TaskCompletionMetric.
    # Attach the LLMTestCase so TaskKeyMatchMetric can score this span.
    update_current_span(
        test_case=LLMTestCase(
            input=prompt_input,
            actual_output=actual_output,
            expected_output=expected_output,
            completion_time=elapsed,
        )
    )
    return actual_output


@observe(metrics=AGENT_E2E_METRICS)
def run_hermes_agent(prompt_input: str, session_id: int, expected_output: str) -> str:
    """Outer end-to-end trace — TaskCompletionMetric evaluates the full trace.

    TaskCompletionMetric does NOT require update_current_span; it infers the task
    and outcome from the trace automatically.
    """
    return _classify_session(prompt_input, session_id, expected_output)


# ---------------------------------------------------------------------------
# Eval loop — DeepEval evals_iterator() pattern for agents
# ---------------------------------------------------------------------------

def main() -> None:
    if not MERIDIAN_DB.exists():
        print(f"ERROR: meridian.db not found at {MERIDIAN_DB}", file=sys.stderr)
        print("Set MERIDIAN_DB env var to the correct path.", file=sys.stderr)
        sys.exit(1)

    if not _DATASET_FILE.exists():
        print(f"ERROR: dataset not found at {_DATASET_FILE}", file=sys.stderr)
        print("Run tests/evals/build_dataset.py first to generate it.", file=sys.stderr)
        sys.exit(1)

    dataset = EvaluationDataset()
    dataset.add_goldens_from_json_file(file_path=str(_DATASET_FILE))

    if not dataset.goldens:
        print("ERROR: dataset is empty — run build_dataset.py first.", file=sys.stderr)
        sys.exit(1)

    print(f"Loaded {len(dataset.goldens)} goldens from {_DATASET_FILE}")
    print(f"Running hermes agent eval (model: {os.environ.get('OLLAMA_MODEL','gemma4:31b')})\n")

    # evals_iterator() starts a DeepEval test run.
    # Each iteration = one golden = one trace = one test case in the run.
    # Metrics on @observe(metrics=[...]) are evaluated per span/trace automatically.
    for golden in dataset.evals_iterator():
        metadata = golden.additional_metadata or {}
        session_id = metadata.get("session_id")

        if not session_id:
            print(f"  SKIP: golden has no session_id in metadata — {golden.input[:60]!r}")
            continue

        expected = golden.expected_output or "none"
        print(f"  session {session_id} | expected={expected}")

        run_hermes_agent(golden.input, session_id, expected)

    print("\nEval complete.")
    print("To view results locally: deepeval view")


if __name__ == "__main__":
    main()
