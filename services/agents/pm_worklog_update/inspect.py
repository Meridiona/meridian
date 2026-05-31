"""Inspector — show the SessionBundle for one ticket × one window.

Read-only CLI for eyeballing what the Synth would see. Boots no agno,
no LLM, no Jira — just runs the same query that `run_cycle` uses and
pretty-prints the bundle.

Usage:

    cd services
    .venv/bin/python -m agents.pm_worklog_update.inspect --task KAN-64

    # explicit window
    .venv/bin/python -m agents.pm_worklog_update.inspect --task KAN-64 \\
        --window-start 2026-05-28T09:00:00Z \\
        --window-end   2026-05-28T10:00:00Z

    # dump full session_text excerpts (helpful when debugging the prompt)
    .venv/bin/python -m agents.pm_worklog_update.inspect --task KAN-64 --full-text

    # machine-readable JSON for piping into jq
    .venv/bin/python -m agents.pm_worklog_update.inspect --task KAN-64 --json
"""
from __future__ import annotations

import argparse
import json
import sys
from collections import Counter
from datetime import datetime, timedelta, timezone

from agents.pm_worklog_update import config, db


# ──────────────────────── CLI ──────────────────────────────────────────────────


def _parse_iso(s: str) -> datetime:
    cleaned = s.replace("Z", "+00:00")
    dt = datetime.fromisoformat(cleaned)
    if dt.tzinfo is None:
        dt = dt.replace(tzinfo=timezone.utc)
    return dt.astimezone(timezone.utc)


def build_parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(
        prog="agents.pm_worklog_update.inspect",
        description="Show classified-session data for one Jira ticket in a window.",
    )
    p.add_argument("--task", required=True, help="Jira ticket key, e.g. KAN-64")
    p.add_argument(
        "--window-start",
        type=_parse_iso,
        default=None,
        help="ISO-8601 UTC start. Default: now - PM_WORKLOG_INTERVAL_HOURS",
    )
    p.add_argument(
        "--window-end",
        type=_parse_iso,
        default=None,
        help="ISO-8601 UTC end. Default: now",
    )
    p.add_argument(
        "--full-text",
        action="store_true",
        help="Dump each session's full excerpt (2KB cap from db.py).",
    )
    p.add_argument(
        "--json",
        dest="emit_json",
        action="store_true",
        help="Emit the SessionBundle as JSON on stdout instead of the human view.",
    )
    return p


# ──────────────────────── Formatting helpers ───────────────────────────────────


def _fmt_secs(s: int) -> str:
    """Format seconds as a compact h/m/s string."""
    if s < 60:
        return f"{s}s"
    if s < 3600:
        return f"{s // 60}m {s % 60}s"
    h, rem = divmod(s, 3600)
    m, _ = divmod(rem, 60)
    return f"{h}h {m}m"


def _fmt_local(iso_utc: str) -> str:
    """Render a UTC ISO timestamp as the host TZ in HH:MM:SS for readability."""
    cleaned = iso_utc.replace("Z", "+00:00")
    dt = datetime.fromisoformat(cleaned).astimezone()
    return dt.strftime("%H:%M:%S")


# ──────────────────────── Human view ───────────────────────────────────────────


def render_human(bundle, *, full_text: bool) -> str:
    out: list[str] = []
    line = "─" * 78

    out.append(line)
    out.append(f" Ticket : {bundle.task_key}")
    if bundle.pm_task_title:
        out.append(f" Title  : {bundle.pm_task_title}")
    if bundle.pm_task_status:
        out.append(f" Status : {bundle.pm_task_status}")
    if bundle.assignee_name:
        out.append(f" Assignee: {bundle.assignee_name}")
    out.append(f" Window : {bundle.window_start}  →  {bundle.window_end}")
    out.append(f" Cycle  : {bundle.cycle_index}")
    out.append(line)

    out.append(
        f" Sessions     : {len(bundle.sessions)}"
        f"   (total {_fmt_secs(bundle.total_seconds)},"
        f" real {_fmt_secs(bundle.real_seconds)})"
    )
    out.append(
        f" Raw text     : {bundle.raw_text_bytes:,} bytes"
        f"   (heavy: {bundle.is_heavy})"
    )
    if bundle.earlier_today_summaries:
        out.append(" Earlier today:")
        for s in bundle.earlier_today_summaries:
            out.append(f"   - {s}")

    # Aggregate dimensions across all sessions in the window
    dim_counts: dict[str, Counter] = {}
    app_secs: Counter = Counter()
    for s in bundle.sessions:
        app_secs[s.app_name] += s.duration_s
        for dim, vals in s.dimensions.items():
            dim_counts.setdefault(dim, Counter()).update(vals)

    if app_secs:
        out.append("")
        out.append(" Apps (by time):")
        for name, secs in sorted(app_secs.items(), key=lambda kv: -kv[1])[:8]:
            out.append(f"   {name:<24} {_fmt_secs(secs):>10}")

    if dim_counts:
        out.append("")
        out.append(" Dimensions (top values):")
        for dim, counts in sorted(dim_counts.items()):
            tops = ", ".join(f"{v}×{n}" for v, n in counts.most_common(5))
            out.append(f"   {dim:<12} {tops}")

    out.append("")
    out.append(line)
    out.append(f" Per-session breakdown ({len(bundle.sessions)} rows)")
    out.append(line)

    if not bundle.sessions:
        out.append(" (no classified sessions in this window)")
    else:
        out.append(
            f" {'id':>6}  {'started':>8}  {'dur':>9}  {'real':>9}  "
            f"{'app':<18}  top_title"
        )
        out.append(" " + ("─" * 76))
        for s in bundle.sessions:
            real = s.duration_s - s.idle_frame_s
            top = (s.top_titles or [""])[0][:30]
            out.append(
                f" {s.id:>6}  {_fmt_local(s.started_at):>8}  "
                f"{_fmt_secs(s.duration_s):>9}  {_fmt_secs(real):>9}  "
                f"{s.app_name[:18]:<18}  {top}"
            )

    if full_text:
        out.append("")
        out.append(line)
        out.append(" Full session excerpts (capped at 2KB each in the bundle)")
        out.append(line)
        for s in bundle.sessions:
            out.append("")
            out.append(f" === session {s.id} — {s.app_name} — {_fmt_secs(s.duration_s)} ===")
            if s.top_titles:
                out.append(f" top_titles: {s.top_titles}")
            if s.dimensions:
                out.append(f" dimensions: {s.dimensions}")
            out.append("")
            out.append(s.excerpt or "(no text)")
    return "\n".join(out)


# ──────────────────────── Entry point ──────────────────────────────────────────


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    end = args.window_end or datetime.now(timezone.utc)
    start = args.window_start or (end - timedelta(hours=config.PM_WORKLOG_INTERVAL_HOURS))
    if start >= end:
        sys.stderr.write(f"window-start ({start}) must be earlier than window-end ({end})\n")
        return 2

    db.init_schema()
    bundle = db.fetch_session_bundle(
        task_key=args.task,
        window_start=start,
        window_end=end,
        cycle_index=0,
    )

    if args.emit_json:
        sys.stdout.write(bundle.model_dump_json(indent=2) + "\n")
    else:
        sys.stdout.write(render_human(bundle, full_text=args.full_text) + "\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
