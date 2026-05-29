"""Interactive classifier eval runner — scores Goldens against the MLX server
and emits OTel spans to OpenObserve.

Each Golden becomes a child span 'eval.classify' under one root span
'eval.run'. Spans carry: model, persona, seed_id, difficulty, expected_*,
actual_*, key_ok, type_ok, elapsed_s. Filter in OpenObserve by
service=meridian-eval to see the full run as a trace tree.

Use this for interactive experimentation (prompt changes, model swaps, new
Goldens). For CI assertions and Confident AI reports use test_classifier.py.

Pre-reqs:
    - MLX server running (auto-discovered on port 7823, or set MLX_SERVER_URL)
    - MERIDIAN_OTLP_ENDPOINT + MERIDIAN_OO_AUTH set (loaded from .env)
    - services/.venv active

Usage:
    # Render Goldens first (once per persona edit):
    services/.venv/bin/python services/tests/evals/render_seeds.py a_meridian

    # Run eval (server auto-discovered; or set MLX_SERVER_URL explicitly):
    EVAL_DATASET_PATH=services/tests/evals/data/generated/goldens_a_meridian.json \\
    services/.venv/bin/python services/tests/evals/eval_classifier.py

    # Validate a specific model is loaded:
    services/.venv/bin/python services/tests/evals/eval_classifier.py --model phi-4

Strategy selection:
    EVAL_STRATEGY=direct_http (default)  — POST to MLX /classify server
    Future: EVAL_STRATEGY=extract_then_classify, EVAL_STRATEGY=retrieval_augmented, …
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
from tests.evals.strategies import from_env as strategy_from_env  # noqa: E402


def _query_model_info(server_url: str) -> dict:
    """GET /info from the MLX server. Returns {} if the endpoint is unavailable."""
    url = server_url.rstrip("/") + "/info"
    try:
        req = urllib.request.Request(url)
        with urllib.request.urlopen(req, timeout=5) as resp:
            return json.loads(resp.read())
    except Exception:
        return {}


def _resolve_server(model_arg: str | None, port: int = 7823) -> "tuple[str, str, str] | None":
    """Return (server_url, model_id, source) or None if no server is reachable.

    source labels how the server was found: 'env' or 'auto-discovered'.
    model_id comes from /info — authoritative for span stamping. Falls back to
    the MLX_MODEL_ID env var if /info is unavailable (e.g. older server build).
    If model_arg is given, warns when it doesn't match the loaded model.
    """
    from agents.llm_selector import discover_mlx_eval_server, resolve_model

    env_url = os.environ.get("MLX_SERVER_URL")
    if env_url:
        server_url = env_url.rstrip("/")
        source = "env"
    else:
        discovered = discover_mlx_eval_server(port)
        if not discovered:
            print(
                f"ERROR: no MLX eval server found on port {port}.\n"
                f"  Start with: python -m agents.server --backend mlx --port {port}\n"
                f"  Or set:     MLX_SERVER_URL=http://127.0.0.1:{port}",
                file=sys.stderr,
            )
            return None
        server_url = discovered
        source = "auto-discovered"

    info = _query_model_info(server_url)
    model_id: str = info.get("model_id") or os.environ.get("MLX_MODEL_ID", "unknown")

    if model_arg:
        entry = resolve_model(model_arg)
        target_hf_id = (entry["hf_id"] if entry else model_arg) or model_arg
        if target_hf_id != model_id:
            print(
                f"WARNING: --model {model_arg!r} ({target_hf_id}) "
                f"but server has {model_id!r}",
                file=sys.stderr,
            )

    return server_url, model_id, source


def main() -> int:
    import argparse
    parser = argparse.ArgumentParser(description="Meridian eval smoke runner")
    parser.add_argument(
        "--model", default=None, metavar="NAME",
        help="Model short name or HF ID to validate against the running server (e.g. phi-4)",
    )
    parser.add_argument(
        "--port", type=int, default=7823, metavar="PORT",
        help="Port to probe for the MLX eval server when MLX_SERVER_URL is not set (default: 7823)",
    )
    args = parser.parse_args()

    resolved = _resolve_server(args.model, args.port)
    if resolved is None:
        return 1
    server_url, model_id, server_source = resolved

    dataset_path = Path(
        os.environ.get("EVAL_DATASET_PATH")
        or (_EVAL_DIR / "data" / "generated" / "goldens_real.json")
    )
    if not dataset_path.exists():
        print(f"ERROR: dataset not found at {dataset_path}", file=sys.stderr)
        return 1

    # ── Strategy ──
    try:
        strategy = strategy_from_env()
    except ValueError as exc:
        print(f"ERROR: {exc}", file=sys.stderr)
        return 1
    hyperparams = strategy.as_hyperparameters()

    # ── OTel setup ──
    # service.name=meridian-eval so OpenObserve queries can filter on it.
    obs_setup("meridian-eval")
    tracer = trace.get_tracer("meridian.eval")
    otlp_endpoint = os.environ.get("MERIDIAN_OTLP_ENDPOINT", "(unset — spans will not export)")
    print(f"Tracing: meridian-eval → {otlp_endpoint}")

    dataset = EvaluationDataset()
    dataset.add_goldens_from_json_file(file_path=str(dataset_path))
    print(f"Loaded {len(dataset.goldens)} Goldens from {dataset_path.name}")
    print(f"Model:    {model_id}")
    print(f"Server:   {server_url}  [{server_source}]")
    print(f"Strategy: {strategy.name}  config: {hyperparams}")
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
        root_span.set_attribute("strategy",     strategy.name)
        root_span.set_attribute("dataset_size", len(dataset.goldens))
        root_span.set_attribute("model_id",     model_id)
        for k, v in hyperparams.items():
            root_span.set_attribute(f"strategy.{k}", str(v))

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
                case_span.set_attribute("strategy",        strategy.name)
                case_span.set_attribute("expected.task_key",     exp.get("task_key") or "none")
                case_span.set_attribute("expected.session_type", exp.get("session_type") or "")

                result = strategy.classify_prompt(golden.input)
                error: str | None = result.extra.get("error") if result.method.endswith("_error") else None
                actual = result.as_actual_output()
                case_span.set_attribute("classifier.confidence", result.confidence)
                case_span.set_attribute("elapsed_s",             round(result.elapsed_s, 2))
                if error:
                    case_span.set_attribute("error", error)

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
                "elapsed":    result.elapsed_s,
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
                f"{(exp.get('session_type') or ''):<10} {(act.get('session_type') or ''):<10} {t} {result.elapsed_s:>4.1f}"
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
    # Force-flush is required to land the root eval.run span — it only ends when
    # the `with` block above closes, and obs_shutdown's 5s drain sometimes loses
    # the race on long runs (15+ min). Without this, the per-Golden eval.classify
    # children land but the root carrying accuracy.both / dataset_size / strategy
    # attributes is dropped, leaving an orphan trace in OpenObserve.
    _provider = trace.get_tracer_provider()
    if hasattr(_provider, "force_flush"):
        _provider.force_flush(timeout_millis=5_000)
    obs_shutdown()

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
