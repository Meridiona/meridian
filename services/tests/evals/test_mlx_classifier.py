"""Eval suite for the MLX direct in-process classifier (run_task_linker_mlx.py).

MLX is a direct LLM call — not an agent SDK — so evaluation follows the
DeepEval 4.0 component-level pattern: build LLMTestCase objects from goldens,
then assert_test (per-case) or evaluate() (batch report).

Run (from services/, with .venv313 active):

    # Smoke tests only — no model load:
    pytest tests/evals/test_mlx_classifier.py -m "integration and not slow"

    # End-to-end eval — all goldens in one DeepEval report:
    MLX_SERVER_URL=http://localhost:7823 \
        .venv313/bin/deepeval test run tests/evals/test_mlx_classifier.py \
        -k "test_mlx_e2e" --identifier "mlx-baseline" --ignore-errors

    # Per-golden breakdown — individual pass/fail per session:
    MLX_SERVER_URL=http://localhost:7823 \
        .venv313/bin/deepeval test run tests/evals/test_mlx_classifier.py \
        -k "test_mlx_per_golden" --identifier "mlx-per-golden" --ignore-errors

    # Manual accuracy table (no deepeval runner needed):
    MLX_SERVER_URL=http://localhost:7823 \
        .venv313/bin/python3.13 tests/evals/test_mlx_classifier.py

Marks:
    integration — harness + metric wiring tests, no model call
    slow        — loads the MLX model; requires Apple Silicon + .venv313
"""
from __future__ import annotations

import json
import os
import sys
import time
from pathlib import Path
from typing import Iterator

import pytest

from deepeval import assert_test, evaluate
from deepeval.dataset import EvaluationDataset, Golden, get_current_golden
from deepeval.test_case import LLMTestCase
from deepeval.tracing import observe, update_current_span, update_current_trace

from metrics import CLASSIFIER_METRICS, TaskKeyMatchMetric, SessionTypeMatchMetric
from strategies import from_env as strategy_from_env

# ---------------------------------------------------------------------------
# Path / env setup
# ---------------------------------------------------------------------------

_SERVICES_DIR = Path(__file__).parent.parent.parent
if str(_SERVICES_DIR) not in sys.path:
    sys.path.insert(0, str(_SERVICES_DIR))

os.environ.setdefault("HERMES_HOME", str(_SERVICES_DIR / ".hermes"))

# ---------------------------------------------------------------------------
# Dataset — goldens loaded from .dataset.json
# expected_output is a JSON string: {"task_key": ..., "session_type": ..., "reasoning": ...}
# ---------------------------------------------------------------------------

# Dataset path is configurable via EVAL_DATASET_PATH — defaults to .dataset.json
# (the real-pulled goldens from build_dataset.py). Point at .synthetic-dataset-<persona>.json
# to run on the hand-authored seed sessions rendered by render_seeds.py.
_DATASET_PATH = Path(
    os.environ.get("EVAL_DATASET_PATH")
    or (Path(__file__).parent / ".dataset.json")
)
dataset = EvaluationDataset()
dataset.add_goldens_from_json_file(file_path=str(_DATASET_PATH))

# ---------------------------------------------------------------------------
# MLX runner — calls the model directly with the prompt from golden.input.
# Returns a JSON string matching the expected_output shape.
# ---------------------------------------------------------------------------

_MLX_MODEL_LABEL = os.environ.get("MLX_MODEL_ID", "Qwen3.5-9B-OptiQ-4bit")

# Strategy selected via EVAL_STRATEGY env var (default: direct_http).
# Built once at module level so all test cases share the same strategy instance.
_strategy = strategy_from_env()


@observe(type="llm", model=_MLX_MODEL_LABEL, name="mlx_classify")
def _run_mlx(prompt_input: str) -> str:
    """Classify one session; return JSON actual_output.

    Decorated with @observe so every call appears as a typed LLM span in the
    DeepEval trace tree. update_current_span/trace attach the test case data
    so metrics render inline in the trace view.

    Uses _strategy (set by EVAL_STRATEGY env var) to generate actual_output.
    Default strategy is DirectHttpStrategy (POST to MLX_SERVER_URL/classify).
    For in-process inference without a server, set EVAL_STRATEGY=direct_mlx
    once that strategy is implemented (Task #9 extension).
    """
    result = _strategy.classify_prompt(prompt_input)
    result_json = result.as_actual_output()

    # Attach test case data to the span and trace so DeepEval can render
    # per-call metrics in the trace tree. Include system prompt so it's
    # visible in Confident AI traces alongside the user message.
    from agents.run_task_linker_mlx import _SYSTEM_PROMPT as _sys
    full_input = json.dumps([
        {"role": "system", "content": _sys},
        {"role": "user",   "content": prompt_input},
    ], ensure_ascii=False)
    golden = get_current_golden()
    expected = golden.expected_output if golden else None
    update_current_span(input=full_input, output=result_json, expected_output=expected)
    update_current_trace(input=full_input, output=result_json, expected_output=expected)

    return result_json


# ---------------------------------------------------------------------------
# Test-case builder
#
# DeepEval 4.0 docs pattern: build LLMTestCase objects from goldens at module
# level, then @pytest.mark.parametrize on dataset.test_cases.
#
# When MLX_SERVER_URL is set the model is already running — _run_mlx() is just
# an HTTP call so we can build eagerly at module level, matching the docs exactly.
#
# Without a server, in-process model load is expensive and must not happen at
# import/collect time, so we fall back to a session-scoped fixture.
# ---------------------------------------------------------------------------

def _build_test_cases() -> list[LLMTestCase]:
    """Run MLX on every golden and populate dataset.test_cases. Idempotent."""
    if dataset.test_cases:
        return dataset.test_cases

    for golden in dataset.goldens:
        t0 = time.time()
        actual = _run_mlx(golden.input)
        elapsed = time.time() - t0
        dataset.add_test_case(LLMTestCase(
            input=golden.input,
            actual_output=actual,
            expected_output=golden.expected_output,
            additional_metadata={
                **(golden.additional_metadata or {}),
                "elapsed_s": round(elapsed, 2),
            },
        ))

    return dataset.test_cases


# Build eagerly at module level when the server is available (docs pattern).
# Parametrize on dataset.test_cases will be non-empty at collection time.
if os.environ.get("MLX_SERVER_URL"):
    _build_test_cases()


# ---------------------------------------------------------------------------
# Session-scoped fixture — used when MLX_SERVER_URL is not set (in-process)
# ---------------------------------------------------------------------------

@pytest.fixture(scope="session")
def mlx_test_cases(tmp_path_factory: pytest.TempPathFactory) -> list[LLMTestCase]:
    """Warm the in-process model once and build test cases for all goldens."""
    if not os.environ.get("MLX_SERVER_URL"):
        import agents.run_task_linker_mlx as m
        m._get_model()
    return _build_test_cases()


# ---------------------------------------------------------------------------
# Smoke tests — no model load
# ---------------------------------------------------------------------------

@pytest.mark.integration
def test_dataset_loads() -> None:
    """Dataset file exists and has at least one golden."""
    assert len(dataset.goldens) > 0, (
        "tests/evals/.dataset.json is empty. "
        "Run tests/evals/build_dataset.py to populate it from meridian.db."
    )


@pytest.mark.integration
def test_metric_task_key_match() -> None:
    """TaskKeyMatchMetric: pass on match, fail on mismatch."""
    metric = TaskKeyMatchMetric()

    exp = json.dumps({"task_key": "KAN-109", "session_type": "task", "reasoning": ""})
    metric.measure(LLMTestCase(input="x", actual_output=exp, expected_output=exp))
    assert metric.is_successful()

    act_miss = json.dumps({"task_key": "KAN-107", "session_type": "task", "reasoning": ""})
    metric.measure(LLMTestCase(input="x", actual_output=act_miss, expected_output=exp))
    assert not metric.is_successful()


@pytest.mark.integration
def test_metric_null_equivalence() -> None:
    """None, 'none', 'null', 'n/a', '' are all equivalent null task_keys."""
    metric = TaskKeyMatchMetric()
    exp = json.dumps({"task_key": "none", "session_type": "overhead", "reasoning": ""})
    for null_val in ["none", "null", "n/a", ""]:
        act = json.dumps({"task_key": null_val, "session_type": "overhead", "reasoning": ""})
        metric.measure(LLMTestCase(input="x", actual_output=act, expected_output=exp))
        assert metric.is_successful(), f"null equivalence failed for {null_val!r}"


@pytest.mark.integration
def test_metric_session_type() -> None:
    """SessionTypeMatchMetric: pass on match, fail on mismatch."""
    metric = SessionTypeMatchMetric()

    exp = json.dumps({"task_key": "none", "session_type": "overhead", "reasoning": ""})
    metric.measure(LLMTestCase(input="x", actual_output=exp, expected_output=exp))
    assert metric.is_successful()

    act_miss = json.dumps({"task_key": "none", "session_type": "untracked", "reasoning": ""})
    metric.measure(LLMTestCase(input="x", actual_output=act_miss, expected_output=exp))
    assert not metric.is_successful()


@pytest.mark.integration
def test_session_classification_schema() -> None:
    """SessionClassification Pydantic schema has all required fields."""
    from agents.run_task_linker_mlx import SessionClassification
    fields = SessionClassification.model_fields
    for f in ("task_key", "confidence", "session_type", "reasoning", "dimensions"):
        assert f in fields, f"missing field: {f}"


@pytest.mark.integration
def test_system_prompt_not_empty() -> None:
    """_SYSTEM_PROMPT contains SYSTEM_CONTEXT + SKILL.md content."""
    from agents.run_task_linker_mlx import _SYSTEM_PROMPT
    assert len(_SYSTEM_PROMPT) > 100, "system prompt looks empty"


# ---------------------------------------------------------------------------
# End-to-end eval — DeepEval 4.0 evaluate() pattern
# Runs all goldens through the model in one shot, produces a full report.
# Use: deepeval test run ... -k test_mlx_e2e
# ---------------------------------------------------------------------------

@pytest.mark.slow
@pytest.mark.integration
def test_mlx_e2e(mlx_test_cases: list[LLMTestCase]) -> None:
    """End-to-end: classify all goldens and evaluate with DeepEval evaluate()."""
    evaluate(
        test_cases=mlx_test_cases,
        metrics=CLASSIFIER_METRICS,
        hyperparameters=_strategy.as_hyperparameters(),
        identifier=f"mlx-{_strategy.name}",
    )


# ---------------------------------------------------------------------------
# Per-golden breakdown — individual assert_test per case for CI granularity
# DeepEval 4.0: parametrize on dataset.test_cases (pre-built), not goldens.
# ---------------------------------------------------------------------------

@pytest.mark.slow
@pytest.mark.integration
@pytest.mark.parametrize("test_case", dataset.test_cases)  # non-empty when MLX_SERVER_URL set
def test_mlx_per_golden(test_case: LLMTestCase, mlx_test_cases: list[LLMTestCase]) -> None:
    """Assert each pre-built test case passes both metrics (docs parametrize pattern).

    mlx_test_cases fixture ensures test cases are built before this runs
    when MLX_SERVER_URL is not set (in-process fallback path).
    """
    assert_test(test_case=test_case, metrics=CLASSIFIER_METRICS)


# ---------------------------------------------------------------------------
# Manual accuracy table — run directly without deepeval runner
# ---------------------------------------------------------------------------

if __name__ == "__main__":
    print("Loading MLX model…")
    import agents.run_task_linker_mlx as _m
    _m._get_model()
    print("Model ready.\n")

    key_metric  = TaskKeyMatchMetric()
    type_metric = SessionTypeMatchMetric()
    rows: list[dict] = []

    for golden in dataset.goldens:
        t0 = time.time()
        try:
            actual = _run_mlx(golden.input)
            error = None
        except Exception as exc:
            actual = json.dumps({"task_key": "none", "session_type": "overhead", "reasoning": ""})
            error = str(exc)
        elapsed = time.time() - t0

        case = LLMTestCase(input=golden.input, actual_output=actual, expected_output=golden.expected_output)
        key_metric.measure(case)
        type_metric.measure(case)

        exp  = json.loads(golden.expected_output) if golden.expected_output else {}
        act  = json.loads(actual)
        meta = golden.additional_metadata or {}
        rows.append({
            "session_id": meta.get("session_id"),
            "app_name":   meta.get("app_name"),
            "exp_key":    exp.get("task_key", "none"),
            "act_key":    act.get("task_key", "none"),
            "exp_type":   exp.get("session_type", ""),
            "act_type":   act.get("session_type", ""),
            "key_ok":     key_metric.is_successful(),
            "type_ok":    type_metric.is_successful(),
            "elapsed_s":  round(elapsed, 2),
            "error":      error,
        })

    key_correct  = sum(1 for r in rows if r["key_ok"])
    type_correct = sum(1 for r in rows if r["type_ok"])
    total = len(rows)

    print(f"{'SID':<8} {'App':<14} {'ExpKey':<10} {'ActKey':<10} {'K':<2} {'ExpType':<12} {'ActType':<12} {'T':<2} {'s':>5}")
    print("-" * 80)
    for r in rows:
        k = "✓" if r["key_ok"] else "✗"
        t = "✓" if r["type_ok"] else "✗"
        print(
            f"{str(r['session_id']):<8} {str(r['app_name']):<14} "
            f"{r['exp_key']:<10} {r['act_key']:<10} {k:<2} "
            f"{r['exp_type']:<12} {r['act_type']:<12} {t:<2} {r['elapsed_s']:>5.1f}"
        )
    print("-" * 80)
    if total:
        print(f"task_key accuracy:     {key_correct}/{total} = {key_correct/total:.0%}")
        print(f"session_type accuracy: {type_correct}/{total} = {type_correct/total:.0%}")
