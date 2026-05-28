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
  app_name         = resolved from the OS process tree of running `claude`
                     processes (the terminal/IDE actually hosting the agent —
                     e.g. 'Code', 'Terminal', 'iTerm2', 'Ghostty')

Usage:
  python -m services.ax_sidecar            # run in foreground
  # or via launchd — see services/scripts/install-ax-sidecar.sh
"""

import hashlib
import json
import logging
import os
import re
import sqlite3
import subprocess
import sys
import time
from datetime import datetime, timedelta, timezone
from pathlib import Path
from typing import Dict, Iterator, List, Optional, Tuple

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
# Error log dedup — persistent failures (missing DB, locked sqlite, missing
# source dirs) repeat every 3s; logging them once until the situation changes
# keeps the log readable without losing signal.
# ---------------------------------------------------------------------------

_last_log: Dict[str, str] = {}


def _log_state(level: int, key: str, msg: str, *args) -> None:
    """Log `msg` only when it differs from the last message logged under `key`."""
    text = msg % args if args else msg
    if _last_log.get(key) == text:
        return
    _last_log[key] = text
    log.log(level, text)


def _clear_state(key: str) -> None:
    """Reset dedup so the next log under `key` is emitted again."""
    _last_log.pop(key, None)


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
    """Yield each well-formed JSON record in a .jsonl file.

    Tolerates partial writes (a final line being written when we open the
    file), files disappearing or rotating mid-read, and permission/IO
    errors — all return an empty iterator silently so callers can move on.
    """
    try:
        fh = path.open("rb")
    except (FileNotFoundError, PermissionError, OSError):
        return
    try:
        for raw in fh:
            try:
                yield json.loads(raw)
            except Exception:
                continue
    except OSError:
        return
    finally:
        try:
            fh.close()
        except Exception:
            pass


def _file_birth_time(path: Path) -> Optional[datetime]:
    """File creation time (macOS st_birthtime). Falls back to mtime elsewhere."""
    try:
        stat = path.stat()
    except (FileNotFoundError, PermissionError, OSError):
        return None
    ts = getattr(stat, "st_birthtime", None) or stat.st_mtime
    return datetime.fromtimestamp(ts, tz=timezone.utc)


def _lookup_app_near(
    anchor: datetime, window_secs: float = HOST_APP_LOOKBACK_SECS, before_only: bool = False
) -> str:
    """Most-recent non-sidecar app_name within window_secs of `anchor`. Empty if none.

    When `before_only=True`, only frames at or before `anchor` are considered —
    use this for birth-time lookups so a post-start app switch doesn't win.
    """
    if not SCREENPIPE_DB.exists():
        _log_state(
            logging.WARNING, "screenpipe_db_missing",
            "screenpipe DB not found at %s — host app lookups disabled until it appears",
            SCREENPIPE_DB,
        )
        return ""
    _clear_state("screenpipe_db_missing")
    fmt = "%Y-%m-%dT%H:%M:%S+00:00"
    lo = (anchor - timedelta(seconds=window_secs)).strftime(fmt)
    hi = anchor.strftime(fmt) if before_only else (anchor + timedelta(seconds=window_secs)).strftime(fmt)
    conn = None
    try:
        conn = sqlite3.connect(str(SCREENPIPE_DB), timeout=5, isolation_level=None)
        conn.execute("PRAGMA busy_timeout=5000")
        row = conn.execute(
            """
            SELECT app_name FROM frames
            WHERE device_name != ?
              AND app_name IS NOT NULL
              AND app_name != ''
              AND timestamp BETWEEN ? AND ?
            ORDER BY timestamp DESC LIMIT 1
            """,
            (DEVICE_NAME, lo, hi),
        ).fetchone()
        if row and row[0]:
            _clear_state("host_app_lookup_failed")
            return row[0]
    except sqlite3.Error as exc:
        _log_state(
            logging.WARNING, "host_app_lookup_failed",
            "host app lookup failed: %s", exc,
        )
    finally:
        if conn is not None:
            try:
                conn.close()
            except Exception:
                pass
    return ""


# Maps substrings found in a process comm/path to a display app name.
# Checked in order — first match wins.
_TERMINAL_PATTERNS: List[Tuple[str, str]] = [
    (r"Visual Studio Code|Code Helper|/Code\.app/", "Code"),
    (r"iTerm2", "iTerm2"),
    (r"/Terminal\.app/|MacOS/Terminal\b", "Terminal"),
    (r"Ghostty", "Ghostty"),
    (r"Warp\b", "Warp"),
    (r"Alacritty", "Alacritty"),
    (r"Hyper\b", "Hyper"),
    (r"\bkitty\b", "kitty"),
    (r"WezTerm", "WezTerm"),
]


def _comm_to_app(comm: str) -> str:
    for pattern, name in _TERMINAL_PATTERNS:
        if re.search(pattern, comm, re.IGNORECASE):
            return name
    return ""


def _walk_proc_to_terminal(pid: str, max_depth: int = 8) -> str:
    """Walk the process tree upward from `pid`, returning the terminal/IDE app name."""
    current = pid
    for _ in range(max_depth):
        try:
            out = subprocess.run(
                ["ps", "-o", "ppid=,comm=", "-p", current],
                capture_output=True, text=True, timeout=2,
            ).stdout.strip()
            if not out:
                return ""
            parts = out.split(None, 1)
            if len(parts) < 2:
                return ""
            ppid, comm = parts[0], parts[1]
            app = _comm_to_app(comm)
            if app:
                return app
            if ppid in ("0", "1", current):
                return ""
            current = ppid
        except Exception:
            return ""
    return ""


def _find_host_app_from_process(_jsonl_path: Path) -> str:
    """Detect the terminal/IDE hosting a Claude Code session via the OS process tree.

    Finds all running `claude` processes and walks each parent chain to find
    the terminal/IDE app. Returns the first match — the same user typically
    only has claude processes in one terminal, and if they have multiple, any
    one of them gives the right answer for the app_name tag.
    """
    try:
        pids_out = subprocess.run(
            ["pgrep", "-f", "claude"],
            capture_output=True, text=True, timeout=2,
        ).stdout.strip()
        pids = [p for p in pids_out.split("\n") if p]
    except Exception:
        return ""

    for pid in pids:
        app = _walk_proc_to_terminal(pid)
        if app:
            return app
    return ""


def _resolve_session_host_app(jsonl_path: Path) -> str:
    """Resolve the terminal/IDE hosting a session.

    Primary: OS process tree — walks the parent chain of running `claude`
    processes to find the actual terminal/IDE, independent of screen state.
    Fallback: screenpipe frames around the session birth time.
    Final fallback: FALLBACK_APP_NAME.
    """
    app = _find_host_app_from_process(jsonl_path)
    if app:
        return app
    birth = _file_birth_time(jsonl_path)
    if birth is not None:
        hit = _lookup_app_near(birth, window_secs=60.0, before_only=True)
        if hit:
            return hit
    hit = _lookup_app_near(
        datetime.now(timezone.utc), window_secs=HOST_APP_LOOKBACK_SECS
    )
    return hit or FALLBACK_APP_NAME


def _insert_frame(
    text: str, window_name: str, hash_val: int, app_name: str, capture_trigger: str
) -> bool:
    """Write one accessibility frame into screenpipe's frames table. Returns True on success."""
    if not SCREENPIPE_DB.exists():
        _log_state(
            logging.WARNING, "screenpipe_db_missing",
            "screenpipe DB not found at %s — inserts disabled until it appears",
            SCREENPIPE_DB,
        )
        return False
    _clear_state("screenpipe_db_missing")
    conn = None
    try:
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
        _clear_state("insert_failed")
        return True
    except sqlite3.Error as exc:
        _log_state(
            logging.WARNING, "insert_failed",
            "frame insert failed (will retry next tick): %s", exc,
        )
        return False
    finally:
        if conn is not None:
            try:
                conn.close()
            except Exception:
                pass


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
    """Yield (dedup_key, window_name, transcript) for active Claude sessions.

    Silently returns nothing if Claude Code isn't installed (the projects dir
    doesn't exist) or if the user simply hasn't run it recently — both are
    expected idle states, not errors.
    """
    if not CLAUDE_PROJECTS_DIR.exists():
        return
    cutoff = time.time() - max_age
    try:
        projects = list(CLAUDE_PROJECTS_DIR.iterdir())
    except (FileNotFoundError, PermissionError, OSError) as exc:
        _log_state(
            logging.WARNING, "claude_iterdir_failed",
            "claude projects dir unreadable: %s", exc,
        )
        return
    _clear_state("claude_iterdir_failed")
    for proj in projects:
        try:
            if not proj.is_dir():
                continue
        except OSError:
            continue
        try:
            jsonls = list(proj.glob("*.jsonl")) + list(proj.glob("subagents/*.jsonl"))
        except (FileNotFoundError, PermissionError, OSError):
            continue
        for f in jsonls:
            try:
                if f.stat().st_mtime < cutoff:
                    continue
            except (FileNotFoundError, PermissionError, OSError):
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
    """Yield (dedup_key, window_name, transcript) for active Codex sessions.

    Silently returns nothing if Codex isn't installed or hasn't been used —
    both are expected idle states, not errors. To stay cheap when Codex is
    used heavily over many days, only the current and previous day's
    subdirectories are scanned (recent rollouts sort to the top by date).
    """
    if not CODEX_SESSIONS_DIR.exists():
        return
    cutoff = time.time() - max_age
    today = datetime.now(timezone.utc)
    candidates = {today, today - timedelta(days=1)}
    seen: set = set()
    for day in candidates:
        day_dir = CODEX_SESSIONS_DIR / day.strftime("%Y") / day.strftime("%m") / day.strftime("%d")
        if not day_dir.exists():
            continue
        try:
            files = list(day_dir.glob("rollout-*.jsonl"))
        except (FileNotFoundError, PermissionError, OSError):
            continue
        for f in files:
            if f in seen:
                continue
            seen.add(f)
            try:
                if f.stat().st_mtime < cutoff:
                    continue
            except (FileNotFoundError, PermissionError, OSError):
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
    log.info(
        "starting — DB=%s claude=%s codex=%s poll=%.0fs max-age=%.0fs",
        SCREENPIPE_DB,
        CLAUDE_PROJECTS_DIR,
        CODEX_SESSIONS_DIR,
        POLL_INTERVAL,
        MAX_AGE_SECS,
    )
    log.info(
        "claude available=%s, codex available=%s — missing sources are skipped (not fatal)",
        CLAUDE_PROJECTS_DIR.exists(),
        CODEX_SESSIONS_DIR.exists(),
    )

    last_hash: Dict[str, int] = {}
    # Resolved host app for each session, set once on first sighting and
    # kept for the session's lifetime so a single conversation gets one
    # consistent app_name even if the user briefly switches focus.
    session_app: Dict[str, str] = {}

    while True:
        try:
            for _name, records_fn, trigger in SOURCES:
                try:
                    for key, window, text in records_fn(MAX_AGE_SECS):
                        h = _content_hash(text)
                        if last_hash.get(key) == h:
                            continue
                        # Resolve the host app once per session. Don't cache
                        # FALLBACK_APP_NAME — screenpipe may just be lagging,
                        # so we retry on the next message until we get a real
                        # app name.
                        cached_app = session_app.get(key)
                        if cached_app is None or cached_app == FALLBACK_APP_NAME:
                            _, _, path_str = key.partition(":")
                            resolved = _resolve_session_host_app(Path(path_str))
                            session_app[key] = resolved
                            if cached_app != resolved:
                                log.info(
                                    "session host app -> %s — %s",
                                    resolved,
                                    window,
                                )
                        app_name = session_app[key]
                        ok = _insert_frame(text, window, h, app_name, trigger)
                        if not ok:
                            # Don't advance last_hash — retry next tick once
                            # the DB is available again.
                            continue
                        last_hash[key] = h
                        log.info(
                            "inserted %d chars — app=%s — %s",
                            len(text),
                            app_name,
                            window,
                        )
                except Exception as exc:
                    _log_state(
                        logging.ERROR, f"{_name}_source_error",
                        "%s source error: %s", _name, exc,
                    )
        except Exception as exc:
            # Belt-and-suspenders: never let the poll loop die from a bug
            # in a single tick. Log and try again next interval.
            _log_state(
                logging.ERROR, "tick_error",
                "unexpected error in poll tick: %s", exc,
            )

        time.sleep(POLL_INTERVAL)


if __name__ == "__main__":
    main()
