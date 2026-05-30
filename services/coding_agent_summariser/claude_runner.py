"""Run `claude -p` with the session-summary skill and structured output.

Returns the validated `{summary, blockers}` dict, or raises:
  * `RateLimited`     — subscription usage/rate limit hit (caller falls back to MLX)
  * `SummariserError` — anything else (missing CLI, timeout, bad output);
                        caller leaves the row NULL and retries next tick.

Auth: uses the user's Claude subscription. We explicitly drop ANTHROPIC_API_KEY
from the child env so a stray key can't silently switch us to metered API
billing, and set MERIDIAN_SUMMARISER=1 so the indexer's SessionEnd hook can
ignore the throwaway session this call spawns. `--no-session-persistence` means
no JSONL is written for it either.
"""
from __future__ import annotations

import json
import logging
import os
import subprocess
from typing import Optional

from coding_agent_summariser import config
from coding_agent_summariser.prompts import (
    SUMMARY_SCHEMA,
    first_line,
    looks_rate_limited,
)

log = logging.getLogger(__name__)


class SummariserError(RuntimeError):
    """Recoverable failure — leave the row unsummarised and retry later."""


class RateLimited(SummariserError):
    """Subscription rate/usage limit — switch to the MLX fallback."""


def run_claude(
    stdin_text: str,
    *,
    model: Optional[str] = None,
    skill: Optional[str] = None,
    timeout: Optional[int] = None,
) -> dict:
    """Invoke `claude -p /<skill>` over stdin; return {summary, blockers}."""
    model = model or config.CLAUDE_MODEL
    skill = skill or config.SKILL_NAME
    timeout = timeout or config.CLAUDE_TIMEOUT_S

    cmd = [
        "claude", "-p",
        f"/{skill} Summarise the coding-session transcript provided on stdin.",
        "--output-format", "json",
        "--json-schema", json.dumps(SUMMARY_SCHEMA),
        "--model", model,
        "--no-session-persistence",
        "--strict-mcp-config",          # drop MCP overhead; keeps skills working
    ]

    env = os.environ.copy()
    env.pop("ANTHROPIC_API_KEY", None)  # force subscription auth, never metered API
    env["MERIDIAN_SUMMARISER"] = "1"    # let the indexer hook skip our spawned session

    try:
        proc = subprocess.run(
            cmd,
            input=stdin_text,
            capture_output=True,
            text=True,
            timeout=timeout,
            cwd=str(config.MERIDIAN_HOME),   # neutral cwd — no project CLAUDE.md to load
            env=env,
        )
    except FileNotFoundError as exc:
        raise SummariserError("claude CLI not found on PATH") from exc
    except subprocess.TimeoutExpired as exc:
        raise SummariserError(f"claude -p timed out after {timeout}s") from exc

    if proc.returncode != 0:
        blob = f"{proc.stderr}\n{proc.stdout}"
        if looks_rate_limited(blob):
            raise RateLimited(first_line(proc.stderr) or "rate/usage limit")
        raise SummariserError(
            f"claude exited {proc.returncode}: {first_line(proc.stderr) or first_line(proc.stdout)}"
        )

    try:
        payload = json.loads(proc.stdout)
    except json.JSONDecodeError as exc:
        raise SummariserError(f"claude output not JSON: {proc.stdout[:200]!r}") from exc

    # Even on exit 0 the result envelope can report an error (e.g. mid-run limit).
    if payload.get("is_error") or payload.get("subtype") not in (None, "success"):
        detail = str(payload.get("result") or payload.get("subtype") or "error")
        if looks_rate_limited(detail):
            raise RateLimited(detail[:200])
        raise SummariserError(f"claude result error: {detail[:200]}")

    structured = payload.get("structured_output")
    if not isinstance(structured, dict) or not (structured.get("summary") or "").strip():
        raise SummariserError("claude returned no usable structured summary")

    return {
        "summary": structured["summary"].strip(),
        "blockers": [b for b in structured.get("blockers", []) if isinstance(b, str)],
    }
