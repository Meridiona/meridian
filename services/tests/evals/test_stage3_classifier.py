"""Eval suite for the Stage 3 session→task classifier (KAN-109).

Use case: Agent — hermes AIAgent classifies a session into a Jira ticket key
or routes as overhead/untracked. No native DeepEval integration exists for
hermes, so this uses the no-tracing single-turn template.

Run (from services/):
    deepeval test run tests/evals/test_stage3_classifier.py \\
        --identifier "model-comparison-round-1" \\
        --ignore-errors \\
        --skip-on-missing-params

Dataset: tests/evals/.dataset.json
  Populate with real labeled sessions before running KAN-113/KAN-114.
  See tests/evals/build_dataset.py to generate from meridian.db.

Subtasks:
  KAN-110 — this scaffold
  KAN-111 — build .dataset.json from meridian.db
  KAN-113 — populate CANDIDATE_MODELS and run model comparison
  KAN-114 — populate PROMPT_VARIANTS and run prompt comparison
  KAN-115 — apply winning config
"""
from __future__ import annotations

import os
import sys
import time
from pathlib import Path

import pytest

from deepeval import assert_test
from deepeval.dataset import EvaluationDataset, Golden
from deepeval.test_case import LLMTestCase

from metrics import CLASSIFIER_METRICS

# ---------------------------------------------------------------------------
# Path setup
# ---------------------------------------------------------------------------

_SERVICES_DIR = Path(__file__).parent.parent.parent
if str(_SERVICES_DIR) not in sys.path:
    sys.path.insert(0, str(_SERVICES_DIR))

os.environ.setdefault("HERMES_HOME", str(_SERVICES_DIR / ".hermes"))

# ---------------------------------------------------------------------------
# Dataset — load goldens from .dataset.json (DeepEval Golden format)
# ---------------------------------------------------------------------------

dataset = EvaluationDataset()
dataset.add_goldens_from_json_file(
    file_path=str(Path(__file__).parent / ".dataset.json")
)

# ---------------------------------------------------------------------------
# Models to compare — edit before running KAN-113
# ---------------------------------------------------------------------------

CANDIDATE_MODELS: list[tuple[str, str]] = [
    # (model_name, base_url)
    # ("llama3.1:8b",  "http://localhost:11434"),
    # ("mistral:7b",   "http://localhost:11434"),
    # ("qwen2.5:7b",   "http://localhost:11434"),
    # ("gemma2:9b",    "http://localhost:11434"),
]

# ---------------------------------------------------------------------------
# Prompt variants — edit before running KAN-114
# ---------------------------------------------------------------------------

PROMPT_VARIANTS: list[dict] = [
    # {"name": "no_recent_context", "include_recent": False},
    # {"name": "strict_json_instruction", "output_format": "strict"},
]

# ---------------------------------------------------------------------------
# Classifier runner
# ---------------------------------------------------------------------------

def _classify(model_name: str, base_url: str, prompt_input: str) -> str:
    """Call Ollama directly with the formatted prompt; return extracted task_key."""
    import ollama
    from agents._parser import parse_response
    from agents._system_context import SYSTEM_CONTEXT

    response = ollama.chat(
        model=model_name,
        base_url=base_url,
        messages=[
            {"role": "system", "content": SYSTEM_CONTEXT},
            {"role": "user", "content": prompt_input},
        ],
    )
    raw = response["message"]["content"].strip()
    task_key, _conf, _reason, _dims, _session_type, _err = parse_response(raw, set())
    return task_key or "none"


# ---------------------------------------------------------------------------
# KAN-110: Smoke tests — harness wires up correctly without a real model call
# ---------------------------------------------------------------------------

@pytest.mark.integration
def test_dataset_loads():
    """Dataset file exists and loads at least one golden."""
    assert len(dataset.goldens) > 0, (
        "tests/evals/.dataset.json is empty. "
        "Run tests/evals/build_dataset.py to populate it from meridian.db."
    )


@pytest.mark.integration
def test_metric_exact_match():
    """TaskKeyMatchMetric passes on match, fails on mismatch."""
    from metrics import TaskKeyMatchMetric

    metric = TaskKeyMatchMetric()

    match_case = LLMTestCase(input="x", actual_output="KAN-86", expected_output="KAN-86")
    metric.measure(match_case)
    assert metric.is_successful()

    miss_case = LLMTestCase(input="x", actual_output="KAN-99", expected_output="KAN-86")
    metric.measure(miss_case)
    assert not metric.is_successful()


@pytest.mark.integration
def test_metric_null_equivalence():
    """None, 'none', 'null', '' all match as equivalent null labels."""
    from metrics import TaskKeyMatchMetric

    metric = TaskKeyMatchMetric()
    for null_val in [None, "none", "null", "n/a", ""]:
        case = LLMTestCase(input="x", actual_output=null_val, expected_output="none")
        metric.measure(case)
        assert metric.is_successful(), f"null equivalence failed for {null_val!r}"


# ---------------------------------------------------------------------------
# KAN-113: Baseline eval — production model from config.py
# Uncomment after populating .dataset.json (KAN-111).
# ---------------------------------------------------------------------------

# @pytest.mark.integration
# @pytest.mark.parametrize("golden", dataset.goldens)
# def test_baseline_classifier(golden: Golden):
#     """Evaluate the current production model (MODEL from config.py)."""
#     from agents.config import MODEL, BASE_URL
#     actual_output = _classify(MODEL, BASE_URL, golden.input)
#     test_case = LLMTestCase(
#         input=golden.input,
#         actual_output=actual_output,
#         expected_output=golden.expected_output,
#     )
#     assert_test(test_case=test_case, metrics=CLASSIFIER_METRICS)


# ---------------------------------------------------------------------------
# KAN-113: Model comparison grid
# Uncomment after populating CANDIDATE_MODELS and .dataset.json.
# ---------------------------------------------------------------------------

# @pytest.mark.integration
# @pytest.mark.parametrize("model_name,base_url", CANDIDATE_MODELS)
# @pytest.mark.parametrize("golden", dataset.goldens)
# def test_model_comparison(golden: Golden, model_name: str, base_url: str):
#     """Compare task_key accuracy across Ollama models."""
#     actual_output = _classify(model_name, base_url, golden.input)
#     test_case = LLMTestCase(
#         input=golden.input,
#         actual_output=actual_output,
#         expected_output=golden.expected_output,
#     )
#     assert_test(test_case=test_case, metrics=CLASSIFIER_METRICS)


# ---------------------------------------------------------------------------
# KAN-114: Prompt variant comparison
# Uncomment after populating PROMPT_VARIANTS and running KAN-113.
# ---------------------------------------------------------------------------

# @pytest.mark.integration
# @pytest.mark.parametrize("variant", PROMPT_VARIANTS)
# @pytest.mark.parametrize("golden", dataset.goldens)
# def test_prompt_variants(golden: Golden, variant: dict):
#     """Compare task_key accuracy across prompt configs for the best model."""
#     from agents.config import MODEL, BASE_URL
#     # TODO: wire variant config into _classify() or build_user_message()
#     actual_output = _classify(MODEL, BASE_URL, golden.input)
#     test_case = LLMTestCase(
#         input=golden.input,
#         actual_output=actual_output,
#         expected_output=golden.expected_output,
#     )
#     assert_test(test_case=test_case, metrics=CLASSIFIER_METRICS)
