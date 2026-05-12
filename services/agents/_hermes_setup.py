"""Hermes import shim for dual-mode (dev / prod) operation.

In prod (`HERMES_DEV_MODE` unset or "0") the installed `hermes-agent` package
provides `run_agent` — no sys.path manipulation is needed.

In dev (`HERMES_DEV_MODE=1`) we prepend `services/.hermes/` (a gitignored
local checkout) so the local source shadows the installed package, allowing
breakpoint debugging without reinstalling on every edit.

Usage
-----
Call `ensure_hermes_importable()` once, early in any module that needs
`from run_agent import AIAgent`, before the import itself:

    from agents._hermes_setup import ensure_hermes_importable
    ensure_hermes_importable()
    from run_agent import AIAgent  # noqa: E402

No code runs at import time — only calling the function triggers path surgery.
"""
from __future__ import annotations

import os
import sys
from pathlib import Path


def ensure_hermes_importable() -> None:
    """Ensure `run_agent` can be imported, honouring HERMES_DEV_MODE.

    Dev mode  (HERMES_DEV_MODE=1): inserts ``services/.hermes/`` at
    ``sys.path[0]`` so the local source shadows the installed package.

    Prod mode (HERMES_DEV_MODE=0 or unset): no-op — the installed
    ``hermes-agent`` package already exposes ``run_agent``.
    """
    if os.environ.get("HERMES_DEV_MODE", "0") != "1":
        return

    dev_path = Path(__file__).resolve().parents[1] / ".hermes"
    dev_path_str = str(dev_path)

    if dev_path_str not in sys.path:
        sys.path.insert(0, dev_path_str)
