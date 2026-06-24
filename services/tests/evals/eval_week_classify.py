# ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
"""Eval of the production /classify_sessions endpoint against the week golden dataset.

Calls the SAME HTTP endpoint as the Rust daemon (POST /classify_sessions) so
model swaps and prompt changes are tested end-to-end without re-rendering prompts.

Dataset: services/tests/evals/rerank/data/sessions_week.json + labels_week.py
Labels use an acceptable-set (okset) format, so label uncertainty is captured.
Sessions must exist in the running server's meridian.db (they are real DB rows).

Each session becomes a child span 'eval.classify' under one root span 'eval.run'.
Filter in OpenObserve by service=meridian-eval.

Pre-reqs:
    - MLX server running (port 7823, or set MLX_SERVER_URL)
    - Sessions 31310–35478 must exist in ~/.meridian/meridian.db
    - MERIDIAN_OTLP_ENDPOINT set (loaded from .env) for OTel export
    - services/.venv active

Usage:
    services/.venv/bin/python services/tests/evals/eval_week_classify.py

    # Override server or dataset:
    MLX_SERVER_URL=http://127.0.0.1:7823 \\
    EVAL_DATASET_PATH=services/tests/evals/rerank/data/sessions_week.json \\
    services/.venv/bin/python services/tests/evals/eval_week_classify.py

    # Validate a specific model is loaded first:
    services/.venv/bin/python services/tests/evals/eval_week_classify.py --model phi-4
"""
from __future__ import annotations

import argparse
import importlib.util
import json
import os
import sys
import time
import urllib.error
import urllib.request
from collections import defaultdict
from pathlib import Path

_EVAL_DIR     = Path(__file__).parent
_SERVICES_DIR = _EVAL_DIR.parent.parent
_REPO_DIR     = _SERVICES_DIR.parent
_RERANK_DIR   = _EVAL_DIR / "rerank"
_DATA_DIR     = _RERANK_DIR / "data"

if str(_SERVICES_DIR) not in sys.path:
    sys.path.insert(0, str(_SERVICES_DIR))

try:
    from dotenv import load_dotenv
    repo_env = _REPO_DIR / ".env"
    if repo_env.exists():
        load_dotenv(repo_env, override=False)
except ImportError:
    pass

from agents.observability import setup as obs_setup, shutdown as obs_shutdown
from opentelemetry import trace
from opentelemetry.trace import StatusCode

from deepeval.test_case import LLMTestCase
from tests.evals.metrics import TaskKeyMatchMetric, SessionTypeMatchMetric, UntrackedNotTaskMetric


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def _query_model_info(server_url: str) -> dict:
    try:
        with urllib.request.urlopen(f"{server_url}/health", timeout=5) as r:
            return json.loads(r.read())
    except Exception:
        return {}


def _resolve_server(model_arg: str | None, port: int = 7823) -> "tuple[str, str, str] | None":
    """Return (url, model_id, source) or None if no server is reachable."""
    candidates = []
    if env_url := os.environ.get("MLX_SERVER_URL"):
        candidates.append((env_url.rstrip("/"), "env MLX_SERVER_URL"))
    candidates.append((f"http://127.0.0.1:{port}", "auto-discovered"))

    for url, source in candidates:
        info = _query_model_info(url)
        if info:
            model_id = info.get("model_id", "unknown")
            if model_arg and model_arg.lower() not in model_id.lower():
                print(f"WARNING: requested model {model_arg!r} but server has {model_id!r}", flush=True)
            return url, model_id, source
    return None


def _classify_session(server_url: str, session_id: int, db_path: str) -> dict:
    """POST to /classify_sessions for a single session. Returns the result dict."""
    payload = json.dumps({"session_ids": [session_id], "meridian_db": db_path}).encode()
    req = urllib.request.Request(
        f"{server_url}/classify_sessions",
        data=payload,
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    try:
        with urllib.request.urlopen(req, timeout=120) as r:
            body = json.loads(r.read())
            results = body.get("results", [])
            return results[0] if results else {}
    except urllib.error.HTTPError as e:
        return {"error": f"HTTP {e.code}: {e.read().decode()[:200]}"}
    except Exception as exc:
        return {"error": str(exc)}


def _load_labels(labels_file: Path) -> dict:
    """Load labels_week.py and return the L dict."""
    spec = importlib.util.spec_from_file_location("labels_week", labels_file)
    mod  = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod.L


def _hit(act_key: str | None, act_type: str, primary: str, okset: set, stype: str) -> bool:
    """Same acceptable-set logic as eval_reranker_classify.py / plans.py."""
    pred = act_key if act_key and act_key.lower() not in ("none", "null", "") else "NONE"
    accept = set(okset)
    if primary == "NONE":
        accept.add("NONE")
    return pred in accept


def _write_results(path: Path, meta: dict, rows: list[dict]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps({**meta, "per_session": rows}, indent=2))


def _percentile(values: list[float], pct: float) -> float:
    if not values:
        return 0.0
    s = sorted(values)
    idx = max(0, int(len(s) * pct / 100) - 1)
    return s[idx]


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

def main() -> int:
    parser = argparse.ArgumentParser(description="Eval production /classify_sessions vs week dataset")
    parser.add_argument("--model",   default=None, help="Assert model substring loaded on server")
    parser.add_argument("--db",      default=str(Path.home() / ".meridian" / "meridian.db"),
                        help="Path to meridian.db (sent to server; server may override)")
    parser.add_argument("--dataset", default=str(_DATA_DIR / "sessions_week.json"),
                        help="Path to sessions_week.json")
    parser.add_argument("--labels",  default=str(_DATA_DIR / "labels_week.py"),
                        help="Path to labels_week.py")
    args = parser.parse_args()

    dataset_path = Path(os.environ.get("EVAL_DATASET_PATH") or args.dataset)
    labels_path  = Path(args.labels)

    if not dataset_path.exists():
        print(f"ERROR: dataset not found at {dataset_path}", file=sys.stderr)
        return 1
    if not labels_path.exists():
        print(f"ERROR: labels not found at {labels_path}", file=sys.stderr)
        return 1

    server = _resolve_server(args.model)
    if server is None:
        print("ERROR: no MLX server reachable — start it with 'meridian mlx-server' or set MLX_SERVER_URL", file=sys.stderr)
        return 1
    server_url, model_id, server_source = server

    obs_setup("meridian-eval")
    tracer = trace.get_tracer("meridian-eval")

    sessions = json.load(dataset_path.open())
    labels   = _load_labels(labels_path)

    run_id = f"week_{time.strftime('%Y%m%dT%H%M%S')}"

    print(f"Dataset:  {dataset_path.name}  ({len(sessions)} sessions)")
    print(f"Labels:   {labels_path.name}  ({len(labels)} labels)")
    print(f"Server:   {server_url}  [{server_source}]")
    print(f"Model:    {model_id}")
    print(f"Run ID:   {run_id}")
    print()

    key_metric  = TaskKeyMatchMetric()
    type_metric = SessionTypeMatchMetric()
    unt_metric  = UntrackedNotTaskMetric()

    rows: list[dict] = []
    elapsed_list: list[float] = []

    fmt = "{:>5}  {:<11} {:<11} {:>10} {:>10}  K  {:<11} {:<11}  T  {:>5}"
    print(fmt.format("id", "exp_type", "act_type", "exp_key", "act_key", "exp_stype", "act_stype", "s"))
    print("-" * 100)

    with tracer.start_as_current_span("eval.run") as root_span:
        trace_id_hex = format(root_span.get_span_context().trace_id, "032x")
        root_span.set_attribute("run.id",        run_id)
        root_span.set_attribute("dataset",       dataset_path.name)
        root_span.set_attribute("dataset_size",  len(sessions))
        root_span.set_attribute("server_url",    server_url)
        root_span.set_attribute("model_id",      model_id)
        root_span.add_event("run_started", attributes={"run.id": run_id, "model_id": model_id})

        for session in sessions:
            sid = session["id"]
            label = labels.get(sid)
            if label is None:
                print(f"  {sid}: no label — skipping")
                continue

            primary, okset, uncertain, stype, note = label

            with tracer.start_as_current_span("eval.classify") as span:
                span.set_attribute("seed_id",          sid)
                span.set_attribute("session_day",      session.get("day", ""))
                span.set_attribute("uncertain",        uncertain)
                span.set_attribute("expected.task_key",     primary)
                span.set_attribute("expected.session_type", stype)

                t0 = time.monotonic()
                result = _classify_session(server_url, sid, args.db)
                elapsed = time.monotonic() - t0
                elapsed_list.append(elapsed)

                error = result.get("error")
                act_key   = result.get("task_key")   or None
                act_stype = result.get("session_type") or "untracked"
                confidence = float(result.get("confidence") or 0.0)

                if error:
                    span.set_status(StatusCode.ERROR, str(error)[:200])
                    span.set_attribute("error", str(error)[:200])
                    print(f"  {sid}: ERROR — {error[:80]}")
                    continue

                # Acceptable-set hit (same logic as eval_reranker_classify)
                hit = _hit(act_key, act_stype, primary, okset, stype)

                # DeepEval metrics (exact primary match + type match)
                exp_json = json.dumps({"task_key": primary if primary != "NONE" else None,
                                       "session_type": stype, "reasoning": note})
                act_json = json.dumps({"task_key": act_key, "session_type": act_stype,
                                       "reasoning": result.get("reasoning", "")})
                case = LLMTestCase(input=str(sid), actual_output=act_json, expected_output=exp_json)

                key_metric.measure(case)
                type_metric.measure(case)
                unt_metric.measure(case)

                key_ok  = hit          # use okset logic, not exact-match
                type_ok = type_metric.is_successful()
                unt_ok  = unt_metric.is_successful()

                span.set_attribute("actual.task_key",      str(act_key or "none"))
                span.set_attribute("actual.session_type",  act_stype)
                span.set_attribute("classifier.confidence", confidence)
                span.set_attribute("key_ok",  key_ok)
                span.set_attribute("type_ok", type_ok)
                span.set_attribute("unt_ok",  unt_ok)
                span.set_attribute("elapsed_s", round(elapsed, 2))

                if not (key_ok and type_ok):
                    span.set_status(StatusCode.ERROR, "classification_mismatch")
                    span.add_event("classification_mismatch", attributes={
                        "expected.task_key":     primary,
                        "expected.session_type": stype,
                        "actual.task_key":       str(act_key or "none"),
                        "actual.session_type":   act_stype,
                    })

                rows.append({
                    "session_id": sid, "day": session.get("day"),
                    "exp_type": stype,  "act_type": act_stype,
                    "exp_key":  primary, "act_key": act_key,
                    "key_ok": key_ok,   "type_ok": type_ok, "unt_ok": unt_ok,
                    "uncertain": uncertain,
                    "confidence": confidence,
                    "elapsed_s": round(elapsed, 2),
                    "note": note,
                })

                mark = "✓" if key_ok and type_ok else "✗"
                print(fmt.format(
                    sid, stype, act_stype,
                    primary, str(act_key or "—"),
                    stype, act_stype,
                    f"{elapsed:.1f}",
                ) + f"  {mark}")

        # ── Aggregate ────────────────────────────────────────────────────────
        total   = len(rows)
        n_both  = sum(1 for r in rows if r["key_ok"] and r["type_ok"])
        n_key   = sum(1 for r in rows if r["key_ok"])
        n_type  = sum(1 for r in rows if r["type_ok"])
        n_unt   = sum(1 for r in rows if r["unt_ok"])

        by_type: dict[str, list] = defaultdict(list)
        for r in rows:
            by_type[r["exp_type"]].append(r)

        root_span.set_attribute("results.total",   total)
        root_span.set_attribute("results.both_ok", n_both)
        root_span.set_attribute("results.key_ok",  n_key)
        root_span.set_attribute("results.accuracy", round(n_both / total, 4) if total else 0)

        print()
        print("=" * 60)
        print(f"RESULTS  {dataset_path.name}  (n={total})")
        print(f"  overall (key+type):  {n_both}/{total} = {n_both/total*100:.1f}%")
        print(f"  key match (okset):   {n_key}/{total}  = {n_key/total*100:.1f}%")
        print(f"  type match:          {n_type}/{total} = {n_type/total*100:.1f}%")
        print(f"  untracked guard:     {n_unt}/{total}  = {n_unt/total*100:.1f}%")
        print()
        for t in ("task", "overhead", "untracked"):
            grp = by_type[t]
            if not grp:
                continue
            ok = sum(1 for r in grp if r["key_ok"] and r["type_ok"])
            print(f"  {t:<12}: {ok}/{len(grp)} = {ok/len(grp)*100:.0f}%")
        print()
        if elapsed_list:
            print(f"  elapsed p50={_percentile(elapsed_list,50):.1f}s  p95={_percentile(elapsed_list,95):.1f}s  total={sum(elapsed_list):.0f}s")
        print(f"  model:  {model_id}")
        print(f"  run_id: {run_id}  trace: {trace_id_hex[:16]}…")
        print("=" * 60)

        print()
        print("Failures:")
        for r in rows:
            if not (r["key_ok"] and r["type_ok"]):
                print(f"  {r['session_id']}  exp={r['exp_type']}/{r['exp_key']}  act={r['act_type']}/{r['act_key']}  conf={r['confidence']:.2f}")

    # ── Write results ─────────────────────────────────────────────────────────
    results_dir = _EVAL_DIR / "results"
    out = results_dir / f"week_classify_{run_id}.json"
    _write_results(out, {
        "run_id": run_id, "model_id": model_id,
        "server_url": server_url, "dataset": dataset_path.name,
        "trace_id": trace_id_hex,
        "summary": {
            "total": total, "both_ok": n_both,
            "accuracy": round(n_both / total, 4) if total else 0,
            "key_ok": n_key, "type_ok": n_type, "unt_ok": n_unt,
            "by_type": {t: {"n": len(by_type[t]),
                            "ok": sum(1 for r in by_type[t] if r["key_ok"] and r["type_ok"])}
                        for t in ("task", "overhead", "untracked")},
        },
    }, rows)
    print(f"\nResults → {out}")

    obs_shutdown()
    return 0


if __name__ == "__main__":
    sys.exit(main())
