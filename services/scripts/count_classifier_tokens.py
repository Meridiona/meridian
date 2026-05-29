#!/usr/bin/env python3
"""Count input + output tokens for each session in a classifier JSONL log.

Reads `services/logs/mlx/server_*.jsonl` produced by the FastAPI MLX
server's `/classify_sessions` endpoint and prints a per-session token
table using the same tokenizer the classifier itself loads.

Usage:
    cd services
    .venv313/bin/python scripts/count_classifier_tokens.py
    .venv313/bin/python scripts/count_classifier_tokens.py logs/mlx/server_20260528T175639.jsonl
    .venv313/bin/python scripts/count_classifier_tokens.py --top 20

Designed to be read-only — no daemon restart, no behavioural change.
"""
from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

LOG_DIR = Path(__file__).resolve().parent.parent / "logs" / "mlx"
DEFAULT_MODEL_ID = "mlx-community/Qwen3.5-9B-OptiQ-4bit"


def _latest_log() -> Path:
    files = sorted(LOG_DIR.glob("server_*.jsonl"))
    if not files:
        sys.exit(f"no log files found under {LOG_DIR}")
    return files[-1]


def _read_records(path: Path) -> list[dict]:
    """Parse JSONL, drop blank / malformed lines, drop pre-result records."""
    out: list[dict] = []
    for line_no, line in enumerate(path.read_text().splitlines(), start=1):
        if not line.strip():
            continue
        try:
            rec = json.loads(line)
        except json.JSONDecodeError as exc:
            print(f"  skip line {line_no}: {exc}", file=sys.stderr)
            continue
        if "result" in rec and rec["result"].get("session_id"):
            out.append(rec)
    return out


def _format_messages(rec: dict) -> list[dict]:
    """Reconstruct the exact messages outlines saw at inference time."""
    return [
        {"role": "system", "content": rec.get("system_prompt", "") or ""},
        {"role": "user",   "content": rec.get("user_message", "") or ""},
    ]


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(prog="count_classifier_tokens")
    parser.add_argument(
        "path", nargs="?", default=None,
        help="JSONL log path. Defaults to the most recent under services/logs/mlx/",
    )
    parser.add_argument(
        "--model-id", default=DEFAULT_MODEL_ID,
        help="HuggingFace model id (must match the classifier's _MLX_MODEL_ID)",
    )
    parser.add_argument(
        "--top", type=int, default=0,
        help="Show only the top-N slowest sessions (by elapsed_s). 0 = all.",
    )
    parser.add_argument(
        "--csv", action="store_true",
        help="Emit machine-readable CSV instead of the human table",
    )
    args = parser.parse_args(argv[1:])

    log_path = Path(args.path) if args.path else _latest_log()
    if not log_path.exists():
        sys.exit(f"log file {log_path} does not exist")

    records = _read_records(log_path)
    if not records:
        sys.exit(f"no usable records in {log_path}")

    print(f"reading {log_path}  ({len(records)} records)", file=sys.stderr)
    print(f"loading tokenizer for {args.model_id}…", file=sys.stderr)
    import mlx_lm                       # local import — heavy dep
    _, tok = mlx_lm.load(args.model_id)

    rows: list[dict] = []
    for rec in records:
        result = rec["result"] or {}
        messages = _format_messages(rec)
        prompt = tok.apply_chat_template(
            messages, tokenize=False, add_generation_prompt=True
        )
        in_tok = len(tok.encode(prompt))

        result_json = json.dumps(result, ensure_ascii=False)
        out_tok = len(tok.encode(result_json))

        rows.append({
            "session_id":  result["session_id"],
            "task_key":    result.get("task_key") or "-",
            "in_tok":      in_tok,
            "out_tok":     out_tok,
            "elapsed_s":   float(result.get("elapsed_s", 0.0)),
            "tok_per_sec": out_tok / float(result.get("elapsed_s", 1.0) or 1.0),
        })

    if args.top:
        rows.sort(key=lambda r: r["elapsed_s"], reverse=True)
        rows = rows[: args.top]

    if args.csv:
        writer_cols = ["session_id", "task_key", "in_tok", "out_tok",
                       "elapsed_s", "tok_per_sec"]
        sys.stdout.write(",".join(writer_cols) + "\n")
        for r in rows:
            sys.stdout.write(",".join(str(r[c]) for c in writer_cols) + "\n")
        return 0

    # Human-readable table
    hdr = (
        f"{'session_id':>11} {'task_key':>9} {'in_tok':>7} "
        f"{'out_tok':>7} {'elapsed_s':>10} {'tok/s':>7}"
    )
    print()
    print(hdr)
    print("-" * len(hdr))
    for r in rows:
        print(
            f"{r['session_id']:>11} "
            f"{r['task_key']:>9} "
            f"{r['in_tok']:>7} "
            f"{r['out_tok']:>7} "
            f"{r['elapsed_s']:>10.2f} "
            f"{r['tok_per_sec']:>7.1f}"
        )

    # Aggregate row
    if rows:
        n = len(rows)
        avg_in = sum(r["in_tok"] for r in rows) / n
        avg_out = sum(r["out_tok"] for r in rows) / n
        avg_elapsed = sum(r["elapsed_s"] for r in rows) / n
        total_out = sum(r["out_tok"] for r in rows)
        total_elapsed = sum(r["elapsed_s"] for r in rows)
        avg_tok_per_sec = total_out / total_elapsed if total_elapsed else 0
        print("-" * len(hdr))
        print(
            f"{'avg':>11} {'(' + str(n) + ')':>9} "
            f"{avg_in:>7.0f} {avg_out:>7.0f} "
            f"{avg_elapsed:>10.2f} {avg_tok_per_sec:>7.1f}"
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
