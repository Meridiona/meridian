"""Detect which terminal / IDE is hosting the `claude` CLI.

Strategy: `pgrep -x claude` finds processes whose executable basename is
exactly `claude` (catches both the persistent VS Code extension claude
and any terminal-launched claude that happens to be alive). We walk the
parent chain of each match and return the first ancestor that matches
a known terminal/IDE name pattern.

Verified live on this codebase to return "Code" reliably for VS Code
sessions; falls through to "unknown" only if no `claude` process exists
at the moment we check AND no terminal app is otherwise discoverable.

Used by the indexer when registering an ended session — we want to tag
the resulting app_sessions row with the right host app so the UI groups
correctly and the PM updater attributes time correctly.
"""
from __future__ import annotations

import logging
import re
import subprocess
from typing import List, Tuple

log = logging.getLogger(__name__)

FALLBACK_APP_NAME = "unknown"

# `ps -o comm` returns the executable basename (or path on macOS for
# bundle apps). First regex match wins.
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

# What "valid host app" means downstream — anything not in this set is
# treated as a misfire by callers that cache the result.
TERMINAL_ALLOWLIST = frozenset(name for _, name in _TERMINAL_PATTERNS)


def detect_host_app() -> str:
    """Best-effort terminal/IDE for the currently-running `claude` process.

    Returns the canonical app name (e.g. "Code", "iTerm2") or
    `FALLBACK_APP_NAME` if no live `claude` process is found.
    """
    try:
        out = subprocess.run(
            ["pgrep", "-x", "claude"],
            capture_output=True, text=True, timeout=2,
        ).stdout.strip()
    except Exception as exc:                                # noqa: BLE001
        log.debug("pgrep failed: %s", exc)
        return FALLBACK_APP_NAME

    pids = [p for p in out.split("\n") if p]
    for pid in pids:
        app = _walk_to_terminal(pid)
        if app:
            return app
    return FALLBACK_APP_NAME


# ──────────────────────── Internals ────────────────────────────────────────────


def _walk_to_terminal(pid: str, max_depth: int = 8) -> str:
    """Walk the parent chain of `pid` looking for a known terminal/IDE."""
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
            name = _comm_to_app(comm)
            if name:
                return name
            if ppid in ("0", "1", current):
                return ""
            current = ppid
        except Exception:
            return ""
    return ""


def _comm_to_app(comm: str) -> str:
    for pattern, name in _TERMINAL_PATTERNS:
        if re.search(pattern, comm, re.IGNORECASE):
            return name
    return ""
