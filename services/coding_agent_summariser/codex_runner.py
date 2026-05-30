"""Run `codex exec` to summarise a Codex session (symmetry with claude_runner).

Codex sessions are summarised with the user's own Codex CLI / auth, just as
Claude sessions use `claude -p`. Returns the validated `{summary, blockers}`
dict, or raises `RateLimited` (Codex usage limit → caller falls back to MLX) /
`SummariserError` (anything else → retry later).

Flags chosen for a safe, side-effect-free, non-interactive run:
  * `-s read-only`            — the agent may not edit anything
  * `--skip-git-repo-check`   — we run from ~/.meridian, not a git repo
  * `--ephemeral`             — no session file written → indexer won't re-pick it
  * `--output-schema FILE`    — final message conforms to SUMMARY_SCHEMA
  * `-o FILE`                 — capture that final message cleanly
"""
from __future__ import annotations

import json
import logging
import os
import subprocess
import tempfile
from pathlib import Path
from typing import Optional, Tuple

from coding_agent_summariser import config
from coding_agent_summariser.claude_runner import RateLimited, SummariserError
from coding_agent_summariser.prompts import (
    SUMMARY_INSTRUCTION,
    SUMMARY_SCHEMA,
    first_line,
    looks_rate_limited,
)

log = logging.getLogger(__name__)

_PROMPT = SUMMARY_INSTRUCTION + " Summarise the coding-session transcript provided on stdin."


def run_codex(
    stdin_text: str,
    *,
    model: Optional[str] = None,
    timeout: Optional[int] = None,
) -> dict:
    """Invoke `codex exec`, feeding the transcript on stdin; return {summary, blockers}."""
    model = model if model is not None else config.CODEX_MODEL
    timeout = timeout or config.CODEX_TIMEOUT_S

    with tempfile.TemporaryDirectory(prefix="codex_summ_") as td:
        schema_path = Path(td) / "schema.json"
        out_path = Path(td) / "last_message.txt"
        schema_path.write_text(json.dumps(SUMMARY_SCHEMA))

        cmd = [
            "codex", "exec", _PROMPT,
            "-s", "read-only",
            "--skip-git-repo-check",
            "--ephemeral",
            "--output-schema", str(schema_path),
            "-o", str(out_path),
            "-C", str(config.MERIDIAN_HOME),
        ]
        if model:
            cmd += ["-m", model]

        env = os.environ.copy()
        env["MERIDIAN_SUMMARISER"] = "1"   # marker for the indexer hook (defensive)

        try:
            proc = subprocess.run(
                cmd, input=stdin_text, capture_output=True, text=True,
                timeout=timeout, cwd=str(config.MERIDIAN_HOME), env=env,
            )
        except FileNotFoundError as exc:
            raise SummariserError("codex CLI not found on PATH") from exc
        except subprocess.TimeoutExpired as exc:
            raise SummariserError(f"codex exec timed out after {timeout}s") from exc

        if proc.returncode != 0:
            blob = f"{proc.stderr}\n{proc.stdout}"
            if looks_rate_limited(blob):
                raise RateLimited(first_line(proc.stderr) or "codex usage limit")
            raise SummariserError(f"codex exited {proc.returncode}: {first_line(proc.stderr)}")

        text = out_path.read_text().strip() if out_path.exists() else ""
        if not text:
            raise SummariserError("codex produced no output")

        summary, blockers = _extract(text)
        if not summary:
            raise SummariserError("codex output had no usable summary")
        return {"summary": summary, "blockers": blockers}


def _extract(text: str) -> Tuple[str, list]:
    """Pull (summary, blockers) from codex's final message.

    With --output-schema the message should be a JSON object; if codex returns
    prose instead, fall back to treating the whole text as the summary.
    """
    obj = _try_json_object(text)
    if isinstance(obj, dict) and isinstance(obj.get("summary"), str):
        return (
            obj["summary"].strip(),
            [b for b in obj.get("blockers", []) if isinstance(b, str)],
        )
    return text.strip(), []


def _try_json_object(text: str):
    try:
        return json.loads(text)
    except json.JSONDecodeError:
        # Tolerate a JSON object embedded in surrounding prose.
        start, end = text.find("{"), text.rfind("}")
        if 0 <= start < end:
            try:
                return json.loads(text[start:end + 1])
            except json.JSONDecodeError:
                return None
    return None
