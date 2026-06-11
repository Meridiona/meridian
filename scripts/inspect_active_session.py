#!/usr/bin/env python3
# ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
"""Pretty-print the contents of active_session (or any app_session by id).

Usage:
  python3 scripts/inspect_active_session.py            # active session
  python3 scripts/inspect_active_session.py 1466       # a closed session by id
  python3 scripts/inspect_active_session.py --full     # no truncation
  MERIDIAN_DB=/path/to/db python3 scripts/inspect_active_session.py
"""
from __future__ import annotations

import argparse
import json
import os
import sqlite3
import sys
import textwrap
from pathlib import Path

DEFAULT_DB = Path.home() / ".meridian" / "meridian.db"
TRUNC = 400  # default per-text-blob char cap
WRAP = 110


def hr(char: str = "─") -> str:
    return char * WRAP


def header(title: str) -> str:
    return f"\n{hr('═')}\n  {title}\n{hr('═')}"


def section(title: str, count: int | None = None) -> str:
    suffix = f" ({count})" if count is not None else ""
    return f"\n{hr()}\n  {title}{suffix}\n{hr()}"


def fmt_bytes(b: int) -> str:
    for unit in ("B", "KB", "MB"):
        if b < 1024:
            return f"{b:.1f} {unit}"
        b /= 1024
    return f"{b:.1f} GB"


def trunc(text: str, cap: int) -> str:
    if cap <= 0 or len(text) <= cap:
        return text
    return text[:cap] + f"… [+{len(text) - cap} chars]"


def wrap(text: str, indent: str = "    ") -> str:
    return textwrap.fill(
        text,
        width=WRAP,
        initial_indent=indent,
        subsequent_indent=indent,
        replace_whitespace=False,
        drop_whitespace=False,
    )


def load_row(db_path: Path, session_id: int | None) -> sqlite3.Row | None:
    conn = sqlite3.connect(db_path)
    conn.row_factory = sqlite3.Row
    if session_id is None:
        return conn.execute("SELECT * FROM active_session WHERE id = 1").fetchone()
    return conn.execute(
        "SELECT * FROM app_sessions WHERE id = ?", (session_id,)
    ).fetchone()


def parse_json(blob: str | None) -> list | dict | None:
    if not blob:
        return None
    try:
        return json.loads(blob)
    except json.JSONDecodeError:
        return blob  # raw fallback


def render_meta(row: sqlite3.Row, source: str) -> None:
    print(header(f"{source}  —  id={row['id']}  app={row['app_name']!r}"))
    print(f"  started_at        {row['started_at']}")
    end_col = "last_seen_at" if source == "active_session" else "ended_at"
    print(f"  {end_col:<17} {row[end_col]}")
    if source == "app_sessions":
        print(f"  duration_s        {row['duration_s']}")
    print(f"  frame_count       {row['frame_count']}  (idle={row['idle_frame_count']})")
    print(f"  frame_id range    {row['min_frame_id']} → {row['max_frame_id']}")
    print(f"  category          {row['category']}  (conf={row['confidence']:.2f})")


def render_window_titles(blob: str | None) -> None:
    titles = parse_json(blob) or []
    print(section("WINDOW TITLES", len(titles)))
    if not titles:
        print("  (none)")
        return
    for t in titles:
        name = t.get("window_name", "") if isinstance(t, dict) else str(t)
        count = t.get("count", "") if isinstance(t, dict) else ""
        print(f"  [{count:>3}×] {trunc(name, 200)}")


def render_ocr(blob: str | None, cap: int) -> None:
    samples = parse_json(blob) or []
    blob_size = len(blob or "")
    print(section(f"OCR SAMPLES  [{fmt_bytes(blob_size)} on disk]", len(samples)))
    if not samples:
        print("  (none)")
        return
    for i, s in enumerate(samples, 1):
        if not isinstance(s, dict):
            print(f"  [{i}]  {trunc(str(s), cap)}")
            continue
        ts = s.get("timestamp", "")
        win = s.get("window_name") or "—"
        text = s.get("text", "")
        print(f"\n  ── sample {i}/{len(samples)} ── {ts}")
        print(f"     window: {trunc(win, 120)}")
        print(f"     text   ({len(text)} chars):")
        print(wrap(trunc(text, cap), indent="       "))


def render_elements(blob: str | None, cap: int) -> None:
    samples = parse_json(blob) or []
    blob_size = len(blob or "")
    print(section(f"AX-TREE / ELEMENTS SAMPLES  [{fmt_bytes(blob_size)} on disk]", len(samples)))
    if not samples:
        print("  (none)")
        return
    for i, s in enumerate(samples, 1):
        if not isinstance(s, dict):
            print(f"  [{i}]  {trunc(str(s), cap)}")
            continue
        ts = s.get("timestamp", "")
        role = s.get("element_type") or s.get("role") or "—"
        text = s.get("text") or s.get("value") or ""
        win = s.get("window_name") or "—"
        print(f"\n  ── element {i}/{len(samples)} ── {ts}")
        print(f"     role:   {role}")
        print(f"     window: {trunc(win, 120)}")
        if text:
            print(f"     text   ({len(text)} chars):")
            print(wrap(trunc(text, cap), indent="       "))
        # show any other fields not already covered
        extras = {k: v for k, v in s.items() if k not in {"timestamp", "element_type", "role", "text", "value", "window_name"}}
        if extras:
            print(f"     extras: {trunc(json.dumps(extras, ensure_ascii=False), 200)}")


def render_audio(blob: str | None, cap: int) -> None:
    snippets = parse_json(blob) or []
    blob_size = len(blob or "")
    print(section(f"AUDIO SNIPPETS  [{fmt_bytes(blob_size)} on disk]", len(snippets)))
    if not snippets:
        print("  (none)")
        return
    for i, s in enumerate(snippets, 1):
        if not isinstance(s, dict):
            print(f"  [{i}]  {trunc(str(s), cap)}")
            continue
        ts = s.get("timestamp") or s.get("started_at", "")
        speaker = s.get("speaker") or s.get("device") or "—"
        text = s.get("transcription") or s.get("text", "")
        print(f"\n  ── snippet {i}/{len(snippets)} ── {ts}")
        print(f"     speaker/device: {speaker}")
        if text:
            print(f"     text   ({len(text)} chars):")
            print(wrap(trunc(text, cap), indent="       "))


def render_signals(blob: str | None) -> None:
    sig = parse_json(blob)
    print(section("SIGNALS"))
    if not sig:
        print("  (none)")
        return
    print(textwrap.indent(json.dumps(sig, indent=2, ensure_ascii=False), "  "))


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("session_id", nargs="?", type=int, default=None,
                    help="app_sessions.id; omit for active_session")
    ap.add_argument("--db", type=Path, default=Path(os.environ.get("MERIDIAN_DB", DEFAULT_DB)),
                    help=f"path to meridian.db (default: {DEFAULT_DB})")
    ap.add_argument("--full", action="store_true", help="no truncation of text blobs")
    ap.add_argument("--cap", type=int, default=TRUNC, help=f"per-blob char cap (default: {TRUNC})")
    args = ap.parse_args()

    if not args.db.exists():
        print(f"error: db not found at {args.db}", file=sys.stderr)
        return 2

    cap = 0 if args.full else args.cap
    source = "active_session" if args.session_id is None else "app_sessions"
    row = load_row(args.db, args.session_id)
    if row is None:
        print(f"error: no row in {source}" + (f" with id={args.session_id}" if args.session_id else ""), file=sys.stderr)
        return 1

    render_meta(row, source)
    render_window_titles(row["window_titles"])
    render_ocr(row["ocr_samples"], cap)
    render_elements(row["elements_samples"], cap)
    render_audio(row["audio_snippets"], cap)
    render_signals(row["signals"])
    print()
    return 0


if __name__ == "__main__":
    sys.exit(main())
