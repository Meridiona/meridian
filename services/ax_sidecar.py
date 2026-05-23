"""
ax_sidecar — injects VS Code a11y terminal content into screenpipe's frames table.

Polls every 3 s when VS Code is the frontmost app. Runs the compiled
swift/ax_terminal binary (which walks from AXFocusedUIElement up to AXWebArea
and then down to depth 35) and inserts the captured text as an accessibility
frame into screenpipe's SQLite DB. Deduplicates on content hash so unchanged
screens don't generate rows.

Usage:
  python -m services.ax_sidecar            # run in foreground
  # or via launchd — see services/scripts/install-ax-sidecar.sh
"""

import hashlib
import logging
import os
import sqlite3
import subprocess
import sys
import time
from datetime import datetime, timezone
from pathlib import Path
from typing import Optional

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

_here = Path(__file__).parent
AX_TERMINAL = (_here.parent / "swift" / "ax_terminal").resolve()

POLL_INTERVAL = float(os.environ.get("AX_SIDECAR_INTERVAL", "3"))
DEVICE_NAME = "ax-sidecar"


def _frontmost_app() -> str:
    """Return the localizedName of the current frontmost application."""
    result = subprocess.run(
        [
            "osascript",
            "-e",
            'tell application "System Events" to return name of first application process whose frontmost is true',
        ],
        capture_output=True,
        text=True,
        timeout=3,
    )
    return result.stdout.strip()


def _window_title() -> str:
    """Return VS Code's current frontmost window title."""
    result = subprocess.run(
        [
            "osascript",
            "-e",
            'tell application "Code" to return name of front window',
        ],
        capture_output=True,
        text=True,
        timeout=3,
    )
    return result.stdout.strip() or "VS Code"


def _run_ax_terminal() -> str:
    """Run the compiled ax_terminal binary and return its stdout."""
    result = subprocess.run(
        [str(AX_TERMINAL)],
        capture_output=True,
        text=True,
        timeout=10,
    )
    return result.stdout.strip()


def _content_hash(text: str) -> int:
    """Stable 64-bit signed hash for dedup (matches screenpipe's i64 column)."""
    digest = hashlib.blake2b(text.encode(), digest_size=8).digest()
    value = int.from_bytes(digest, "big", signed=False)
    # Fit into SQLite INTEGER (signed 64-bit)
    if value >= 2**63:
        value -= 2**64
    return value


def _insert_frame(text: str, window_name: str, hash_val: int) -> None:
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
        VALUES (0, ?, 'Code', ?, 1, ?, ?, ?, 'ax_sidecar', 'accessibility')
        """,
        (now, window_name, DEVICE_NAME, text, hash_val),
    )
    conn.close()


def main() -> None:
    if not AX_TERMINAL.exists():
        log.error("ax_terminal binary not found at %s — compile it first:", AX_TERMINAL)
        log.error("  swiftc swift/ax_terminal.swift -o swift/ax_terminal -O")
        sys.exit(1)

    if not SCREENPIPE_DB.exists():
        log.error("screenpipe DB not found at %s", SCREENPIPE_DB)
        sys.exit(1)

    log.info("starting — DB=%s poll=%.0fs", SCREENPIPE_DB, POLL_INTERVAL)

    last_hash = None  # type: Optional[int]

    while True:
        try:
            app = _frontmost_app()
            if app == "Code":
                text = _run_ax_terminal()
                if text:
                    h = _content_hash(text)
                    if h != last_hash:
                        window = _window_title()
                        _insert_frame(text, window, h)
                        last_hash = h
                        log.info("inserted %d chars — %s", len(text), window)
        except subprocess.TimeoutExpired:
            log.warning("ax_terminal timed out, skipping")
        except Exception as exc:
            log.error("error: %s", exc)

        time.sleep(POLL_INTERVAL)


if __name__ == "__main__":
    main()
