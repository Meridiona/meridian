#!/usr/bin/env python3
"""Count input + output tokens for classifier runs or pm-update synth inputs.

Two modes:

  classifier (default)
    Reads `services/logs/mlx/server_*.jsonl` produced by the FastAPI MLX
    server and prints per-session token counts using the same tokenizer the
    classifier loads.

  pm-update  (--pm-update --task-key KAN-XX)
    Queries meridian.db directly, builds the synth agent prompt using
    session_summary (falling back to 2KB session_text for legacy rows),
    and prints the token budget breakdown.

Usage:
    cd services
    .venv/bin/python scripts/count_classifier_tokens.py
    .venv/bin/python scripts/count_classifier_tokens.py --top 20
    .venv/bin/python scripts/count_classifier_tokens.py logs/mlx/server_20260528T175639.jsonl

    .venv/bin/python scripts/count_classifier_tokens.py --pm-update --task-key KAN-142
    .venv/bin/python scripts/count_classifier_tokens.py --pm-update --task-key KAN-142 --hours 2
    .venv/bin/python scripts/count_classifier_tokens.py --pm-update --all-tasks --hours 1
"""
from __future__ import annotations

import argparse
import json
import sqlite3
import sys
from datetime import datetime, timedelta, timezone
from pathlib import Path

LOG_DIR = Path(__file__).resolve().parent.parent / "logs" / "mlx"
SERVICES_DIR = Path(__file__).resolve().parent.parent
DEFAULT_MODEL_ID = "mlx-community/Qwen3.5-9B-OptiQ-4bit"
EXCERPT_CAP = 2_000   # fallback cap for rows without session_summary
TOP_TITLES_N = 3


# ─────────────────────── classifier mode helpers ───────────────────────────────


def _latest_log() -> Path:
    files = sorted(LOG_DIR.glob("server_*.jsonl"))
    if not files:
        sys.exit(f"no log files found under {LOG_DIR}")
    return files[-1]


def _read_records(path: Path) -> list[dict]:
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


def _format_messages_classifier(rec: dict) -> list[dict]:
    return [
        {"role": "system", "content": rec.get("system_prompt", "") or ""},
        {"role": "user",   "content": rec.get("user_message", "") or ""},
    ]


# ─────────────────────── pm-update mode helpers ────────────────────────────────


def _load_skill(name: str) -> str:
    search = [
        SERVICES_DIR / "skills" / "activity" / name / "SKILL.md",
        SERVICES_DIR / "skills" / name / "SKILL.md",
    ]
    for p in search:
        if p.exists():
            return p.read_text()
    return ""


def _meridian_db() -> Path:
    import os
    raw = os.environ.get("MERIDIAN_DB", str(Path.home() / ".meridian" / "meridian.db"))
    return Path(raw)


def _parse_top_titles(raw: str | None, n: int = TOP_TITLES_N) -> list[str]:
    if not raw:
        return []
    try:
        parsed = json.loads(raw)
    except json.JSONDecodeError:
        return []
    titles = sorted(
        (p for p in parsed if isinstance(p, dict) and p.get("title")),
        key=lambda p: p.get("count", 0),
        reverse=True,
    )
    return [p["title"] for p in titles[:n]]


def _fetch_sessions(
    task_key: str,
    since: datetime,
    db_path: Path,
) -> list[dict]:
    con = sqlite3.connect(db_path)
    con.row_factory = sqlite3.Row
    since_iso = since.astimezone(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
    rows = con.execute(
        """
        SELECT id, app_name, started_at, ended_at, duration_s,
               window_titles, session_text, session_summary, category,
               session_text_source,
               COALESCE(idle_frame_count, 0) AS idle_frame_count,
               COALESCE(frame_count, 0)      AS frame_count
        FROM app_sessions
        WHERE task_key = ?
          AND COALESCE(task_session_type, '') = 'task'
          AND started_at >= ?
        ORDER BY id ASC
        """,
        (task_key, since_iso),
    ).fetchall()
    con.close()
    return [dict(r) for r in rows]


def _fetch_tasks_with_recent_sessions(since: datetime, db_path: Path) -> list[str]:
    con = sqlite3.connect(db_path)
    con.row_factory = sqlite3.Row
    since_iso = since.astimezone(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
    rows = con.execute(
        """
        SELECT DISTINCT task_key FROM app_sessions
        WHERE task_key IS NOT NULL AND task_key != ''
          AND COALESCE(task_session_type, '') = 'task'
          AND started_at >= ?
        ORDER BY task_key
        """,
        (since_iso,),
    ).fetchall()
    con.close()
    return [r["task_key"] for r in rows]


def _render_synth_input(task_key: str, sessions: list[dict]) -> str:
    """Reproduce the user message the synth agent receives."""
    lines: list[str] = [f"# TICKET: {task_key}", ""]
    lines.append(f"# SESSIONS ({len(sessions)})")
    for s in sessions:
        lines.append(f"## session {s['id']} — {s['app_name']} — {s['duration_s']}s")
        titles = _parse_top_titles(s.get("window_titles"))
        if titles:
            lines.append(f"top_titles: {titles}")
        summary = (s.get("session_summary") or "").strip()
        text    = (s.get("session_text") or "").strip()
        # Mirror db.py:199 — prefer summary, fall back to 2KB excerpt
        excerpt = summary if summary else text[:EXCERPT_CAP]
        source  = "summary" if summary else s.get("session_text_source", "ocr")
        if excerpt:
            lines.append(f"excerpt [{source}]:\n{excerpt}\n")
    return "\n".join(lines)


def _run_pm_update_mode(
    task_keys: list[str],
    hours: float,
    model_id: str,
    db_path: Path,
) -> int:
    since = datetime.now(timezone.utc) - timedelta(hours=hours)

    skill = _load_skill("pm-worklog-synth")
    if not skill:
        print("warning: pm-worklog-synth SKILL.md not found — system prompt will be empty",
              file=sys.stderr)

    # Qwen3 averages ~3.8 chars/token for English prose — accurate enough for
    # budget planning without loading model weights.
    def _tok(text: str) -> int:
        return max(1, round(len(text) / 3.8))

    skill_toks = _tok(skill)
    print(f"\npm-update synth token budget  (last {hours:.0f}h, ~3.8 chars/tok estimate)")
    print(f"  system prompt (SKILL.md): ~{skill_toks} tokens  ({len(skill)} chars)")
    print()

    hdr = f"{'task_key':>10}  {'sessions':>8}  {'sum_chars':>9}  {'txt_chars':>9}  {'user_toks':>9}  {'total_in':>9}  {'output_cap':>10}"
    print(hdr)
    print("-" * len(hdr))

    for task_key in task_keys:
        sessions = _fetch_sessions(task_key, since, db_path)
        if not sessions:
            print(f"{task_key:>10}  {'0':>8}  {'—':>9}  {'—':>9}  {'—':>9}  {'—':>9}  {'—':>10}  (no sessions)")
            continue

        sum_chars = sum(len((s.get("session_summary") or "").strip()) for s in sessions)
        txt_chars = sum(len((s.get("session_text") or "").strip()) for s in sessions)

        user_msg  = _render_synth_input(task_key, sessions)
        user_toks = _tok(user_msg)
        total_in  = skill_toks + user_toks

        print(
            f"{task_key:>10}  {len(sessions):>8}  {sum_chars:>9}  {txt_chars:>9}"
            f"  {user_toks:>9}  {total_in:>9}  {'8000':>10}"
        )

        # Per-session breakdown
        for s in sessions:
            summary = (s.get("session_summary") or "").strip()
            text    = (s.get("session_text") or "").strip()
            excerpt = summary if summary else text[:EXCERPT_CAP]
            source  = "summary" if summary else "ocr_excerpt"
            e_toks  = _tok(excerpt) if excerpt else 0
            print(
                f"  {'id=' + str(s['id']):>12}  {s['app_name'][:20]:<20}"
                f"  {source:<12}  excerpt_chars={len(excerpt):>5}  excerpt_toks~{e_toks:>4}"
            )

    print()
    print("context window: 262k tokens (MLX server).  output cap: 8000 tokens (PM_UPDATE_SYNTH_MAX_TOKENS)")
    return 0


# ─────────────────────── classifier mode ───────────────────────────────────────


def _run_classifier_mode(args: argparse.Namespace) -> int:
    log_path = Path(args.path) if args.path else _latest_log()
    if not log_path.exists():
        sys.exit(f"log file {log_path} does not exist")

    records = _read_records(log_path)
    if not records:
        sys.exit(f"no usable records in {log_path}")

    print(f"reading {log_path}  ({len(records)} records)", file=sys.stderr)

    def _tok(text: str) -> int:
        return max(1, round(len(text) / 3.8))

    rows: list[dict] = []
    for rec in records:
        result   = rec["result"] or {}
        messages = _format_messages_classifier(rec)
        prompt   = "\n".join(m["content"] for m in messages)
        in_tok   = _tok(prompt)
        out_tok  = _tok(json.dumps(result, ensure_ascii=False))
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
        cols = ["session_id", "task_key", "in_tok", "out_tok", "elapsed_s", "tok_per_sec"]
        sys.stdout.write(",".join(cols) + "\n")
        for r in rows:
            sys.stdout.write(",".join(str(r[c]) for c in cols) + "\n")
        return 0

    hdr = (
        f"{'session_id':>11} {'task_key':>9} {'in_tok':>7} "
        f"{'out_tok':>7} {'elapsed_s':>10} {'tok/s':>7}"
    )
    print()
    print(hdr)
    print("-" * len(hdr))
    for r in rows:
        print(
            f"{r['session_id']:>11} {r['task_key']:>9} {r['in_tok']:>7} "
            f"{r['out_tok']:>7} {r['elapsed_s']:>10.2f} {r['tok_per_sec']:>7.1f}"
        )

    if rows:
        n           = len(rows)
        avg_in      = sum(r["in_tok"] for r in rows) / n
        avg_out     = sum(r["out_tok"] for r in rows) / n
        avg_elapsed = sum(r["elapsed_s"] for r in rows) / n
        total_out   = sum(r["out_tok"] for r in rows)
        total_el    = sum(r["elapsed_s"] for r in rows)
        avg_tps     = total_out / total_el if total_el else 0
        print("-" * len(hdr))
        print(
            f"{'avg':>11} {'(' + str(n) + ')':>9} "
            f"{avg_in:>7.0f} {avg_out:>7.0f} "
            f"{avg_elapsed:>10.2f} {avg_tps:>7.1f}"
        )
    return 0


# ─────────────────────── entry point ───────────────────────────────────────────


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(prog="count_classifier_tokens")

    # classifier mode args
    parser.add_argument("path", nargs="?", default=None,
                        help="JSONL log path (classifier mode). Defaults to most recent.")
    parser.add_argument("--model-id", default=DEFAULT_MODEL_ID,
                        help="HuggingFace model id (must match MLX_MODEL_ID)")
    parser.add_argument("--top", type=int, default=0,
                        help="Show only top-N slowest sessions (0 = all)")
    parser.add_argument("--csv", action="store_true",
                        help="Emit CSV instead of human table")

    # pm-update mode args
    parser.add_argument("--pm-update", action="store_true",
                        help="Switch to pm-update synth token mode")
    parser.add_argument("--task-key", default=None,
                        help="Jira task key to analyse (pm-update mode)")
    parser.add_argument("--all-tasks", action="store_true",
                        help="Analyse all tasks with sessions in the window (pm-update mode)")
    parser.add_argument("--hours", type=float, default=1.0,
                        help="How many hours back to look (pm-update mode, default 1)")

    args = parser.parse_args(argv[1:])

    if args.pm_update:
        db_path = _meridian_db()
        if not db_path.exists():
            sys.exit(f"meridian.db not found at {db_path}")

        since = datetime.now(timezone.utc) - timedelta(hours=args.hours)

        if args.all_tasks:
            task_keys = _fetch_tasks_with_recent_sessions(since, db_path)
            if not task_keys:
                sys.exit(f"no classified sessions in the last {args.hours:.0f}h")
        elif args.task_key:
            task_keys = [args.task_key]
        else:
            # default: all tasks with recent sessions
            task_keys = _fetch_tasks_with_recent_sessions(since, db_path)
            if not task_keys:
                sys.exit(f"no classified sessions in the last {args.hours:.0f}h")

        return _run_pm_update_mode(task_keys, args.hours, args.model_id, db_path)

    return _run_classifier_mode(args)


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
