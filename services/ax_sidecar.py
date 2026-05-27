"""
ax_sidecar — injects Claude Code session transcripts into screenpipe's frames table.

Watches `~/.claude/projects/<sanitized-cwd>/<session-uuid>.jsonl` files —
Claude Code's persistent conversation log. Every poll (3s by default), any
session file modified within the last 30s is re-parsed into a flat
`[user] ... [assistant] ...` transcript and inserted as an accessibility
frame so it shows up in screenpipe search and Meridian's ETL.

Deduplicates per-session on content hash so unchanged sessions don't
generate rows. The frame is tagged `app_name='Code'`, `device_name='claude-session'`,
`capture_trigger='claude_session'`, `text_source='accessibility'`.

Usage:
  python -m services.ax_sidecar            # run in foreground
  # or via launchd — see services/scripts/install-ax-sidecar.sh
"""

import hashlib
import json
import logging
import os
import sqlite3
import sys
import time
from datetime import datetime, timezone
from pathlib import Path
from typing import Dict, Iterable, List, Tuple

logging.basicConfig(
    level=logging.INFO,
    format="%(asctime)s [ax-sidecar] %(levelname)s %(message)s",
    datefmt="%Y-%m-%dT%H:%M:%S",
    stream=sys.stdout,
)
log = logging.getLogger(__name__)

SCREENPIPE_DB = Path(
    os.environ.get("SCREENPIPE_DB", "~/.screenpipe/db.sqlite")
).expanduser()

CLAUDE_PROJECTS_DIR = Path(
    os.environ.get("CLAUDE_PROJECTS_DIR", "~/.claude/projects")
).expanduser()

POLL_INTERVAL = float(os.environ.get("AX_SIDECAR_INTERVAL", "3"))

# Sessions whose .jsonl was modified within the last MAX_AGE seconds are
# considered "active" and will be re-checked. Anything older is ignored.
MAX_AGE_SECS = float(os.environ.get("AX_SIDECAR_MAX_AGE", "30"))

DEVICE_NAME = "claude-session"
CAPTURE_TRIGGER = "claude_session"
APP_NAME = "Code"


def _content_hash(text: str) -> int:
    """Stable 64-bit signed hash for dedup (matches screenpipe's i64 column)."""
    digest = hashlib.blake2b(text.encode(), digest_size=8).digest()
    value = int.from_bytes(digest, "big", signed=False)
    if value >= 2**63:
        value -= 2**64
    return value


def _format_content(content) -> str:
    """Flatten Claude Code's content (string or list of blocks) into text."""
    if isinstance(content, str):
        return content
    if not isinstance(content, list):
        return ""
    parts: List[str] = []
    for block in content:
        if not isinstance(block, dict):
            parts.append(str(block))
            continue
        btype = block.get("type")
        if btype == "text":
            parts.append(block.get("text", ""))
        elif btype == "tool_use":
            name = block.get("name", "?")
            inp = block.get("input")
            inp_repr = json.dumps(inp, ensure_ascii=False)[:400] if inp else ""
            parts.append(f"[tool_use: {name} {inp_repr}]".rstrip())
        elif btype == "tool_result":
            tr = block.get("content", "")
            if isinstance(tr, list):
                tr = "\n".join(
                    p.get("text", "") if isinstance(p, dict) else str(p) for p in tr
                )
            tr_str = str(tr)
            if len(tr_str) > 500:
                tr_str = tr_str[:500] + "…"
            parts.append(f"[tool_result: {tr_str}]")
        elif btype == "thinking":
            t = block.get("thinking", "")
            if t:
                parts.append(f"[thinking] {t}")
    return "\n".join(p for p in parts if p)


def _parse_session(jsonl_path: Path) -> str:
    """Parse a Claude Code session JSONL into a `[role] text` transcript."""
    lines: List[str] = []
    try:
        with jsonl_path.open("rb") as fh:
            for raw in fh:
                try:
                    d = json.loads(raw)
                except Exception:
                    continue
                rec_type = d.get("type")
                if rec_type not in ("user", "assistant"):
                    continue
                msg = d.get("message") or {}
                role = msg.get("role") or rec_type
                content_str = _format_content(msg.get("content", ""))
                if content_str.strip():
                    lines.append(f"[{role}] {content_str}")
    except FileNotFoundError:
        return ""
    return "\n\n".join(lines)


def _scan_active_sessions(max_age: float) -> Iterable[Tuple[Path, Path]]:
    """Yield (project_dir, jsonl_path) for sessions modified within max_age seconds."""
    if not CLAUDE_PROJECTS_DIR.exists():
        return
    cutoff = time.time() - max_age
    for proj in CLAUDE_PROJECTS_DIR.iterdir():
        if not proj.is_dir():
            continue
        for f in proj.glob("*.jsonl"):
            try:
                if f.stat().st_mtime >= cutoff:
                    yield (proj, f)
            except FileNotFoundError:
                continue


def _window_name(project_dir: Path) -> str:
    """Render the window_name shown in screenpipe rows."""
    name = project_dir.name  # e.g. "-Users-akarshhegde-Documents-Meridiona-meridian"
    # Truncate long paths for the screenpipe column (UI shows ~50 chars).
    if len(name) > 60:
        name = name[:60]
    return f"Claude Code — {name}"


def _insert_frame(text: str, window_name: str, hash_val: int) -> None:
    """Write one claude-session frame into screenpipe's frames table."""
    conn = sqlite3.connect(str(SCREENPIPE_DB), timeout=10, isolation_level=None)
    conn.execute("PRAGMA journal_mode=WAL")
    conn.execute("PRAGMA busy_timeout=5000")
    ts = datetime.now(timezone.utc)
    now = ts.strftime("%Y-%m-%dT%H:%M:%S.") + f"{ts.microsecond // 1000:03d}Z"
    conn.execute(
        """
        INSERT INTO frames
            (offset_index, timestamp, app_name, window_name, focused,
             device_name, accessibility_text, content_hash, capture_trigger,
             text_source)
        VALUES (0, ?, ?, ?, 1, ?, ?, ?, ?, 'accessibility')
        """,
        (now, APP_NAME, window_name, DEVICE_NAME, text, hash_val, CAPTURE_TRIGGER),
    )
    conn.close()


def main() -> None:
    if not SCREENPIPE_DB.exists():
        log.error("screenpipe DB not found at %s", SCREENPIPE_DB)
        sys.exit(1)
    if not CLAUDE_PROJECTS_DIR.exists():
        log.error(
            "Claude projects dir not found at %s — is Claude Code installed?",
            CLAUDE_PROJECTS_DIR,
        )
        sys.exit(1)

    log.info(
        "starting — DB=%s projects=%s poll=%.0fs max-age=%.0fs",
        SCREENPIPE_DB,
        CLAUDE_PROJECTS_DIR,
        POLL_INTERVAL,
        MAX_AGE_SECS,
    )

    last_hash: Dict[str, int] = {}

    while True:
        try:
            for proj, jsonl in _scan_active_sessions(MAX_AGE_SECS):
                text = _parse_session(jsonl)
                if not text:
                    continue
                h = _content_hash(text)
                key = str(jsonl)
                if last_hash.get(key) == h:
                    continue
                _insert_frame(text, _window_name(proj), h)
                last_hash[key] = h
                log.info(
                    "inserted %d chars — %s",
                    len(text),
                    jsonl.stem[:8],
                )
        except Exception as exc:
            log.error("error: %s", exc)

        time.sleep(POLL_INTERVAL)


if __name__ == "__main__":
    main()
