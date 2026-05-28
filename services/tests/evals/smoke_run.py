"""One-shot smoke runner — runs dev_a synthetic Goldens through the MLX server
and emits OTel spans to OpenObserve so the run is visible in your existing
tracing UI (the @observe decorator in test_mlx_classifier.py targets Confident AI
cloud; this runner uses the OTel SDK directly to land in OpenObserve).

Each Golden becomes a child span 'eval.classify' under one root span
'eval.run'. Spans carry: model, persona, seed_id, difficulty, expected_*,
actual_*, key_ok, type_ok, elapsed_s. Filter in OpenObserve by
service=meridian-eval to see the full run as a trace tree.

Pre-reqs:
    - MLX server on MLX_SERVER_URL (e.g. http://localhost:7823)
    - MERIDIAN_OTLP_ENDPOINT + MERIDIAN_OO_AUTH set (loaded from .env)
    - services/.venv active

Usage:
    EVAL_DATASET_PATH=services/tests/evals/.synthetic-dataset-a_meridian.json \\
    MLX_SERVER_URL=http://localhost:7823 \\
    services/.venv/bin/python services/tests/evals/smoke_run.py
"""
from __future__ import annotations

import json
import os
import sys
import time
import urllib.request
import urllib.parse
from collections import defaultdict
from pathlib import Path

_EVAL_DIR = Path(__file__).parent
_SERVICES_DIR = _EVAL_DIR.parent.parent
_REPO_DIR = _SERVICES_DIR.parent
if str(_SERVICES_DIR) not in sys.path:
    sys.path.insert(0, str(_SERVICES_DIR))

# Load .env (repo + ~/.meridian) so OTLP endpoint + auth are visible.
# Earliest-wins: repo .env first, then ~/.meridian/.env, then existing shell env.
try:
    from dotenv import load_dotenv  # noqa: E402
    home_env = Path.home() / ".meridian" / ".env"
    if home_env.exists():
        load_dotenv(home_env, override=False)
    repo_env = _REPO_DIR / ".env"
    if repo_env.exists():
        load_dotenv(repo_env, override=False)
except ImportError:
    pass  # dotenv optional — env may already be set by shell

from agents.observability import setup as obs_setup, shutdown as obs_shutdown  # noqa: E402
from opentelemetry import trace  # noqa: E402

from deepeval.dataset import EvaluationDataset  # noqa: E402
from deepeval.test_case import LLMTestCase  # noqa: E402

from tests.evals.metrics import TaskKeyMatchMetric, SessionTypeMatchMetric  # noqa: E402


def _classify_http(server_url: str, prompt_input: str, timeout: int = 120) -> tuple[str, dict]:
    """POST to /classify; return (actual_output JSON string, raw response dict)."""
    req = urllib.request.Request(
        f"{server_url.rstrip('/')}/classify",
        data=json.dumps({"input": prompt_input}).encode(),
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    with urllib.request.urlopen(req, timeout=timeout) as resp:
        data = json.loads(resp.read())
    actual = json.dumps({
        "task_key":     data.get("task_key") or "none",
        "session_type": data.get("session_type", "overhead"),
        "reasoning":    data.get("reasoning", ""),
    }, ensure_ascii=False)
    return actual, data


def main() -> int:
    server_url = os.environ.get("MLX_SERVER_URL")
    if not server_url:
        print("ERROR: MLX_SERVER_URL not set", file=sys.stderr)
        return 1

    dataset_path = Path(
        os.environ.get("EVAL_DATASET_PATH")
        or (_EVAL_DIR / ".dataset.json")
    )
    if not dataset_path.exists():
        print(f"ERROR: dataset not found at {dataset_path}", file=sys.stderr)
        return 1

    # ── OTel setup ──
    # service.name=meridian-eval so OpenObserve queries can filter on it.
    obs_setup("meridian-eval")
    tracer = trace.get_tracer("meridian.eval")
    otlp_endpoint = os.environ.get("MERIDIAN_OTLP_ENDPOINT", "(unset — spans will not export)")
    print(f"Tracing: meridian-eval → {otlp_endpoint}")

    dataset = EvaluationDataset()
    dataset.add_goldens_from_json_file(file_path=str(dataset_path))
    print(f"Loaded {len(dataset.goldens)} Goldens from {dataset_path.name}")
    print(f"Classifier: {server_url}")
    print()

    key_metric = TaskKeyMatchMetric()
    type_metric = SessionTypeMatchMetric()
    rows: list[dict] = []

    persona = dataset.goldens[0].additional_metadata.get("persona", "unknown") if dataset.goldens else "unknown"
    run_id = f"smoke_{time.strftime('%Y%m%dT%H%M%S')}"

    # ── Root span for the whole run ──
    with tracer.start_as_current_span("eval.run") as root_span:
        root_span.set_attribute("run.id",       run_id)
        root_span.set_attribute("persona",      persona)
        root_span.set_attribute("dataset_path", str(dataset_path))
        root_span.set_attribute("server_url",   server_url)
        root_span.set_attribute("dataset_size", len(dataset.goldens))

        print(f"{'seed':>5} {'app':<14} {'diff':<11} {'exp_key':<10} {'act_key':<10} K {'exp_type':<10} {'act_type':<10} T {'s':>4}")
        print("-" * 95)

        for golden in dataset.goldens:
            meta = golden.additional_metadata or {}
            seed_id = meta.get("seed_id", "?")
            difficulty = meta.get("difficulty", "?")
            app_name = meta.get("app_name", "?")
            exp = json.loads(golden.expected_output)

            with tracer.start_as_current_span("eval.classify") as case_span:
                case_span.set_attribute("seed_id",         str(seed_id))
                case_span.set_attribute("difficulty",      difficulty)
                case_span.set_attribute("app_name",        app_name)
                case_span.set_attribute("persona",         persona)
                case_span.set_attribute("expected.task_key",     exp.get("task_key") or "none")
                case_span.set_attribute("expected.session_type", exp.get("session_type") or "")

                t0 = time.time()
                try:
                    actual, raw = _classify_http(server_url, golden.input)
                    error: str | None = None
                    case_span.set_attribute("classifier.confidence", float(raw.get("confidence", 0.0)))
                except Exception as exc:
                    actual = json.dumps({"task_key": "none", "session_type": "overhead", "reasoning": ""})
                    error = str(exc)[:200]
                    case_span.set_attribute("error", error)
                elapsed = time.time() - t0
                case_span.set_attribute("elapsed_s", round(elapsed, 2))

                case = LLMTestCase(input=golden.input, actual_output=actual, expected_output=golden.expected_output)
                key_metric.measure(case)
                key_ok = key_metric.is_successful()
                type_metric.measure(case)
                type_ok = type_metric.is_successful()

                act = json.loads(actual)
                case_span.set_attribute("actual.task_key",     act.get("task_key") or "none")
                case_span.set_attribute("actual.session_type", act.get("session_type") or "")
                case_span.set_attribute("key_ok",  key_ok)
                case_span.set_attribute("type_ok", type_ok)
                case_span.set_attribute("both_ok", key_ok and type_ok)

                # Use span events for the reasoning text (longer, doesn't belong as an attribute)
                if act.get("reasoning"):
                    case_span.add_event("actual_reasoning", attributes={"text": act["reasoning"][:1000]})

            rows.append({
                "seed_id":    seed_id,
                "app_name":   app_name,
                "difficulty": difficulty,
                "exp_key":    (exp.get("task_key") or "none"),
                "act_key":    (act.get("task_key") or "none"),
                "key_ok":     key_ok,
                "exp_type":   (exp.get("session_type") or ""),
                "act_type":   (act.get("session_type") or ""),
                "type_ok":    type_ok,
                "elapsed":    elapsed,
                "error":      error,
            })

            # Flush the just-closed case span immediately so it can't be lost
            # if the run is killed mid-way. Cheap; BatchSpanProcessor drains its
            # current queue. The 'with' for eval.run is still open — only ended
            # children get exported here.
            _provider = trace.get_tracer_provider()
            if hasattr(_provider, "force_flush"):
                _provider.force_flush(timeout_millis=2_000)

            k = "✓" if key_ok else "✗"
            t = "✓" if type_ok else "✗"
            line = (
                f"{seed_id:>5} {app_name:<14} {difficulty:<11} "
                f"{(exp.get('task_key') or 'none'):<10} {(act.get('task_key') or 'none'):<10} {k} "
                f"{(exp.get('session_type') or ''):<10} {(act.get('session_type') or ''):<10} {t} {elapsed:>4.1f}"
            )
            if error:
                line += f"  ERROR: {error}"
            print(line)

        # Aggregate stats on root span
        total = len(rows)
        key_correct  = sum(1 for r in rows if r["key_ok"])
        type_correct = sum(1 for r in rows if r["type_ok"])
        both_correct = sum(1 for r in rows if r["key_ok"] and r["type_ok"])
        root_span.set_attribute("accuracy.task_key",     round(key_correct / total, 3) if total else 0.0)
        root_span.set_attribute("accuracy.session_type", round(type_correct / total, 3) if total else 0.0)
        root_span.set_attribute("accuracy.both",         round(both_correct / total, 3) if total else 0.0)

    # ── Per-tier breakdown ──
    print("-" * 95)
    if total == 0:
        print("No rows scored.")
        return 1

    print(f"task_key match:     {key_correct}/{total}  =  {key_correct/total:.0%}")
    print(f"session_type match: {type_correct}/{total}  =  {type_correct/total:.0%}")
    print(f"both match:         {both_correct}/{total}  =  {both_correct/total:.0%}")
    print()

    by_tier: dict[str, list[dict]] = defaultdict(list)
    for r in rows:
        by_tier[r["difficulty"]].append(r)

    print("Per-tier accuracy (both metrics must pass):")
    print(f"  {'tier':<14} {'pass/total':<12} {'task_key':<14} {'session_type'}")
    print(f"  {'-'*14} {'-'*12} {'-'*14} {'-'*12}")
    for tier in sorted(by_tier.keys()):
        items = by_tier[tier]
        b = sum(1 for r in items if r["key_ok"] and r["type_ok"])
        k = sum(1 for r in items if r["key_ok"])
        t_ = sum(1 for r in items if r["type_ok"])
        n = len(items)
        print(f"  {tier:<14} {b}/{n}  ({b/n:.0%})    {k}/{n}  ({k/n:.0%})      {t_}/{n}  ({t_/n:.0%})")

    elapsed_total = sum(r["elapsed"] for r in rows)
    print()
    print(f"Total inference time: {elapsed_total:.1f}s  ·  avg per case: {elapsed_total/total:.2f}s")
    print()
    print(f"Run id: {run_id}")
    print(f"In OpenObserve: service=meridian-eval, span_name=eval.run, attribute run.id={run_id}")

    # ── Flush spans before exit (BatchSpanProcessor is async) ──
    obs_shutdown()

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
