"""Claude Code SessionEnd hook entry point.

Claude Code invokes a hook script and feeds it a JSON payload on stdin
describing the event. For SessionEnd the payload contains the session id
and the path to the JSONL transcript. We register that one session
immediately, return non-zero only on unexpected internal failure.

Hook installation: see `services/scripts/install-claude-hook.sh`. The
script that Claude Code actually invokes is
`python -m coding_agent_indexer.hook`,
making this module the user-facing executable entry point.

Why this is separate from daemon.py:
  * different invocation model (one-shot, stdin payload, parent process
    waits for exit)
  * lighter logging (no tick loop, no state file)
  * fast: don't import the daemon's polling code

Exit codes:
  0  registered (or idempotently no-op'd)
  0  payload couldn't be parsed but we didn't crash — never block Claude
  1  unexpected exception (logged with traceback)
"""
from __future__ import annotations

import json
import logging
import sys
from pathlib import Path
from typing import Optional

from agents import observability
from coding_agent_indexer import register

log = logging.getLogger(__name__)


def main(argv: Optional[list[str]] = None) -> int:
    observability.setup("meridian-coding-agent-indexer-hook")

    payload = _read_payload(argv or sys.argv[1:])
    jsonl_path = _extract_jsonl_path(payload)

    if jsonl_path is None:
        log.warning("hook: could not determine JSONL path from payload %r", payload)
        observability.shutdown()
        return 0                                            # never block Claude on bad payload

    log.info("hook: registering %s", jsonl_path)
    try:
        result = register.register_ended_session(jsonl_path)
    except Exception as exc:                                # noqa: BLE001
        log.exception("hook: unexpected failure registering %s", jsonl_path)
        observability.shutdown()
        return 1
    log.info(
        "hook: outcome=%s uuid=%s row_id=%s host=%s",
        result.outcome.value, result.session_uuid, result.row_id, result.host_app,
    )
    observability.shutdown()
    return 0


# ──────────────────────── Payload handling ─────────────────────────────────────


def _read_payload(argv: list[str]) -> dict:
    """Hook payload arrives on stdin as JSON. Fall back to argv for manual tests.

    `python -m coding_agent_indexer.hook /path/to/x.jsonl` works
    for quick testing without piping JSON in.
    """
    # Path passed as positional arg → wrap in fake payload
    if argv:
        return {"jsonl_path": argv[0]}

    # Stdin JSON
    try:
        raw = sys.stdin.read()
    except Exception:
        return {}
    if not raw.strip():
        return {}
    try:
        return json.loads(raw)
    except json.JSONDecodeError:
        return {}


def _extract_jsonl_path(payload: dict) -> Optional[Path]:
    """Tease the JSONL path out of whatever shape the hook gave us.

    Claude Code's hook payload schema may evolve; we probe a handful of
    plausible keys. Fall back to constructing from `session_id` + the
    canonical project-dir convention.
    """
    direct = payload.get("jsonl_path") or payload.get("transcript_path")
    if direct:
        return Path(str(direct)).expanduser()

    session_id = payload.get("session_id") or payload.get("sessionId")
    cwd        = payload.get("cwd") or payload.get("project_cwd")
    if session_id and cwd:
        sanitized = "-" + str(cwd).replace("/", "-")
        return (
            Path("~/.claude/projects").expanduser()
            / sanitized
            / f"{session_id}.jsonl"
        )

    return None


if __name__ == "__main__":
    raise SystemExit(main())
