"""
ax_sidecar — captures terminal coding-agent conversations into screenpipe.

Watches the persistent session logs that local coding agents write to disk
and inserts each conversation's transcript as an accessibility frame into
screenpipe's frames table. This lets Meridian and downstream tooling search
across agent conversations the same way they search screen activity.

Currently supports two sources:

  * Claude Code      ~/.claude/projects/<sanitized-cwd>/<uuid>.jsonl
  * Codex (CLI/TUI)  ~/.codex/sessions/<YYYY>/<MM>/<DD>/rollout-*.jsonl

Each tick (3s by default) the daemon scans for session files modified within
the last `AX_SIDECAR_MAX_AGE` seconds, parses them into a flat
`[user] ... [claude-code|codex] ...` transcript, and inserts a new row only
when the per-session content hash has changed.

Frames are tagged:
  device_name      = 'ax-sidecar'
  text_source      = 'accessibility'
  capture_trigger  = 'claude_session' or 'codex_session'
  app_name         = resolved from the most-recent non-sidecar screenpipe
                     frame (the terminal/IDE actually hosting the agent —
                     e.g. 'Code', 'Terminal', 'iTerm2', 'Ghostty')

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
from typing import Dict, Iterator, List, Tuple

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

CODEX_SESSIONS_DIR = Path(
    os.environ.get("CODEX_SESSIONS_DIR", "~/.codex/sessions")
).expanduser()

POLL_INTERVAL = float(os.environ.get("AX_SIDECAR_INTERVAL", "3"))
MAX_AGE_SECS = float(os.environ.get("AX_SIDECAR_MAX_AGE", "30"))

DEVICE_NAME = "ax-sidecar"

# Window (in seconds) the host-app resolver looks back over for the most
# recent screenpipe frame whose app_name isn't ours. Wide enough to survive
# a few skipped screenpipe ticks, narrow enough not to inherit a stale app.
HOST_APP_LOOKBACK_SECS = float(os.environ.get("AX_SIDECAR_HOST_APP_LOOKBACK", "15"))

# Used when no recent screenpipe frame is available (e.g. screenpipe stopped
# or the user has been idle longer than HOST_APP_LOOKBACK_SECS).
FALLBACK_APP_NAME = "unknown"

# ---------------------------------------------------------------------------
# Common helpers
# ---------------------------------------------------------------------------


def _content_hash(text: str) -> int:
    """Stable 64-bit signed hash for dedup (matches screenpipe's i64 column)."""
    digest = hashlib.blake2b(text.encode(), digest_size=8).digest()
    value = int.from_bytes(digest, "big", signed=False)
    if value >= 2**63:
        value -= 2**64
    return value


def _iter_jsonl(path: Path) -> Iterator[dict]:
    """Yield each well-formed JSON record in a .jsonl file. Tolerates partial writes."""
    try:
        with path.open("rb") as fh:
            for raw in fh:
                try:
                    yield json.loads(raw)
                except Exception:
                    continue
    except FileNotFoundError:
        return


def _resolve_host_app_name(lookback_secs: float = HOST_APP_LOOKBACK_SECS) -> str:
    """Return the app_name of the most-recent non-sidecar screenpipe frame.

    Agent transcripts don't carry their own app identity — they describe the
    conversation, not where the terminal is hosted. Pulling app_name from
    whatever screenpipe just captured pins each transcript row to the real
    host app (`Code`, `Terminal`, `iTerm2`, `Ghostty`, etc.) so the ETL can
    group it with the user's actual app session instead of inventing a new one.
    """
    try:
        conn = sqlite3.connect(str(SCREENPIPE_DB), timeout=5, isolation_level=None)
        conn.execute("PRAGMA busy_timeout=5000")
        cutoff = f"-{int(lookback_secs)} seconds"
        row = conn.execute(
            """
            SELECT app_name FROM frames
            WHERE device_name != ?
              AND app_name IS NOT NULL
              AND app_name != ''
              AND timestamp > datetime('now', ?)
            ORDER BY timestamp DESC LIMIT 1
            """,
            (DEVICE_NAME, cutoff),
        ).fetchone()
        conn.close()
        if row and row[0]:
            return row[0]
    except Exception as exc:
        log.warning("host app lookup failed: %s", exc)
    return FALLBACK_APP_NAME


def _insert_frame(
    text: str, window_name: str, hash_val: int, app_name: str, capture_trigger: str
) -> None:
    """Write one accessibility frame into screenpipe's frames table."""
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
        (now, app_name, window_name, DEVICE_NAME, text, hash_val, capture_trigger),
    )
    conn.close()


def _truncate(s: str, n: int) -> str:
    s = s.strip()
    return s if len(s) <= n else s[:n] + "…"


# ---------------------------------------------------------------------------
# Claude Code source
# ---------------------------------------------------------------------------

CLAUDE_ASSISTANT_LABEL = "claude-code"


def _claude_format_content(content) -> str:
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
            parts.append(f"[tool_result: {_truncate(str(tr), 500)}]")
        elif btype == "thinking":
            t = block.get("thinking", "")
            if t:
                parts.append(f"[thinking] {t}")
    return "\n".join(p for p in parts if p)


def _claude_parse(jsonl_path: Path) -> Tuple[str, str]:
    """Return (transcript, title). Title is the most recent customTitle seen."""
    lines: List[str] = []
    title = ""
    for d in _iter_jsonl(jsonl_path):
        rec_type = d.get("type")
        if rec_type == "custom-title":
            ct = d.get("customTitle")
            if ct:
                title = ct
            continue
        if rec_type not in ("user", "assistant"):
            continue
        msg = d.get("message") or {}
        role_raw = msg.get("role") or rec_type
        role = CLAUDE_ASSISTANT_LABEL if role_raw == "assistant" else role_raw
        content_str = _claude_format_content(msg.get("content", ""))
        if content_str.strip():
            lines.append(f"[{role}] {content_str}")
    return "\n\n".join(lines), title


def _claude_project_label(project_dir: Path) -> str:
    """Trailing segment of the sanitized project dir (`...meridian` -> `meridian`)."""
    segments = [s for s in project_dir.name.split("-") if s]
    return segments[-1] if segments else project_dir.name


def _claude_window_name(project_dir: Path, jsonl_path: Path, title: str) -> str:
    short_uuid = jsonl_path.stem[:8]
    project = _claude_project_label(project_dir)
    if title:
        return f"Claude Code — {project} — {_truncate(title, 60)} [{short_uuid}]"
    return f"Claude Code — {project} — {short_uuid}"


def _claude_records(max_age: float) -> Iterator[Tuple[str, str, str]]:
    """Yield (dedup_key, window_name, transcript) for active Claude sessions."""
    if not CLAUDE_PROJECTS_DIR.exists():
        return
    cutoff = time.time() - max_age
    for proj in CLAUDE_PROJECTS_DIR.iterdir():
        if not proj.is_dir():
            continue
        for f in proj.glob("*.jsonl"):
            try:
                if f.stat().st_mtime < cutoff:
                    continue
            except FileNotFoundError:
                continue
            text, title = _claude_parse(f)
            if not text:
                continue
            yield (f"claude:{f}", _claude_window_name(proj, f, title), text)


# ---------------------------------------------------------------------------
# Codex source
# ---------------------------------------------------------------------------

CODEX_ASSISTANT_LABEL = "codex"


def _codex_extract_text(payload: dict) -> str:
    """Pull plain text out of a Codex event_msg payload (user_message or agent_message)."""
    msg = payload.get("message", "")
    if isinstance(msg, str):
        return msg
    if isinstance(msg, list):
        parts = []
        for block in msg:
            if isinstance(block, dict):
                parts.append(block.get("text", "") or block.get("content", ""))
            else:
                parts.append(str(block))
        return "\n".join(p for p in parts if p)
    return ""


def _codex_parse(
    jsonl_path: Path,
) -> Tuple[str, str, str, str]:
    """Return (transcript, cwd, session_id, first_user_msg) for a Codex rollout."""
    lines: List[str] = []
    cwd = ""
    session_id = ""
    first_user_msg = ""
    for d in _iter_jsonl(jsonl_path):
        rtype = d.get("type")
        payload = d.get("payload") or {}
        if rtype == "session_meta":
            cwd = payload.get("cwd", "") or cwd
            session_id = payload.get("id", "") or session_id
            continue
        if rtype != "event_msg":
            continue
        sub = payload.get("type")
        if sub == "user_message":
            text = _codex_extract_text(payload)
            if not text.strip():
                continue
            if not first_user_msg:
                first_user_msg = text
            lines.append(f"[user] {text}")
        elif sub == "agent_message":
            text = _codex_extract_text(payload)
            if text.strip():
                lines.append(f"[{CODEX_ASSISTANT_LABEL}] {text}")
    return "\n\n".join(lines), cwd, session_id, first_user_msg


def _codex_project_label(cwd: str, jsonl_path: Path) -> str:
    """Last path component of session cwd (e.g. `meridian`). Fallback: parent date dir."""
    if cwd:
        return Path(cwd).name or cwd
    return jsonl_path.parent.name


def _codex_window_name(
    cwd: str, jsonl_path: Path, session_id: str, first_user_msg: str
) -> str:
    short_uuid = (session_id or jsonl_path.stem.split("-")[-1])[:8]
    project = _codex_project_label(cwd, jsonl_path)
    if first_user_msg:
        return f"Codex — {project} — {_truncate(first_user_msg, 60)} [{short_uuid}]"
    return f"Codex — {project} — {short_uuid}"


def _codex_records(max_age: float) -> Iterator[Tuple[str, str, str]]:
    """Yield (dedup_key, window_name, transcript) for active Codex sessions."""
    if not CODEX_SESSIONS_DIR.exists():
        return
    cutoff = time.time() - max_age
    # Sessions live at YYYY/MM/DD/rollout-*.jsonl — rglob keeps us robust to the layout.
    for f in CODEX_SESSIONS_DIR.rglob("rollout-*.jsonl"):
        try:
            if f.stat().st_mtime < cutoff:
                continue
        except FileNotFoundError:
            continue
        text, cwd, sid, first_msg = _codex_parse(f)
        if not text:
            continue
        yield (f"codex:{f}", _codex_window_name(cwd, f, sid, first_msg), text)


# ---------------------------------------------------------------------------
# Main loop — fans out across both sources
# ---------------------------------------------------------------------------

SOURCES = [
    # (name, records_fn, capture_trigger)
    ("claude", _claude_records, "claude_session"),
    ("codex", _codex_records, "codex_session"),
]


def main() -> None:
    if not SCREENPIPE_DB.exists():
        log.error("screenpipe DB not found at %s", SCREENPIPE_DB)
        sys.exit(1)

    have_claude = CLAUDE_PROJECTS_DIR.exists()
    have_codex = CODEX_SESSIONS_DIR.exists()
    if not (have_claude or have_codex):
        log.error(
            "neither %s nor %s exists — no sources to watch",
            CLAUDE_PROJECTS_DIR,
            CODEX_SESSIONS_DIR,
        )
        sys.exit(1)

    log.info(
        "starting — DB=%s claude=%s codex=%s poll=%.0fs max-age=%.0fs",
        SCREENPIPE_DB,
        CLAUDE_PROJECTS_DIR if have_claude else "(missing)",
        CODEX_SESSIONS_DIR if have_codex else "(missing)",
        POLL_INTERVAL,
        MAX_AGE_SECS,
    )

    last_hash: Dict[str, int] = {}

    while True:
        for _name, records_fn, trigger in SOURCES:
            try:
                for key, window, text in records_fn(MAX_AGE_SECS):
                    h = _content_hash(text)
                    if last_hash.get(key) == h:
                        continue
                    app_name = _resolve_host_app_name()
                    _insert_frame(text, window, h, app_name, trigger)
                    last_hash[key] = h
                    log.info(
                        "inserted %d chars — app=%s — %s",
                        len(text),
                        app_name,
                        window,
                    )
            except Exception as exc:
                log.error("%s source error: %s", _name, exc)

        time.sleep(POLL_INTERVAL)


if __name__ == "__main__":
    main()
