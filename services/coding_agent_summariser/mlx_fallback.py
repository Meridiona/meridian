"""Fallback summariser — the local MLX server's OpenAI-compatible endpoint.

Used only when `claude -p` is rate-limited or unavailable. The local model is
the degraded path: we ask for plain prose (no structured-output / schema, which
the MLX endpoint doesn't enforce) and store that. Uses stdlib `urllib` so the
summariser carries no extra dependency.

Raises `SummariserError` on any failure so the caller leaves the row NULL and
retries later (next tick may have Claude back).
"""
from __future__ import annotations

import json
import logging
import urllib.error
import urllib.request

import re

from coding_agent_summariser import config
from coding_agent_summariser.claude_runner import SummariserError
from coding_agent_summariser.prompts import SUMMARY_RULES

log = logging.getLogger(__name__)

# /summarise enforces the {summary} schema via outlines, so the system message
# only needs the content rules (no output-format clause).
_SYSTEM = SUMMARY_RULES


def summarise(stdin_text: str) -> str:
    """Return a prose summary from the local MLX server, or raise SummariserError.

    Uses the schema-constrained `/summarise` endpoint (outlines FSM forces the
    {summary} shape), which is what stops this reasoning model from leaking its
    chain-of-thought. The `_clean`/reasoning gate below is kept as cheap defence.
    """
    url = f"http://{config.MLX_HOST}:{config.MLX_PORT}/summarise"
    body = json.dumps({
        "transcript": _tail_cap(stdin_text),     # MLX-only: tail of the session
        "system": _SYSTEM,
        "max_tokens": config.MLX_MAX_TOKENS,
        "temperature": 0.2,
    }).encode("utf-8")

    req = urllib.request.Request(
        url, data=body, headers={"Content-Type": "application/json"}, method="POST",
    )
    try:
        with urllib.request.urlopen(req, timeout=config.MLX_TIMEOUT_S) as resp:
            payload = json.loads(resp.read().decode("utf-8"))
    except (urllib.error.URLError, TimeoutError, OSError) as exc:
        raise SummariserError(f"MLX fallback unreachable: {exc}") from exc
    except json.JSONDecodeError as exc:
        raise SummariserError(f"MLX fallback bad JSON: {exc}") from exc

    text = _clean((payload.get("summary") or "").strip())
    if not text:
        raise SummariserError("MLX fallback returned empty summary")
    if _looks_like_reasoning(text):
        # The local model leaked its chain-of-thought instead of a clean
        # summary. Reject rather than store garbage — the row stays NULL and
        # gets a proper summary later from claude/codex (or a future
        # constrained-MLX path).
        raise SummariserError("MLX output looks like leaked reasoning — rejected")
    return text


def _tail_cap(text: str) -> str:
    """Keep only the TAIL (~MLX_INPUT_MAX_TOKENS) of the transcript for MLX.

    The bottom of a session holds the most recent activity / outcome, which is
    what we want the local model to summarise. Claude/Codex are unaffected —
    this trims only the MLX request. Token count is approximated by chars.
    """
    max_chars = config.MLX_INPUT_MAX_TOKENS * config.MLX_CHARS_PER_TOKEN
    if len(text) <= max_chars:
        return text
    return "…[earlier session truncated — most recent activity below]…\n\n" + text[-max_chars:]


_REASONING_MARKERS = (
    "thinking process",
    "analyze the request",
    "**analyze",
    "constraint:",
    "decision:",
    "re-evaluation",
    "let me think",
    "i must follow",
    "the transcript is",
)


def _looks_like_reasoning(text: str) -> bool:
    low = text.lower()
    if low.lstrip().startswith(("thinking process", "1.", "1)", "**analyze")):
        return True
    return sum(1 for m in _REASONING_MARKERS if m in low) >= 2


def _clean(text: str) -> str:
    """Defensively strip a reasoning model's leakage to leave just the prose.

    The prose-only system prompt usually suffices, but local reasoners can still
    emit `<think>…</think>`, a "Thinking Process:" preamble, or a JSON object.
    Order matters: drop think-tags, prefer an embedded `{"summary": …}`, then
    drop a leading reasoning preamble.
    """
    # 1. Remove <think>…</think> (and stray closing tags).
    text = re.sub(r"<think>.*?</think>", "", text, flags=re.DOTALL | re.IGNORECASE).strip()
    text = re.sub(r"</?think>", "", text, flags=re.IGNORECASE).strip()

    # 2. If it emitted JSON anyway, take the summary field.
    start, end = text.find("{"), text.rfind("}")
    if 0 <= start < end:
        try:
            obj = json.loads(text[start:end + 1])
            if isinstance(obj, dict) and isinstance(obj.get("summary"), str) and obj["summary"].strip():
                return obj["summary"].strip()
        except json.JSONDecodeError:
            pass

    # 3. Drop a leading reasoning preamble ("Thinking Process:", "Reasoning:", …)
    #    up to the first blank line that precedes ordinary prose.
    m = re.match(r"^\s*(thinking process|reasoning|analysis|let me think)\b.*?\n\s*\n",
                 text, flags=re.IGNORECASE | re.DOTALL)
    if m:
        text = text[m.end():].strip()
    return text.strip()
