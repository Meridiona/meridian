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

# Load the repo-root .env so OTLP endpoint + auth are visible. Nothing is read
# from outside the repo; the existing shell env still wins (override=False).
try:
    from dotenv import load_dotenv  # noqa: E402
    repo_env = _REPO_DIR / ".env"
    if repo_env.exists():
        load_dotenv(repo_env, override=False)
except ImportError:
    pass  # dotenv optional — env may already be set by shell

from agents.observability import setup as obs_setup, shutdown as obs_shutdown  # noqa: E402
from opentelemetry import trace  # noqa: E402
from opentelemetry.trace import StatusCode  # noqa: E402

from deepeval.dataset import EvaluationDataset  # noqa: E402
from deepeval.test_case import LLMTestCase  # noqa: E402

from tests.evals.metrics import TaskKeyMatchMetric, SessionTypeMatchMetric  # noqa: E402
from tests.evals.strategies import (  # noqa: E402
    from_env as strategy_from_env,
    from_config as strategy_from_config,
)


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
    import urllib.request

    env_url = os.environ.get("MLX_SERVER_URL")
    if env_url:
        server_url = env_url.rstrip("/")
        source = "env"
    else:
        candidate = f"http://127.0.0.1:{port}"
        try:
            urllib.request.urlopen(f"{candidate}/health", timeout=2)
            server_url = candidate
            source = "auto-discovered"
        except Exception:
            print(
                f"ERROR: no MLX eval server found on port {port}.\n"
                f"  Start with: python -m agents.server --port {port}\n"
                f"  Or set:     MLX_SERVER_URL=http://127.0.0.1:{port}",
                file=sys.stderr,
            )
            return None

    info = _query_model_info(server_url)
    model_id: str = info.get("model_id") or os.environ.get("MLX_MODEL_ID", "unknown")

    if model_arg and model_arg != model_id:
        print(
            f"WARNING: --model {model_arg!r} but server has {model_id!r}",
            file=sys.stderr,
        )

    return server_url, model_id, source


def _percentile(values: list[float], pct: float) -> float:
    """Return the pct percentile of values (0-100). Nearest-rank method, no numpy."""
    if not values:
        return 0.0
    s = sorted(values)
    if len(s) == 1:
        return s[0]
    k = max(0, min(len(s) - 1, int(round((pct / 100.0) * (len(s) - 1)))))
    return s[k]


def _write_results_json(
    *,
    rows: list[dict],
    run_id: str,
    run_started_at: str,
    trace_id_hex: str,
    persona: str,
    dataset_path: Path,
    server_url: str,
    server_source: str,
    model_id: str,
    strategy_name: str,
    hyperparams: dict,
    out_dir: Path,
    experiment_config: dict | None = None,
    config_path: Path | None = None,
) -> Path:
    """Write a canonical results JSON file the Claude Code loop can read directly.

    This is the local mirror of the OpenObserve trace — same data, immediately
    accessible without an OTLP round-trip or auth. The eval-feedback skill
    prefers this file over OpenObserve when available; OpenObserve remains a
    fallback for trace exploration.

    Schema is flat and stable (see services/tests/evals/README.md § Results
    schema for the field list). Filename: run_<run_id>.json.
    """
    total = len(rows)
    if total == 0:
        return out_dir / f"run_{run_id}.json"

    # ── Aggregate metrics ─────────────────────────────────────────────────
    key_correct  = sum(1 for r in rows if r["key_ok"])
    type_correct = sum(1 for r in rows if r["type_ok"])
    both_correct = sum(1 for r in rows if r["key_ok"] and r["type_ok"])

    # Per-tier breakdown
    by_tier: dict[str, list[dict]] = defaultdict(list)
    for r in rows:
        by_tier[r["difficulty"]].append(r)
    per_tier_metrics: dict[str, dict] = {}
    for tier, items in sorted(by_tier.items()):
        n = len(items)
        k = sum(1 for r in items if r["key_ok"])
        t = sum(1 for r in items if r["type_ok"])
        b = sum(1 for r in items if r["key_ok"] and r["type_ok"])
        per_tier_metrics[tier] = {
            "total":            n,
            "passed_both":      b,
            "task_key_acc":     round(k / n, 3) if n else 0.0,
            "session_type_acc": round(t / n, 3) if n else 0.0,
            "both_acc":         round(b / n, 3) if n else 0.0,
        }

    # Latency stats
    elapsed_values = [float(r["elapsed"]) for r in rows]
    elapsed_sum = sum(elapsed_values)
    latency = {
        "total_s": round(elapsed_sum, 3),
        "avg_s":   round(elapsed_sum / total, 3),
        "min_s":   round(min(elapsed_values), 3),
        "max_s":   round(max(elapsed_values), 3),
        "p50_s":   round(_percentile(elapsed_values, 50), 3),
        "p95_s":   round(_percentile(elapsed_values, 95), 3),
    }

    # ── Per-seed results (structured) ────────────────────────────────────
    per_seed = [
        {
            "seed_id":    r["seed_id"],
            "difficulty": r["difficulty"],
            "app_name":   r["app_name"],
            "expected": {
                "task_key":     r["exp_task_key_raw"],
                "session_type": r["exp_session_type_raw"],
            },
            "actual": {
                "task_key":     r["act_task_key_raw"],
                "session_type": r["act_session_type_raw"],
                "confidence":   round(r["confidence"], 3),
                "reasoning":    r["reasoning"],
            },
            "key_ok":    r["key_ok"],
            "type_ok":   r["type_ok"],
            "both_ok":   r["key_ok"] and r["type_ok"],
            "elapsed_s": round(float(r["elapsed"]), 3),
            "method":    r["method"],
            "error":     r["error"],
        }
        for r in rows
    ]

    # ── Build envelope ────────────────────────────────────────────────────
    session_text_cap_env = os.environ.get("SESSION_TEXT_CAP")
    session_text_cap = (
        int(session_text_cap_env) if session_text_cap_env and session_text_cap_env.isdigit()
        else 2500
    )

    envelope = {
        "run_id":       run_id,
        "timestamp":    run_started_at,
        "trace_id":     trace_id_hex,
        "experiment": {
            # Experiment-manifest metadata — populated when --config <path> is used.
            # None when the run was launched from env vars only (no config file).
            "name":        (experiment_config or {}).get("name"),
            "description": (experiment_config or {}).get("description"),
            "config_file": str(config_path) if config_path else None,
        },
        "config": {
            "strategy":         strategy_name,
            "model_id":         model_id,
            "dataset_path":     str(dataset_path),
            "dataset_name":     dataset_path.name,
            "persona":          persona,
            "server_url":       server_url,
            "server_source":    server_source,
            "session_text_cap": session_text_cap,
            "hyperparameters":  hyperparams,
            # Server/render-side values recorded for provenance only. The runner
            # does NOT enforce these — user must set them out-of-band (server
            # restart for model/temperature/max_tokens, SESSION_TEXT_CAP env +
            # render_seeds.py re-run for session_text_cap, SKILL.md edit for
            # prompt_version). The experiment-config block above captures what
            # the user DECLARED these were; verifying they match is on them.
            "recorded_from_config": {
                k: v for k, v in (experiment_config or {}).items()
                if k in ("model", "session_text_cap", "temperature",
                         "max_tokens", "prompt_version")
            } if experiment_config else {},
        },
        "metrics": {
            "total_goldens":         total,
            "passed_both":           both_correct,
            "task_key_accuracy":     round(key_correct / total, 3),
            "session_type_accuracy": round(type_correct / total, 3),
            "both_accuracy":         round(both_correct / total, 3),
            "per_tier":              per_tier_metrics,
            "latency":               latency,
        },
        "per_seed_results": per_seed,
    }

    # ── Write file ───────────────────────────────────────────────────────
    out_dir.mkdir(parents=True, exist_ok=True)
    out_path = out_dir / f"run_{run_id}.json"
    out_path.write_text(
        json.dumps(envelope, indent=2, ensure_ascii=False) + "\n",
        encoding="utf-8",
    )
    return out_path


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
    parser.add_argument(
        "--config", type=str, default=None, metavar="PATH",
        help=(
            "Path to a JSON experiment-manifest config. SETS what the runner "
            "controls (strategy, dataset_path, endpoint, strategy options); "
            "RECORDS server/render-side knobs (model, session_text_cap, "
            "temperature, max_tokens, prompt_version) in results.json for "
            "provenance. Overrides env vars. CLI --model overrides config.model."
        ),
    )
    args = parser.parse_args()

    # ── Load experiment config (if provided) ──
    experiment_config: dict = {}
    config_path: Path | None = None
    if args.config:
        config_path = Path(args.config)
        if not config_path.exists():
            print(f"ERROR: config not found at {config_path}", file=sys.stderr)
            return 1
        try:
            experiment_config = json.loads(config_path.read_text(encoding="utf-8"))
        except json.JSONDecodeError as exc:
            print(f"ERROR: config is not valid JSON ({config_path}): {exc}", file=sys.stderr)
            return 1
        print(f"Config:   {config_path}")
        if experiment_config.get("name"):
            print(f"          name={experiment_config['name']!r}")
        if experiment_config.get("description"):
            print(f"          {experiment_config['description']}")

    # ── CLI --model > config.model > env > /info ──
    cli_or_config_model = args.model or experiment_config.get("model")
    resolved = _resolve_server(cli_or_config_model, args.port)
    if resolved is None:
        return 1
    server_url, model_id, server_source = resolved

    # ── Dataset: env > config > default ──
    dataset_path = Path(
        os.environ.get("EVAL_DATASET_PATH")
        or experiment_config.get("dataset_path")
        or (_EVAL_DIR / "data" / "generated" / "goldens_real.json")
    )
    if not dataset_path.exists():
        print(f"ERROR: dataset not found at {dataset_path}", file=sys.stderr)
        return 1

    # ── Strategy: config > env ──
    try:
        if experiment_config:
            strategy = strategy_from_config(experiment_config)
        else:
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
    # run_id format: <persona>_<strategy>_<YYYYMMDDTHHMMSS>
    # Identifying enough that `ls results/*b_generic*` or `ls results/*direct_http*`
    # filters runs without opening files. Used in filename, OTel run.id attribute,
    # and stdout.
    run_id = f"{persona}_{strategy.name}_{time.strftime('%Y%m%dT%H%M%S')}"
    run_started_at = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
    trace_id_hex: str = ""

    # ── Root span for the whole run ──
    with tracer.start_as_current_span("eval.run") as root_span:
        # Capture trace_id inside the with block so we can write it to the
        # local results JSON later (root_span is out of scope after the with).
        trace_id_hex = format(root_span.get_span_context().trace_id, "032x")
        root_span.set_attribute("run.id",       run_id)
        root_span.set_attribute("persona",      persona)
        root_span.set_attribute("dataset_path", str(dataset_path))
        root_span.set_attribute("dataset_name", dataset_path.name)
        root_span.set_attribute("server_url",   server_url)
        root_span.set_attribute("server_source", server_source)
        root_span.set_attribute("strategy",     strategy.name)
        root_span.set_attribute("dataset_size", len(dataset.goldens))
        root_span.set_attribute("model_id",     model_id)
        session_text_cap_env = os.environ.get("SESSION_TEXT_CAP", "")
        if session_text_cap_env:
            root_span.set_attribute("session_text_cap", session_text_cap_env)
        for k, v in hyperparams.items():
            root_span.set_attribute(f"strategy.{k}", str(v))
        # Discrete marker for "run started" — useful for OpenObserve search
        # to find the boundary between configuration and execution.
        root_span.add_event("run_started", attributes={
            "run.id":      run_id,
            "persona":     persona,
            "strategy":    strategy.name,
            "model_id":    model_id,
            "dataset":     dataset_path.name,
            "started_at":  run_started_at,
        })

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
                case_span.set_attribute("prompt_chars",    len(golden.input))
                case_span.set_attribute("expected.task_key",     exp.get("task_key") or "none")
                case_span.set_attribute("expected.session_type", exp.get("session_type") or "")
                # Full rendered prompt as a span event — debuggable in OO without
                # having to find and open the source Golden file. Truncated to 5000
                # chars to keep span payloads under OTel size limits.
                case_span.add_event("prompt_input", attributes={
                    "text":  golden.input[:5000],
                    "chars": len(golden.input),
                    "truncated": len(golden.input) > 5000,
                })

                # Child span scopes the actual strategy invocation (HTTP call or
                # in-process inference). Separates pure-strategy time from metric
                # measurement + bookkeeping. Visible in OO as a distinct child
                # under eval.classify.
                with tracer.start_as_current_span("strategy.invoke") as strat_span:
                    strat_span.set_attribute("strategy.name",     strategy.name)
                    for k, v in hyperparams.items():
                        strat_span.set_attribute(f"strategy.{k}", str(v))
                    result = strategy.classify_prompt(golden.input)
                    strat_span.set_attribute("strategy.method",   result.method)
                    strat_span.set_attribute("strategy.elapsed_s", round(result.elapsed_s, 3))
                    strat_span.set_attribute("classifier.confidence", result.confidence)
                    if result.method.endswith("_error"):
                        err = result.extra.get("error", "unknown")
                        strat_span.set_status(StatusCode.ERROR, str(err)[:200])
                        strat_span.set_attribute("error", str(err)[:200])

                error: str | None = result.extra.get("error") if result.method.endswith("_error") else None
                actual = result.as_actual_output()
                case_span.set_attribute("classifier.confidence", result.confidence)
                case_span.set_attribute("strategy.method",       result.method)
                case_span.set_attribute("elapsed_s",             round(result.elapsed_s, 2))
                if error:
                    case_span.set_attribute("error", error)
                    case_span.set_status(StatusCode.ERROR, str(error)[:200])

                # Raw classifier response as a span event — what the model
                # actually returned, before deepeval-metric formatting. Useful
                # for debugging "why did this fail" without re-running.
                case_span.add_event("classifier_response", attributes={
                    "task_key":     str(result.task_key) if result.task_key else "none",
                    "session_type": result.session_type,
                    "confidence":   result.confidence,
                    "method":       result.method,
                    "elapsed_s":    round(result.elapsed_s, 3),
                })

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
                # Mark mismatch as a clear ERROR/INFO event so it stands out in OO
                if not (key_ok and type_ok):
                    case_span.add_event("classification_mismatch", attributes={
                        "expected.task_key":     str(exp.get("task_key") or "none"),
                        "actual.task_key":       str(act.get("task_key") or "none"),
                        "expected.session_type": str(exp.get("session_type") or ""),
                        "actual.session_type":   str(act.get("session_type") or ""),
                        "key_ok":  key_ok,
                        "type_ok": type_ok,
                    })

                # Use span events for the reasoning text (longer, doesn't belong as an attribute)
                if act.get("reasoning"):
                    case_span.add_event("actual_reasoning", attributes={"text": act["reasoning"][:1000]})

            rows.append({
                "seed_id":    seed_id,
                "app_name":   app_name,
                "difficulty": difficulty,
                # Display strings (for stdout) — preserve "none" sentinel.
                "exp_key":    (exp.get("task_key") or "none"),
                "act_key":    (act.get("task_key") or "none"),
                "exp_type":   (exp.get("session_type") or ""),
                "act_type":   (act.get("session_type") or ""),
                "key_ok":     key_ok,
                "type_ok":    type_ok,
                "elapsed":    result.elapsed_s,
                "error":      error,
                # Raw values (for results JSON) — preserve null for missing task_key.
                "exp_task_key_raw":     exp.get("task_key"),
                "act_task_key_raw":     act.get("task_key"),
                "exp_session_type_raw": exp.get("session_type"),
                "act_session_type_raw": act.get("session_type"),
                "confidence":           result.confidence,
                "reasoning":            act.get("reasoning", ""),
                "method":               result.method,
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
        elapsed_total = sum(r["elapsed"] for r in rows)
        root_span.set_attribute("accuracy.task_key",     round(key_correct / total, 3) if total else 0.0)
        root_span.set_attribute("accuracy.session_type", round(type_correct / total, 3) if total else 0.0)
        root_span.set_attribute("accuracy.both",         round(both_correct / total, 3) if total else 0.0)
        root_span.set_attribute("elapsed_total_s",       round(elapsed_total, 2))
        root_span.set_attribute("elapsed_avg_s",         round(elapsed_total / total, 2) if total else 0.0)

        # Discrete end-of-run marker — pair with run_started event.
        root_span.add_event("run_completed", attributes={
            "total_goldens":         total,
            "passed_both":           both_correct,
            "task_key_accuracy":     round(key_correct / total, 3) if total else 0.0,
            "session_type_accuracy": round(type_correct / total, 3) if total else 0.0,
            "both_accuracy":         round(both_correct / total, 3) if total else 0.0,
            "elapsed_total_s":       round(elapsed_total, 2),
        })

        # Per-tier breakdown as a span event (one event per tier, easy to filter
        # in OO by event name "per_tier_summary").
        by_tier_oo: dict[str, list[dict]] = defaultdict(list)
        for r in rows:
            by_tier_oo[r["difficulty"]].append(r)
        for tier, items in sorted(by_tier_oo.items()):
            n = len(items)
            b = sum(1 for r in items if r["key_ok"] and r["type_ok"])
            root_span.add_event("per_tier_summary", attributes={
                "tier":         tier,
                "total":        n,
                "passed_both":  b,
                "both_acc":     round(b / n, 3) if n else 0.0,
            })

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

    # elapsed_total is already computed inside the eval.run with block above
    print()
    print(f"Total inference time: {elapsed_total:.1f}s  ·  avg per case: {elapsed_total/total:.2f}s")
    print()
    print(f"Run id: {run_id}")
    print(f"In OpenObserve: service=meridian-eval, span_name=eval.run, attribute run.id={run_id}")

    # ── Write canonical results JSON (local mirror of the OTel trace) ──
    # This file is the source of truth for the Claude Code feedback loop —
    # eval-feedback skill reads it directly instead of querying OpenObserve.
    results_path = _write_results_json(
        rows=rows,
        run_id=run_id,
        run_started_at=run_started_at,
        trace_id_hex=trace_id_hex,
        persona=persona,
        dataset_path=dataset_path,
        server_url=server_url,
        server_source=server_source,
        model_id=model_id,
        strategy_name=strategy.name,
        hyperparams=hyperparams,
        out_dir=_EVAL_DIR / "results",
        experiment_config=experiment_config or None,
        config_path=config_path,
    )
    print(f"Results written: {results_path.relative_to(_REPO_DIR)}")

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
